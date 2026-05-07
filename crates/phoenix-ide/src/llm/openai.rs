//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::models::{ApiFormat, ModelSpec};
use super::types::{ContentBlock, LlmRequest, LlmResponse, MessageRole, Usage, LLM_SOURCE_HEADER};
use super::LlmError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Determine the full endpoint URL.
fn resolve_endpoint(
    spec: &ModelSpec,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
) -> String {
    let suffix = match spec.api_format {
        ApiFormat::OpenAIResponses => "responses",
        ApiFormat::OpenAIChat => "chat/completions",
        ApiFormat::Anthropic => {
            unreachable!("anthropic requests do not use openai endpoint resolver")
        }
    };

    if let Some(url) = base_url_override {
        return derive_sibling_endpoint(url, suffix);
    }

    match gateway {
        Some(gw) => {
            let provider = spec.gateway_provider_header().unwrap_or("openai");
            format!("{}/{provider}/v1/{suffix}", gw.trim_end_matches('/'))
        }
        None => format!("https://api.openai.com/v1/{suffix}"),
    }
}

fn derive_sibling_endpoint(base_url: &str, suffix: &str) -> String {
    let path = base_url.split('?').next().unwrap_or(base_url);
    let Some((prefix, _)) = path.split_once("/v1/") else {
        return base_url.to_string();
    };
    format!("{prefix}/v1/{suffix}")
}

// ---------------------------------------------------------------------------
// Responses API
// ---------------------------------------------------------------------------

