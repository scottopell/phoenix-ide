//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::models::{ModelSpec, Provider};
use super::types::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage, LLM_SOURCE_HEADER,
};
use super::LlmError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Complete using OpenAI-compatible API (`OpenAI` or Fireworks)
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    if uses_responses_api(&spec.api_name) {
        complete_responses_api(spec, api_key, gateway, request).await
    } else {
        complete_chat_api(spec, api_key, gateway, request).await
    }
}

/// Complete with streaming, emitting `TokenChunk::Text` events via `chunk_tx`.
pub async fn complete_streaming(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    if uses_responses_api(&spec.api_name) {
        complete_streaming_responses_api(spec, api_key, gateway, request, chunk_tx).await
    } else {
        complete_streaming_chat_api(spec, api_key, gateway, request, chunk_tx).await
    }
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Determine the full endpoint URL based on provider, model, and gateway.
fn resolve_endpoint(spec: &ModelSpec, gateway: Option<&str>) -> String {
    let is_fireworks = spec.provider == Provider::Fireworks;

    match (gateway, is_fireworks) {
        // Fireworks via gateway
        (Some(gw), true) => format!(
            "{}/fireworks/inference/v1/chat/completions",
            gw.trim_end_matches('/')
        ),
        // OpenAI via gateway — always responses endpoint
        (Some(gw), false) => format!("{}/openai/v1/responses", gw.trim_end_matches('/')),
        // Direct Fireworks
        (None, true) => "https://api.fireworks.ai/inference/v1/chat/completions".to_string(),
        // Direct OpenAI — always responses endpoint
        (None, false) => "https://api.openai.com/v1/responses".to_string(),
    }
}

/// All non-Fireworks models use the v1/responses endpoint.
fn uses_responses_api(api_name: &str) -> bool {
    !is_fireworks_model(api_name)
}

/// Fireworks models are identified by their `api_name` prefix.
fn is_fireworks_model(api_name: &str) -> bool {
    api_name.starts_with("accounts/fireworks/")
}

/// Models that use `max_completion_tokens` instead of `max_tokens`.
fn uses_max_completion_tokens(api_name: &str) -> bool {
    matches!(api_name, "o4-mini" | "gpt-5" | "gpt-5-mini" | "gpt-5.1")
}

// ---------------------------------------------------------------------------
// Chat Completions API
// ---------------------------------------------------------------------------

/// Complete using the chat/completions API.
async fn complete_chat_api(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_endpoint(spec, gateway);
    let openai_request = translate_request(&spec.api_name, request);

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER)
        .json(&openai_request)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::network(format!("Request timeout: {e}"))
            } else if e.is_connect() {
                LlmError::network(format!("Connection failed: {e}"))
            } else {
                LlmError::network(format!("Request failed: {e}"))
            }
        })?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| LlmError::network(format!("Failed to read response: {e}")))?;

    if !status.is_success() {
        if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(&body) {
            let message = error_resp.error.message;
            return Err(match status.as_u16() {
                401 | 403 => LlmError::auth(format!("Authentication failed: {message}")),
                429 => LlmError::rate_limit(format!("Rate limit exceeded: {message}")),
                400..=499 => {
                    LlmError::invalid_request(format!("Bad request ({status}): {message}"))
                }
                500..=599 => LlmError::server_error(format!("Server error: {message}")),
                _ => LlmError::server_error(format!("Unexpected HTTP {status}: {message}")),
            });
        }
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    let openai_response: OpenAIResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    normalize_response(openai_response)
}

// ---------------------------------------------------------------------------
// Responses API
// ---------------------------------------------------------------------------

/// Complete using the v1/responses API (all non-Fireworks models).
async fn complete_responses_api(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_endpoint(spec, gateway);
    let responses_request = translate_to_responses_request(&spec.api_name, request);

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER)
        .json(&responses_request)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::network(format!("Request timeout: {e}"))
            } else if e.is_connect() {
                LlmError::network(format!("Connection failed: {e}"))
            } else {
                LlmError::network(format!("Request failed: {e}"))
            }
        })?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| LlmError::network(format!("Failed to read response: {e}")))?;

    if !status.is_success() {
        if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(&body) {
            let message = error_resp.error.message;
            return Err(match status.as_u16() {
                401 | 403 => LlmError::auth(format!("Authentication failed: {message}")),
                429 => LlmError::rate_limit(format!("Rate limit exceeded: {message}")),
                400..=499 => {
                    LlmError::invalid_request(format!("Bad request ({status}): {message}"))
                }
                500..=599 => LlmError::server_error(format!("Server error: {message}")),
                _ => LlmError::server_error(format!("Unexpected HTTP {status}: {message}")),
            });
        }
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    let responses_response: ResponsesApiResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    Ok(normalize_responses_api_response(responses_response))
}

