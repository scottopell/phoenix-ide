//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::models::ModelSpec;
use super::types::{ContentBlock, LlmRequest, LlmResponse, MessageRole, Usage, LLM_SOURCE_HEADER};
use super::LlmError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Determine the full endpoint URL.
/// Priority: `base_url_override` (used as-is) > `gateway` > provider default.
fn resolve_endpoint(gateway: Option<&str>, base_url_override: Option<&str>) -> String {
    if let Some(url) = base_url_override {
        return url.to_string();
    }

    match gateway {
        Some(gw) => format!("{}/openai/v1/responses", gw.trim_end_matches('/')),
        None => "https://api.openai.com/v1/responses".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Responses API
// ---------------------------------------------------------------------------

/// Complete using the `OpenAI` Responses API.
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_endpoint(gateway, base_url_override);
    let responses_request = translate_to_responses_request(&spec.api_name, request);

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
pub async fn complete_streaming(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_endpoint(gateway, base_url_override);
    let mut responses_request = translate_to_responses_request(&spec.api_name, request);
    responses_request.stream = Some(true);

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

    pub fn translate_to_responses_request(
        api_name: &str,
        request: &crate::llm::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request)
    }
}