/// Complete using the `OpenAI` Responses API.
#[allow(clippy::too_many_arguments)]
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
    use_codex_backend: bool,
) -> Result<LlmResponse, LlmError> {
    if use_codex_backend {
        let (chunk_tx, _chunk_rx) = tokio::sync::broadcast::channel(1);
        return complete_streaming(
            spec,
            api_key,
            gateway,
            base_url_override,
            custom_headers,
            request_tags,
            request,
            &chunk_tx,
            use_codex_backend,
        )
        .await;
    }

    let url = resolve_endpoint(spec, gateway, base_url_override);
    if spec.api_format == ApiFormat::OpenAIChat {
        return complete_chat_api(spec, api_key, &url, custom_headers, request_tags, request).await;
    }

    let mut responses_request =
        translate_to_responses_request(&spec.api_name, request, use_codex_backend);
    if !request_tags.is_empty() {
        responses_request.tags = Some(request_tags.clone());
    }

    let client = Client::builder()
        .timeout(Duration::from_mins(5))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER);
    for (k, v) in custom_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let response = builder.json(&responses_request).send().await.map_err(|e| {
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
        // Sentinel — not valid JSON, nothing to do.
        if data == "[DONE]" {
            return Ok(());
        }
        // The gateway omits SSE `event:` lines; type is embedded in the JSON.
        // Parse JSON first, then dispatch on data["type"], falling back to event_type.
        let v: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| LlmError::invalid_response(format!("Failed to parse SSE data: {e}")))?;

        let dispatch_type = v
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(event_type);

        tracing::debug!(dispatch_type, "responses_api SSE event");

        match dispatch_type {
            "response.output_text.delta" => {
                if let Some(delta) = v.get("delta").and_then(serde_json::Value::as_str) {
                    if !delta.is_empty() {
                        let _ = chunk_tx.send(super::TokenChunk::Text(delta.to_string()));
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    match serde_json::from_value::<ResponsesApiOutput>(item.clone()) {
                        Ok(output) => {
                            tracing::debug!(
                                output_type = %output.r#type,
                                "responses_api output item collected"
                            );
                            self.output_items.push(output);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                item = %item,
                                "responses_api failed to deserialize output item"
                            );
                        }
                    }
                }
            }
            // OpenAI Responses API terminal event. Task 583 spec incorrectly named
            // this "response.done" — the actual OpenAI spec uses "response.completed".
            "response.completed" => {
                if let Some(usage) = v.pointer("/response/usage") {
                    tracing::debug!(usage = %usage, "responses_api usage extracted");
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
                } else {
                    tracing::warn!(data, "responses_api terminal event had no /response/usage");
                }
                self.done = true;
            }
            _ => {
                tracing::debug!(dispatch_type, "responses_api ignoring event");
            }
        }
        Ok(())
    }

    fn into_response(self) -> LlmResponse {
        tracing::debug!(
            output_items = self.output_items.len(),
            input_tokens = self.input_tokens,
            output_tokens = self.output_tokens,
            "responses_api stream accumulator finalizing"
        );
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

/// Complete with streaming, emitting `TokenChunk::Text` events via `chunk_tx`.
#[allow(clippy::too_many_arguments)]
pub async fn complete_streaming(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    use_codex_backend: bool,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_endpoint(spec, gateway, base_url_override);
    if spec.api_format == ApiFormat::OpenAIChat {
        return complete_streaming_chat_api(
            spec,
            api_key,
            &url,
            custom_headers,
            request_tags,
            request,
            chunk_tx,
        )
        .await;
    }

    let mut responses_request =
        translate_to_responses_request(&spec.api_name, request, use_codex_backend);
    responses_request.stream = Some(true);
    if !request_tags.is_empty() {
        responses_request.tags = Some(request_tags.clone());
    }

    let client = Client::builder()
        .timeout(Duration::from_mins(10))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER);
    for (k, v) in custom_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let response = builder.json(&responses_request).send().await.map_err(|e| {
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
    let mut sse = super::sse::SseParser::new();
    let mut stream = response.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        for event in sse.push(&chunk) {
            if let Err(e) = acc.process_event(&event.event_type, &event.data, chunk_tx) {
                tracing::error!(
                    event_type = %event.event_type,
                    data_len = event.data.len(),
                    "SSE event processing failed; dumping parser diagnostics"
                );
                tracing::error!("{}", sse.diagnostic_dump());
                return Err(e);
            }
            if acc.done {
                break 'outer;
            }
        }
    }

    for event in sse.finish() {
        acc.process_event(&event.event_type, &event.data, chunk_tx)?;
    }

    Ok(acc.into_response())
}

// ---------------------------------------------------------------------------
// Chat Completions API
// ---------------------------------------------------------------------------

async fn complete_chat_api(
    spec: &ModelSpec,
    api_key: &str,
    url: &str,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let mut chat_request = translate_to_chat_request(&spec.api_name, request);
    if !request_tags.is_empty() {
        chat_request.tags = Some(request_tags.clone());
    }

    let client = Client::builder()
        .timeout(Duration::from_mins(5))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("source", LLM_SOURCE_HEADER);
    for (k, v) in custom_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let response = builder.json(&chat_request).send().await.map_err(|e| {
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

    let chat_response: ChatCompletionsResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    normalize_chat_response(chat_response)
}

async fn complete_streaming_chat_api(
    spec: &ModelSpec,
    api_key: &str,
    url: &str,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let mut chat_request = translate_to_chat_request(&spec.api_name, request);
    chat_request.stream = Some(true);
    chat_request.stream_options = Some(ChatStreamOptions {
        include_usage: true,
    });
    if !request_tags.is_empty() {
        chat_request.tags = Some(request_tags.clone());
    }

    let client = Client::builder()
        .timeout(Duration::from_mins(10))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("source", LLM_SOURCE_HEADER);
    for (k, v) in custom_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let response = builder.json(&chat_request).send().await.map_err(|e| {
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

    let mut stream = response.bytes_stream();
    let mut sse = crate::llm::sse::SseParser::new();
    let mut acc = ChatStreamAccumulator::new();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        for event in sse.push(&chunk) {
            acc.process_event(&event.data, chunk_tx)?;
            if acc.done {
                break 'outer;
            }
        }
    }

    for event in sse.finish() {
        acc.process_event(&event.data, chunk_tx)?;
    }

    acc.into_response()
}

#[allow(clippy::too_many_lines)]
fn translate_to_chat_request(api_name: &str, request: &LlmRequest) -> ChatCompletionsRequest {
    use super::types::ImageSource;

    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: Some(ChatContent::Text(
                request
                    .system
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in &request.messages {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };
        let mut text_blocks = Vec::new();
        let mut image_blocks = Vec::new();
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => text_blocks.push(text.as_str()),
                ContentBlock::Image { source } => image_blocks.push(source),
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ChatToolCall {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: ChatFunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(input)
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    });
                }
                ContentBlock::ToolResult { .. } => tool_results.push(block),
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::WebSearchToolResult { .. }
                | ContentBlock::WebFetchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. }
                | ContentBlock::BashCodeExecutionToolResult { .. }
                | ContentBlock::TextEditorCodeExecutionToolResult { .. }
                | ContentBlock::McpToolUse { .. }
                | ContentBlock::McpToolResult { .. } => {
                    tracing::debug!(
                        "Skipping Anthropic server block in chat completions translation"
                    );
                }
            }
        }

        if !text_blocks.is_empty() || !image_blocks.is_empty() || !tool_calls.is_empty() {
            let content = if image_blocks.is_empty() {
                Some(ChatContent::Text(text_blocks.join("\n")))
            } else {
                let mut parts: Vec<ChatContentPart> = text_blocks
                    .iter()
                    .map(|text| ChatContentPart::Text {
                        text: (*text).to_string(),
                    })
                    .collect();
                for source in image_blocks {
                    let ImageSource::Base64 { media_type, data } = source;
                    parts.push(ChatContentPart::ImageUrl {
                        image_url: ChatImageUrl {
                            url: format!("data:{media_type};base64,{data}"),
                        },
                    });
                }
                Some(ChatContent::Parts(parts))
            };

            messages.push(ChatMessage {
                role: role.to_string(),
                content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
            });
        }

        for block in tool_results {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                images,
                is_error,
            } = block
            {
                if !images.is_empty() {
                    tracing::debug!(
                        n = images.len(),
                        "dropping images from chat completions tool result — unsupported by this wire format"
                    );
                }
                messages.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(ChatContent::Text(if *is_error {
                        format!("Error: {content}")
                    } else {
                        content.clone()
                    })),
                    tool_calls: None,
                    tool_call_id: Some(tool_use_id.clone()),
                });
            }
        }
    }

    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(
            request
                .tools
                .iter()
                .map(|tool| ChatTool {
                    r#type: "function".to_string(),
                    function: ChatFunction {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: tool.input_schema.clone(),
                    },
                })
                .collect(),
        )
    };

    ChatCompletionsRequest {
        model: api_name.to_string(),
        messages,
        tools,
        max_tokens: request.max_tokens,
        stream: None,
        stream_options: None,
        tool_choice: if request.tools.is_empty() {
            None
        } else {
            Some("auto".to_string())
        },
        parallel_tool_calls: if request.tools.is_empty() {
            None
        } else {
            Some(true)
        },
        tags: None,
    }
}