// ---------------------------------------------------------------------------
// Streaming — Responses API
// ---------------------------------------------------------------------------

/// Accumulates state across Responses API SSE stream events.
struct ResponsesStreamAccumulator {
    input_tokens: u32,
    output_tokens: u32,
    /// Completed output items collected from `response.output_item.done` events.
    output_items: Vec<ResponsesApiOutput>,
    /// Set true when `response.done` is received.
    pub done: bool,
}

impl ResponsesStreamAccumulator {
    fn new() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            output_items: Vec::new(),
            done: false,
        }
    }

    fn process_event(
        &mut self,
        event_type: &str,
        data: &str,
        chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    ) -> Result<(), LlmError> {
        let v: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| LlmError::invalid_response(format!("Failed to parse SSE data: {e}")))?;
        match event_type {
            "response.output_text.delta" => {
                if let Some(delta) = v.get("delta").and_then(serde_json::Value::as_str) {
                    if !delta.is_empty() {
                        let _ = chunk_tx.send(super::TokenChunk::Text(delta.to_string()));
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    if let Ok(output) = serde_json::from_value::<ResponsesApiOutput>(item.clone()) {
                        self.output_items.push(output);
                    }
                }
            }
            "response.done" => {
                if let Some(usage) = v.pointer("/response/usage") {
                    self.input_tokens = u32::try_from(
                        usage
                            .get("input_tokens")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    )
                    .unwrap_or(0);
                    self.output_tokens = u32::try_from(
                        usage
                            .get("output_tokens")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    )
                    .unwrap_or(0);
                }
                self.done = true;
            }
            _ => {} // response.created, response.content_part.*, etc. — ignored
        }
        Ok(())
    }

    fn into_response(self) -> LlmResponse {
        normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: self.output_items,
            usage: ResponsesApiUsage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
            },
        })
    }
}

async fn complete_streaming_responses_api(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_endpoint(spec, gateway);
    let mut responses_request = translate_to_responses_request(&spec.api_name, request);
    responses_request.stream = Some(true);

    let client = Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER)
        .json(&responses_request)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::network(format!("Request timeout: {e}"))
            } else if e.is_connect() {
                LlmError::network(format!("Connection failed: {e}"))
            } else {
                LlmError::network(format!("Request failed: {e}"))
            }
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::network(format!("Failed to read error response: {e}")))?;
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    let mut acc = ResponsesStreamAccumulator::new();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut current_event = String::new();
    let mut current_data = String::new();
    let mut stream = response.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        byte_buf.extend_from_slice(&chunk);

        loop {
            let Some(nl_pos) = byte_buf.iter().position(|&b| b == b'\n') else {
                break;
            };
            let line = std::str::from_utf8(&byte_buf[..nl_pos])
                .unwrap_or("")
                .trim_end_matches('\r')
                .to_string();
            byte_buf.drain(..=nl_pos);

            if line.is_empty() {
                if !current_data.is_empty() {
                    acc.process_event(&current_event, &current_data, chunk_tx)?;
                    current_event.clear();
                    current_data.clear();
                    if acc.done {
                        break 'outer;
                    }
                }
            } else if let Some(data) = line.strip_prefix("data: ") {
                current_data = data.to_string();
            } else if let Some(event) = line.strip_prefix("event: ") {
                current_event = event.to_string();
            }
        }
    }

    Ok(acc.into_response())
}

// ---------------------------------------------------------------------------
// Streaming — Chat Completions API
// ---------------------------------------------------------------------------

/// Accumulates state across chat/completions SSE stream events.
struct ChatStreamAccumulator {
    prompt_tokens: u32,
    completion_tokens: u32,
    text: String,
    /// Tool call accumulator: index → (id, name, arguments)
    tool_calls: std::collections::BTreeMap<usize, (String, String, String)>,
    finish_reason: Option<String>,
}

impl ChatStreamAccumulator {
    fn new() -> Self {
        Self {
            prompt_tokens: 0,
            completion_tokens: 0,
            text: String::new(),
            tool_calls: std::collections::BTreeMap::new(),
            finish_reason: None,
        }
    }

    fn process_chunk(
        &mut self,
        data: &str,
        chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    ) -> Result<(), LlmError> {
        if data == "[DONE]" {
            return Ok(());
        }
        let v: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| LlmError::invalid_response(format!("Failed to parse SSE data: {e}")))?;

        // Usage-only chunk (sent before [DONE] by some providers)
        if let Some(usage) = v.get("usage") {
            self.prompt_tokens = u32::try_from(
                usage
                    .get("prompt_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(u64::from(self.prompt_tokens)),
            )
            .unwrap_or(self.prompt_tokens);
            self.completion_tokens = u32::try_from(
                usage
                    .get("completion_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(u64::from(self.completion_tokens)),
            )
            .unwrap_or(self.completion_tokens);
        }

        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
            return Ok(());
        };

        if let Some(fr) = choice
            .get("finish_reason")
            .and_then(serde_json::Value::as_str)
        {
            self.finish_reason = Some(fr.to_string());
        }

        let Some(delta) = choice.get("delta") else {
            return Ok(());
        };

        // Text delta
        if let Some(text) = delta.get("content").and_then(serde_json::Value::as_str) {
            if !text.is_empty() {
                self.text.push_str(text);
                let _ = chunk_tx.send(super::TokenChunk::Text(text.to_string()));
            }
        }

        // Tool call deltas
        if let Some(tcs) = delta
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
        {
            for tc in tcs {
                let idx = tc
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let idx = usize::try_from(idx).unwrap_or(0);
                let entry = self.tool_calls.entry(idx).or_default();
                if let Some(id) = tc.get("id").and_then(serde_json::Value::as_str) {
                    entry.0 = id.to_string();
                }
                if let Some(name) = tc
                    .pointer("/function/name")
                    .and_then(serde_json::Value::as_str)
                {
                    entry.1 = name.to_string();
                }
                if let Some(args) = tc
                    .pointer("/function/arguments")
                    .and_then(serde_json::Value::as_str)
                {
                    entry.2.push_str(args);
                }
            }
        }

        Ok(())
    }

    fn into_response(self) -> Result<LlmResponse, LlmError> {
        let mut content = Vec::new();

        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }

        for (_, (id, name, arguments)) in self.tool_calls {
            if name.is_empty() {
                return Err(LlmError::server_error(
                    "OpenAI streaming returned tool call with empty function name",
                ));
            }
            let input = serde_json::from_str(&arguments).map_err(|e| {
                LlmError::server_error(format!("Invalid JSON in tool call arguments: {e}"))
            })?;
            content.push(ContentBlock::ToolUse { id, name, input });
        }

        if content.is_empty() {
            return Err(LlmError::server_error(
                "OpenAI streaming returned empty response (no content or tool calls)",
            ));
        }

        let end_turn = self.finish_reason.as_deref() == Some("stop");

        Ok(LlmResponse {
            content,
            end_turn,
            usage: Usage {
                input_tokens: u64::from(self.prompt_tokens),
                output_tokens: u64::from(self.completion_tokens),
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }
}

async fn complete_streaming_chat_api(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_endpoint(spec, gateway);
    let mut openai_request = translate_request(&spec.api_name, request);
    openai_request.stream = Some(true);

    let client = Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER)
        .json(&openai_request)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::network(format!("Request timeout: {e}"))
            } else if e.is_connect() {
                LlmError::network(format!("Connection failed: {e}"))
            } else {
                LlmError::network(format!("Request failed: {e}"))
            }
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::network(format!("Failed to read error response: {e}")))?;
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    let mut acc = ChatStreamAccumulator::new();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        byte_buf.extend_from_slice(&chunk);

        loop {
            let Some(nl_pos) = byte_buf.iter().position(|&b| b == b'\n') else {
                break;
            };
            let line = std::str::from_utf8(&byte_buf[..nl_pos])
                .unwrap_or("")
                .trim_end_matches('\r')
                .to_string();
            byte_buf.drain(..=nl_pos);

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return acc.into_response();
                }
                acc.process_chunk(data, chunk_tx)?;
            }
        }
    }

    // Stream ended without [DONE] — assemble from accumulated state
    acc.into_response()
}

// ---------------------------------------------------------------------------
// Request translation
// ---------------------------------------------------------------------------