fn normalize_chat_response(resp: ChatCompletionsResponse) -> Result<LlmResponse, LlmError> {
    let Some(choice) = resp.choices.into_iter().next() else {
        return Err(LlmError::invalid_response(
            "Chat completions returned no choices".to_string(),
        ));
    };
    chat_message_to_response(choice.message, resp.usage)
}

fn chat_message_to_response(
    message: ChatResponseMessage,
    usage: Option<ChatUsage>,
) -> Result<LlmResponse, LlmError> {
    let mut content = Vec::new();
    if let Some(text) = message.content {
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
    }
    for call in message.tool_calls.unwrap_or_default() {
        let input = serde_json::from_str(&call.function.arguments).unwrap_or_else(|e| {
            tracing::warn!(error = %e, arguments = %call.function.arguments, "Failed to parse chat tool arguments");
            serde_json::Value::Object(serde_json::Map::new())
        });
        content.push(ContentBlock::ToolUse {
            id: call.id,
            name: call.function.name,
            input,
        });
    }
    if content.is_empty() {
        return Err(LlmError::invalid_response(
            "Chat completions returned empty response".to_string(),
        ));
    }
    let has_tool_calls = content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
    let usage = usage.unwrap_or_default();
    Ok(LlmResponse {
        content,
        end_turn: !has_tool_calls,
        usage: Usage {
            input_tokens: u64::from(usage.prompt_tokens),
            output_tokens: u64::from(usage.completion_tokens),
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        },
    })
}