/// Translate an `LlmRequest` into an `OpenAIRequest` for chat/completions.
fn translate_request(api_name: &str, request: &LlmRequest) -> OpenAIRequest {
    let mut messages = Vec::new();

    // Add system messages first
    if !request.system.is_empty() {
        let system_text = request
            .system
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        messages.push(OpenAIMessage {
            role: "system".to_string(),
            content: Some(OpenAIContent::Text(system_text)),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    // Add conversation messages
    for msg in &request.messages {
        messages.extend(translate_message(msg));
    }

    // Convert tools
    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(
            request
                .tools
                .iter()
                .map(|t| OpenAITool {
                    r#type: "function".to_string(),
                    function: OpenAIFunction {
                        name: t.name.clone(),
                        description: Some(t.description.clone()),
                        parameters: t.input_schema.clone(),
                    },
                })
                .collect(),
        )
    };

    // O-series / GPT-5 models use max_completion_tokens, others use max_tokens
    let (max_tokens, max_completion_tokens) = if uses_max_completion_tokens(api_name) {
        (None, request.max_tokens)
    } else {
        (request.max_tokens, None)
    };

    OpenAIRequest {
        model: api_name.to_string(),
        messages,
        tools,
        max_tokens,
        max_completion_tokens,
        temperature: None,
        top_p: None,
        n: None,
        stream: Some(false),
        stop: None,
        presence_penalty: None,
        frequency_penalty: None,
        logit_bias: None,
        user: None,
    }
}

/// Translate an LLM message to `OpenAI` format.
/// Returns a Vec because tool results need separate messages with role "tool".
pub(crate) fn translate_message(msg: &LlmMessage) -> Vec<OpenAIMessage> {
    use super::types::ImageSource;

    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

    // Separate content into categories
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            ContentBlock::Image { source } => {
                let ImageSource::Base64 { media_type, data } = source;
                images.push((media_type.clone(), data.clone()));
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(OpenAIToolCall {
                    id: id.clone(),
                    r#type: "function".to_string(),
                    function: OpenAIFunctionCall {
                        name: name.clone(),
                        arguments: serde_json::to_string(input)
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                });
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                images: _,
                is_error,
            } => {
                tool_results.push((tool_use_id.clone(), content.clone(), *is_error));
            }
        }
    }

    let mut messages = Vec::new();

    // Build message based on what content we have
    if !text_parts.is_empty() || !images.is_empty() || !tool_calls.is_empty() {
        let content = if images.is_empty() && !text_parts.is_empty() {
            // Simple text-only message
            Some(OpenAIContent::Text(text_parts.join("\n")))
        } else if !images.is_empty() {
            // Vision message with images (must use parts format)
            let mut parts = Vec::new();

            // Add text parts
            for text in text_parts {
                parts.push(OpenAIContentPart::Text { text });
            }

            // Add images
            for (media_type, data) in images {
                parts.push(OpenAIContentPart::ImageUrl {
                    image_url: OpenAIImageUrl {
                        url: format!("data:{media_type};base64,{data}"),
                    },
                });
            }

            Some(OpenAIContent::Parts(parts))
        } else {
            // No text or images (tool calls only)
            None
        };

        let tool_calls_opt = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        messages.push(OpenAIMessage {
            role: role.to_string(),
            content,
            tool_calls: tool_calls_opt,
            tool_call_id: None,
        });
    }

    // Tool results are separate messages with role "tool"
    for (tool_use_id, content, is_error) in tool_results {
        messages.push(OpenAIMessage {
            role: "tool".to_string(),
            content: Some(OpenAIContent::Text(if is_error {
                format!("Error: {content}")
            } else {
                content
            })),
            tool_calls: None,
            tool_call_id: Some(tool_use_id),
        });
    }

    // Edge case: empty message (shouldn't happen, but handle gracefully)
    if messages.is_empty() {
        messages.push(OpenAIMessage {
            role: role.to_string(),
            content: Some(OpenAIContent::Text(String::new())),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    messages
}

/// Translate `LlmRequest` to `ResponsesApiRequest`.
#[allow(clippy::too_many_lines)] // single-pass message translation; splitting would add indirection without clarity
fn translate_to_responses_request(api_name: &str, request: &LlmRequest) -> ResponsesApiRequest {
    use super::types::ImageSource;

    let mut input_items = Vec::new();

    let instructions = if request.system.is_empty() {
        None
    } else {
        Some(
            request
                .system
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    };

    // Process each message as a unit to allow grouping text + images
    for msg in &request.messages {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        let mut text_blocks: Vec<&str> = vec![];
        let mut image_blocks: Vec<&ImageSource> = vec![];
        let mut tool_calls: Vec<&ContentBlock> = vec![];
        let mut tool_results: Vec<&ContentBlock> = vec![];

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => text_blocks.push(text),
                ContentBlock::Image { source } => image_blocks.push(source),
                ContentBlock::ToolUse { .. } => tool_calls.push(block),
                ContentBlock::ToolResult { .. } => tool_results.push(block),
            }
        }

        // Emit single Message item for text + image content
        if !text_blocks.is_empty() || !image_blocks.is_empty() {
            let content = if image_blocks.is_empty() {
                ResponsesApiMessageContent::Text(text_blocks.join("\n"))
            } else {
                let mut parts: Vec<ResponsesApiContentPart> = text_blocks
                    .iter()
                    .map(|t| ResponsesApiContentPart::InputText {
                        text: (*t).to_string(),
                    })
                    .collect();
                for source in &image_blocks {
                    let ImageSource::Base64 { media_type, data } = source;
                    parts.push(ResponsesApiContentPart::InputImage {
                        image_url: format!("data:{media_type};base64,{data}"),
                    });
                }
                ResponsesApiMessageContent::Parts(parts)
            };
            input_items.push(ResponsesApiInputItem::Message {
                role: role.to_string(),
                content,
            });
        }

        // Emit FunctionCall items
        for block in tool_calls {
            if let ContentBlock::ToolUse { id, name, input } = block {
                input_items.push(ResponsesApiInputItem::FunctionCall {
                    call_id: id.clone(),
                    name: name.clone(),
                    arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                });
            }
        }

        // Emit FunctionCallOutput items with image support
        for block in tool_results {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                images,
                is_error,
            } = block
            {
                let text = if *is_error {
                    format!("Error: {content}")
                } else {
                    content.clone()
                };
                let output = if images.is_empty() {
                    ResponsesApiFunctionOutput::Text(text)
                } else {
                    let mut parts = vec![ResponsesApiOutputPart::Text { text }];
                    for img in images {
                        let ImageSource::Base64 { media_type, data } = img;
                        parts.push(ResponsesApiOutputPart::ImageUrl {
                            image_url: ResponsesApiImageUrl {
                                url: format!("data:{media_type};base64,{data}"),
                            },
                        });
                    }
                    ResponsesApiFunctionOutput::Parts(parts)
                };
                input_items.push(ResponsesApiInputItem::FunctionCallOutput {
                    call_id: tool_use_id.clone(),
                    output,
                });
            }
        }
    }

    let tools: Option<Vec<ResponsesApiTool>> = if request.tools.is_empty() {
        None
    } else {
        Some(
            request
                .tools
                .iter()
                .map(|t| ResponsesApiTool {
                    r#type: "function".to_string(),
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                })
                .collect(),
        )
    };

    ResponsesApiRequest {
        model: api_name.to_string(),
        input: input_items,
        instructions,
        tools,
        max_output_tokens: request.max_tokens,
        stream: None,
    }
}

// ---------------------------------------------------------------------------
// Response normalization
// ---------------------------------------------------------------------------

/// Normalize an `OpenAIResponse` (chat/completions) to `LlmResponse`.
pub(crate) fn normalize_response(resp: OpenAIResponse) -> Result<LlmResponse, LlmError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::server_error("No choices in response"))?;

    let mut content = Vec::new();

    // Add text content if present
    if let Some(msg_content) = choice.message.content {
        match msg_content {
            OpenAIContent::Text(text) => {
                if !text.is_empty() {
                    content.push(ContentBlock::Text { text });
                }
            }
            OpenAIContent::Parts(parts) => {
                for part in parts {
                    if let OpenAIContentPart::Text { text } = part {
                        if !text.is_empty() {
                            content.push(ContentBlock::Text { text });
                        }
                    }
                }
            }
        }
    }

    // Add tool calls if present
    if let Some(tool_calls) = choice.message.tool_calls {
        for tc in tool_calls {
            if tc.function.name.is_empty() {
                return Err(LlmError::server_error(
                    "OpenAI returned tool call with empty function name",
                ));
            }

            let input = serde_json::from_str(&tc.function.arguments).map_err(|e| {
                LlmError::server_error(format!("Invalid JSON in tool call arguments: {e}"))
            })?;

            content.push(ContentBlock::ToolUse {
                id: tc.id,
                name: tc.function.name,
                input,
            });
        }
    }

    if content.is_empty() {
        return Err(LlmError::server_error(
            "OpenAI returned empty response (no content or tool calls)",
        ));
    }

    let end_turn = choice.finish_reason == Some("stop".to_string());

    Ok(LlmResponse {
        content,
        end_turn,
        usage: Usage {
            input_tokens: u64::from(resp.usage.prompt_tokens),
            output_tokens: u64::from(resp.usage.completion_tokens),
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        },
    })
}

/// Normalize `ResponsesApiResponse` to `LlmResponse`.
fn normalize_responses_api_response(resp: ResponsesApiResponse) -> LlmResponse {
    let mut content = Vec::new();

    for output in resp.output {
        match output.r#type.as_str() {
            "message" => {
                if let Some(output_content) = output.content {
                    for item in output_content {
                        if item.r#type == "output_text" {
                            if let Some(text) = item.text {
                                if !text.is_empty() {
                                    content.push(ContentBlock::Text { text });
                                }
                            }
                        }
                    }
                }
            }
            "function_call" => {
                if let (Some(name), Some(arguments), Some(call_id)) =
                    (output.name, output.arguments, output.call_id)
                {
                    let input = serde_json::from_str(&arguments).unwrap_or_else(|e| {
                        tracing::warn!(error = %e, arguments = %arguments, "Failed to parse function call arguments");
                        serde_json::Value::Object(serde_json::Map::new())
                    });
                    content.push(ContentBlock::ToolUse {
                        id: call_id,
                        name,
                        input,
                    });
                }
            }
            "reasoning" => {
                // Skip reasoning outputs — internal model thinking
            }
            other => {
                tracing::debug!(output_type = %other, "Ignoring unknown output type");
            }
        }
    }

    let has_tool_calls = content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
    let end_turn = resp.status == "completed" && !has_tool_calls;

    LlmResponse {
        content,
        end_turn,
        usage: Usage {
            input_tokens: u64::from(resp.usage.input_tokens),
            output_tokens: u64::from(resp.usage.output_tokens),
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        },
    }
}