struct ChatStreamAccumulator {
    content: String,
    tool_calls: Vec<ChatToolCallBuilder>,
    usage: Option<ChatUsage>,
    done: bool,
}

impl ChatStreamAccumulator {
    fn new() -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            usage: None,
            done: false,
        }
    }

    fn process_event(
        &mut self,
        data: &str,
        chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    ) -> Result<(), LlmError> {
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let event: ChatStreamChunk = serde_json::from_str(data).map_err(|e| {
            LlmError::invalid_response(format!("Failed to parse chat SSE data: {e}"))
        })?;
        self.usage = event.usage.or(self.usage.take());
        for choice in event.choices {
            if let Some(delta) = choice.delta.content {
                if !delta.is_empty() {
                    self.content.push_str(&delta);
                    let _ = chunk_tx.send(super::TokenChunk::Text(delta));
                }
            }
            for tool_delta in choice.delta.tool_calls.unwrap_or_default() {
                let index = tool_delta.index.unwrap_or(self.tool_calls.len());
                while self.tool_calls.len() <= index {
                    self.tool_calls.push(ChatToolCallBuilder::default());
                }
                let builder = &mut self.tool_calls[index];
                if let Some(id) = tool_delta.id {
                    builder.id = id;
                }
                if let Some(function) = tool_delta.function {
                    if let Some(name) = function.name {
                        builder.name = name;
                    }
                    if let Some(arguments) = function.arguments {
                        builder.arguments.push_str(&arguments);
                    }
                }
            }
        }
        Ok(())
    }

    fn into_response(self) -> Result<LlmResponse, LlmError> {
        let message = ChatResponseMessage {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            tool_calls: if self.tool_calls.is_empty() {
                None
            } else {
                Some(
                    self.tool_calls
                        .into_iter()
                        .map(ChatToolCallBuilder::build)
                        .collect(),
                )
            },
        };
        chat_message_to_response(message, self.usage)
    }
}

#[derive(Default)]
struct ChatToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

impl ChatToolCallBuilder {
    fn build(self) -> ChatToolCall {
        ChatToolCall {
            id: self.id,
            r#type: "function".to_string(),
            function: ChatFunctionCall {
                name: self.name,
                arguments: self.arguments,
            },
        }
    }
}