// ===========================================================================
// OpenAI API types
// ===========================================================================

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<std::collections::HashMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OpenAIContentPart {
    Text { text: String },
    ImageUrl { image_url: OpenAIImageUrl },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIImageUrl {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAITool {
    pub r#type: String,
    pub function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIResponse {
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIChoice {
    pub message: OpenAIMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[allow(dead_code)] // Part of API response, not always used
    pub total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAIErrorResponse {
    error: OpenAIError,
}

#[derive(Debug, Deserialize)]
struct OpenAIError {
    message: String,
    #[allow(dead_code)]
    r#type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}

// Responses API types (for codex models)

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesApiRequest {
    model: String,
    pub(crate) input: Vec<ResponsesApiInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum ResponsesApiInputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: ResponsesApiMessageContent,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        call_id: String,
        output: ResponsesApiFunctionOutput,
    },
}

/// Message content: plain string when text-only, array of parts when images present
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesApiMessageContent {
    Text(String),
    Parts(Vec<ResponsesApiContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesApiContentPart {
    InputText { text: String },
    InputImage { image_url: String }, // "data:{media_type};base64,{data}"
}

/// Function call output: plain string when text-only, array of parts when images present
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesApiFunctionOutput {
    Text(String),
    Parts(Vec<ResponsesApiOutputPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesApiOutputPart {
    Text { text: String },
    ImageUrl { image_url: ResponsesApiImageUrl },
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesApiImageUrl {
    pub(crate) url: String, // "data:{media_type};base64,{data}"
}

#[derive(Debug, Serialize)]
struct ResponsesApiTool {
    r#type: String,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiResponse {
    pub(crate) status: String,
    pub(crate) output: Vec<ResponsesApiOutput>,
    pub(crate) usage: ResponsesApiUsage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiOutput {
    pub(crate) r#type: String,
    #[serde(default)]
    pub(crate) content: Option<Vec<ResponsesApiContent>>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<String>,
    #[serde(default)]
    pub(crate) call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiContent {
    pub(crate) r#type: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiUsage {
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub(crate) fn normalize_response(
        resp: OpenAIResponse,
    ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
        super::normalize_response(resp)
    }

    pub(crate) fn translate_message(msg: &crate::llm::types::LlmMessage) -> Vec<OpenAIMessage> {
        super::translate_message(msg)
    }

    pub fn translate_to_responses_request(
        api_name: &str,
        request: &crate::llm::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request)
    }
}