/// Translate `LlmRequest` to `ResponsesApiRequest`.
///
/// `use_codex_backend` controls two `ChatGPT`-backend-specific tweaks:
/// - `store: false` is sent so the conversation isn't persisted server-side.
/// - When `system` is empty, a default `instructions` value is injected. The
///   `ChatGPT` backend rejects requests without instructions, while the platform
///   Responses API tolerates omission.
#[allow(clippy::too_many_lines)] // single-pass message translation; splitting would add indirection without clarity
fn translate_to_responses_request(
    api_name: &str,
    request: &LlmRequest,
    use_codex_backend: bool,
) -> ResponsesApiRequest {
    use super::types::ImageSource;

    let mut input_items = Vec::new();

    let instructions = if request.system.is_empty() {
        if use_codex_backend {
            Some("You are a helpful assistant.".to_string())
        } else {
            None
        }
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
                // Anthropic-specific server blocks -- no OpenAI equivalent; skip.
                ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::WebSearchToolResult { .. }
                | ContentBlock::WebFetchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. }
                | ContentBlock::BashCodeExecutionToolResult { .. }
                | ContentBlock::TextEditorCodeExecutionToolResult { .. }
                | ContentBlock::McpToolUse { .. }
                | ContentBlock::McpToolResult { .. } => {
                    tracing::debug!(
                        "Skipping Anthropic server block in OpenAI message translation"
                    );
                }
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

    let has_tools = !request.tools.is_empty();
    ResponsesApiRequest {
        model: api_name.to_string(),
        input: input_items,
        instructions,
        tools,
        max_output_tokens: if use_codex_backend {
            None
        } else {
            request.max_tokens
        },
        stream: None,
        store: if use_codex_backend { Some(false) } else { None },
        prompt_cache_key: Some(request.cache_key.as_str().to_string()),
        // Match the explicit defaults Codex CLI and Pi send. `tool_choice`
        // mirrors the server-side default but stabilises the wire shape so
        // non-default strategies become a smaller change later. Omitted when
        // no tools are sent (the API rejects "auto" without a tools array).
        tool_choice: if has_tools {
            Some("auto".to_string())
        } else {
            None
        },
        // `parallel_tool_calls: true` lets the model emit multiple ToolUse
        // blocks in one assistant message. Phoenix's executor runs tools
        // serially (state.rs `ToolExecuting { current_tool, remaining_tools }`),
        // so we don't gain parallelism — but we do save (N-1) LLM round-trips
        // when the model recognises a batch as safely-parallel ("read these
        // three files"). Tradeoff: the model commits to all N tools without
        // seeing intermediate results, so a bad batch wastes the unused
        // calls. Modern models are decent at not batching dependent tools,
        // so on balance the round-trip savings win. Revisit if Phoenix gains
        // a parallel executor (then this becomes a true no-brainer) or if we
        // see the model batching too aggressively in practice.
        parallel_tool_calls: if has_tools { Some(true) } else { None },
        include: Vec::new(),
        tags: None,
    }
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

#[derive(Debug, Serialize)]
struct ChatCompletionsRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<ChatStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Serialize)]
struct ChatStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<ChatContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ChatContent {
    Text(String),
    Parts(Vec<ChatContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ChatContentPart {
    Text { text: String },
    ImageUrl { image_url: ChatImageUrl },
}

#[derive(Debug, Serialize)]
struct ChatImageUrl {
    url: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatToolCall {
    id: String,
    r#type: String,
    function: ChatFunctionCall,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    r#type: String,
    function: ChatFunction,
}

#[derive(Debug, Serialize)]
struct ChatFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsResponse {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamChunk {
    #[serde(default)]
    choices: Vec<ChatStreamChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamChoice {
    delta: ChatDelta,
    #[serde(default)]
    _finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionCallDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesApiRequest {
    model: String,
    pub(crate) input: Vec<ResponsesApiInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    /// `store: false` opts out of `OpenAI`'s server-side conversation persistence.
    /// Required for the `ChatGPT`-backend codex bridge; harmless on platform.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) store: Option<bool>,
    /// Stable identifier for the prompt-prefix cache. Set on every request:
    /// the field is `Option` only because the wire protocol allows omission,
    /// but the typed `LlmRequest` requires the caller to pick a key (see
    /// `PromptCacheKey`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_cache_key: Option<String>,
    /// Tool selection strategy. `"auto"` is the server-side default; sent
    /// explicitly to stabilise the wire shape and to make non-default
    /// strategies a smaller change later. Omitted when no tools are sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<String>,
    /// Allow the model to emit multiple tool calls in one response. The
    /// server-side default is `true`; sent explicitly to stabilise wire
    /// shape. Omitted when no tools are sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parallel_tool_calls: Option<bool>,
    /// Output items the server should include in the response. Keep empty
    /// until Phoenix has a durable transcript representation for additional
    /// output item types such as encrypted reasoning.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) include: Vec<String>,
    /// Free-form metadata forwarded to the gateway/proxy in front of the
    /// model. See `AnthropicRequest::tags` for the rationale; same shape on
    /// both wire formats. Set only when a gateway is configured; omitted
    /// from the wire when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tags: Option<BTreeMap<String, String>>,
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
mod tests {
    use super::*;
    use crate::llm::models::{ApiFormat, AuthFamily, GatewayRoute, ModelFamily, ModelSpec};
    use crate::llm::types::{LlmMessage, LlmRequest, PromptCacheKey, ToolDefinition};

    fn empty_request() -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            cache_key: PromptCacheKey::stable("test"),
        }
    }

    fn gemini_spec() -> ModelSpec {
        ModelSpec {
            id: "gemini-2.5-flash".into(),
            api_name: "google/gemini-2.5-flash".into(),
            family: ModelFamily::Google,
            auth_family: AuthFamily::Gateway,
            gateway_route: Some(GatewayRoute::new("google")),
            api_format: ApiFormat::OpenAIChat,
            description: "test".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: false,
        }
    }

    #[test]
    fn chat_endpoint_derived_from_responses_base_url() {
        assert_eq!(
            resolve_endpoint(
                &gemini_spec(),
                None,
                Some("https://gateway.example.test/v1/responses"),
            ),
            "https://gateway.example.test/v1/chat/completions"
        );
    }

    #[test]
    fn chat_endpoint_uses_gateway_provider_alias() {
        assert_eq!(
            resolve_endpoint(&gemini_spec(), Some("http://gateway/llm"), None),
            "http://gateway/llm/google/v1/chat/completions"
        );
    }

    #[test]
    fn chat_request_translates_tools_and_tool_results() {
        let request = LlmRequest {
            system: vec![],
            messages: vec![
                LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::Text { text: "hi".into() }],
                },
                LlmMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        input: serde_json::json!({"path":"x"}),
                    }],
                },
                LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "call_1".into(),
                        content: "contents".into(),
                        images: vec![],
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "read".into(),
                input_schema: serde_json::json!({"type":"object"}),
                defer_loading: false,
            }],
            max_tokens: Some(10),
            cache_key: PromptCacheKey::stable("test"),
        };
        let wire = serde_json::to_value(translate_to_chat_request(
            "google/gemini-2.5-flash",
            &request,
        ))
        .unwrap();
        assert_eq!(wire["model"], "google/gemini-2.5-flash");
        assert_eq!(
            wire["messages"][1]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(wire["messages"][2]["role"], "tool");
        assert_eq!(wire["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(wire["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn chat_streaming_requests_usage_chunk() {
        let mut request = translate_to_chat_request("google/gemini-2.5-flash", &empty_request());
        request.stream = Some(true);
        request.stream_options = Some(ChatStreamOptions {
            include_usage: true,
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["stream"], true);
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[test]
    fn test_request_tags_omitted_when_none() {
        let req = translate_to_responses_request("gpt-5.5", &empty_request(), false);
        let json = serde_json::to_value(&req).unwrap();
        assert!(
            json.get("tags").is_none(),
            "tags must be omitted from the wire when not set; got {json}"
        );
    }

    #[test]
    fn test_request_tags_serialized_when_set() {
        let mut req = translate_to_responses_request("gpt-5.5", &empty_request(), false);
        let mut tags = BTreeMap::new();
        tags.insert("disable_data_logging".to_string(), "true".to_string());
        tags.insert("foo".to_string(), "bar".to_string());
        req.tags = Some(tags);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["tags"]["disable_data_logging"], "true");
        assert_eq!(json["tags"]["foo"], "bar");
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub fn translate_to_responses_request(
        api_name: &str,
        request: &crate::llm::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, false)
    }

    pub fn translate_to_responses_request_codex(
        api_name: &str,
        request: &crate::llm::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, true)
    }
}
