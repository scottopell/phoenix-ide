//! Anthropic Claude provider implementation

use super::models::ModelSpec;
use super::types::{
    ContentBlock, ImageSource, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage,
    LLM_SOURCE_HEADER,
};
use super::LlmError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Accumulates state across Anthropic SSE stream events to assemble the final response.
struct StreamAccumulator {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    stop_reason: Option<String>,
    content_blocks: Vec<(usize, AnthropicContentBlock)>,
    // Current block being parsed
    current_index: Option<usize>,
    current_is_text: bool,
    current_text: String,
    current_tool_id: String,
    current_tool_name: String,
    current_tool_json: String,
    /// Set true when `message_stop` is received — signals outer loop to stop
    pub done: bool,
}

impl StreamAccumulator {
    fn new() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            stop_reason: None,
            content_blocks: Vec::new(),
            current_index: None,
            current_is_text: false,
            current_text: String::new(),
            current_tool_id: String::new(),
            current_tool_name: String::new(),
            current_tool_json: String::new(),
            done: false,
        }
    }

    /// Process one complete SSE event (`event_type` + JSON data).
    fn process_event(
        &mut self,
        event_type: &str,
        data: &str,
        chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    ) -> Result<(), LlmError> {
        let v: serde_json::Value = serde_json::from_str(data).map_err(|e| {
            LlmError::invalid_response(format!("Failed to parse SSE data: {e} - data: {data}"))
        })?;
        match event_type {
            "message_start" => {
                self.input_tokens = v
                    .pointer("/message/usage/input_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                self.cache_creation_tokens = v
                    .pointer("/message/usage/cache_creation_input_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                self.cache_read_tokens = v
                    .pointer("/message/usage/cache_read_input_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
            }
            "content_block_start" => self.on_block_start(&v),
            "content_block_delta" => self.on_block_delta(&v, chunk_tx),
            "content_block_stop" => self.on_block_stop(),
            "message_delta" => {
                if let Some(sr) = v
                    .pointer("/delta/stop_reason")
                    .and_then(serde_json::Value::as_str)
                {
                    self.stop_reason = Some(sr.to_string());
                }
                self.output_tokens = v
                    .pointer("/usage/output_tokens")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(self.output_tokens);
            }
            "message_stop" => self.done = true,
            _ => {} // "ping" and unknown events ignored
        }
        Ok(())
    }

    fn on_block_start(&mut self, v: &serde_json::Value) {
        let idx = usize::try_from(
            v.get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0);
        let block_type = v
            .pointer("/content_block/type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text");
        self.current_index = Some(idx);
        self.current_is_text = block_type == "text";
        if self.current_is_text {
            self.current_text.clear();
        } else {
            self.current_tool_id = v
                .pointer("/content_block/id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            self.current_tool_name = v
                .pointer("/content_block/name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            self.current_tool_json.clear();
        }
    }

    fn on_block_delta(
        &mut self,
        v: &serde_json::Value,
        chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    ) {
        let delta_type = v
            .pointer("/delta/type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        match delta_type {
            "text_delta" => {
                if let Some(text) = v.pointer("/delta/text").and_then(serde_json::Value::as_str) {
                    self.current_text.push_str(text);
                    // Forward token to UI — failures are fine (ephemeral, no subscribers)
                    let _ = chunk_tx.send(super::TokenChunk::Text(text.to_string()));
                }
            }
            "input_json_delta" => {
                if let Some(partial) = v
                    .pointer("/delta/partial_json")
                    .and_then(serde_json::Value::as_str)
                {
                    self.current_tool_json.push_str(partial);
                }
            }
            _ => {}
        }
    }

    fn on_block_stop(&mut self) {
        self.flush_current_block();
    }

    /// Commit whatever block is currently being accumulated.
    /// Called by `on_block_stop` during normal flow, and by `into_response`
    /// as a safety net for truncated streams.
    fn flush_current_block(&mut self) {
        let Some(idx) = self.current_index.take() else {
            return;
        };
        if self.current_is_text {
            if !self.current_text.is_empty() {
                self.content_blocks.push((
                    idx,
                    AnthropicContentBlock::Text {
                        text: self.current_text.clone(),
                    },
                ));
                self.current_text.clear();
            }
        } else if !self.current_tool_name.is_empty() {
            let input: serde_json::Value = serde_json::from_str(&self.current_tool_json)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            self.content_blocks.push((
                idx,
                AnthropicContentBlock::ToolUse {
                    id: self.current_tool_id.clone(),
                    name: self.current_tool_name.clone(),
                    input,
                },
            ));
            self.current_tool_json.clear();
            self.current_tool_name.clear();
            self.current_tool_id.clear();
        }
    }

    fn into_response(mut self) -> Result<LlmResponse, LlmError> {
        // Flush any in-progress block that never received content_block_stop.
        // This happens when the stream is truncated (e.g. stop_reason=max_tokens):
        // Anthropic sends message_delta + message_stop but skips content_block_stop
        // for the block being generated, so the text/tool_use sits uncommitted.
        self.flush_current_block();
        self.content_blocks.sort_by_key(|(idx, _)| *idx);
        normalize_response(AnthropicResponse {
            content: self.content_blocks.into_iter().map(|(_, b)| b).collect(),
            stop_reason: self.stop_reason,
            usage: AnthropicUsage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                cache_creation_input_tokens: Some(self.cache_creation_tokens),
                cache_read_input_tokens: Some(self.cache_read_tokens),
            },
        })
    }
}

/// Complete using Anthropic Messages API with streaming.
///
/// Emits `TokenChunk::Text` events via `chunk_tx` as text tokens arrive,
/// then returns the fully assembled `LlmResponse`.
pub async fn complete_streaming(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let base_url = match gateway {
        Some(gw) => format!("{}/anthropic/v1/messages", gw.trim_end_matches('/')),
        None => "https://api.anthropic.com/v1/messages".to_string(),
    };
    let client = Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut anthropic_request = translate_request(&spec.api_name, request);
    anthropic_request.stream = Some(true);

    let response = client
        .post(&base_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .header("source", LLM_SOURCE_HEADER)
        .json(&anthropic_request)
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

    let mut acc = StreamAccumulator::new();
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

    // Flush any trailing event (lenient: some gateways omit final blank line)
    for event in sse.finish() {
        acc.process_event(&event.event_type, &event.data, chunk_tx)?;
    }

    acc.into_response()
}

/// Complete using Anthropic Messages API
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let base_url = match gateway {
        Some(gw) => format!("{}/anthropic/v1/messages", gw.trim_end_matches('/')),
        None => "https://api.anthropic.com/v1/messages".to_string(),
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let anthropic_request = translate_request(&spec.api_name, request);

    let response = client
        .post(&base_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .header("source", LLM_SOURCE_HEADER)
        .json(&anthropic_request)
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
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    let anthropic_response: AnthropicResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    normalize_response(anthropic_response)
}

fn translate_request(model_api_name: &str, request: &LlmRequest) -> AnthropicRequest {
    let system: Vec<AnthropicSystemBlock> = request
        .system
        .iter()
        .map(|s| AnthropicSystemBlock {
            r#type: "text".to_string(),
            text: s.text.clone(),
            cache_control: if s.cache {
                Some(CacheControl {
                    r#type: "ephemeral".to_string(),
                })
            } else {
                None
            },
        })
        .collect();

    let messages: Vec<AnthropicMessage> = request.messages.iter().map(translate_message).collect();

    let tools: Vec<AnthropicTool> = request
        .tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect();

    AnthropicRequest {
        model: model_api_name.to_string(),
        max_tokens: request.max_tokens.unwrap_or(16_384),
        system,
        messages,
        tools: if tools.is_empty() { None } else { Some(tools) },
        stream: None,
    }
}

pub(crate) fn translate_message(msg: &LlmMessage) -> AnthropicMessage {
    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

    let content: Vec<AnthropicContentBlock> = msg
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => AnthropicContentBlock::Text { text: text.clone() },
            ContentBlock::Image { source } => {
                let ImageSource::Base64 { media_type, data } = source;
                AnthropicContentBlock::Image {
                    source: AnthropicImageSource {
                        r#type: "base64".to_string(),
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                }
            }
            ContentBlock::ToolUse { id, name, input } => AnthropicContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            },
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                images,
                is_error,
            } => {
                let wire_content = if images.is_empty() {
                    serde_json::Value::String(content.clone())
                } else {
                    let mut blocks = vec![serde_json::json!({"type": "text", "text": content})];
                    for img in images {
                        let ImageSource::Base64 { media_type, data } = img;
                        blocks.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data
                            }
                        }));
                    }
                    serde_json::Value::Array(blocks)
                };
                AnthropicContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: wire_content,
                    is_error: *is_error,
                }
            }
        })
        .collect();

    AnthropicMessage {
        role: role.to_string(),
        content,
    }
}

pub(crate) fn normalize_response(resp: AnthropicResponse) -> Result<LlmResponse, LlmError> {
    let mut content = Vec::new();
    let raw_block_count = resp.content.len();

    for block in resp.content {
        match block {
            AnthropicContentBlock::Text { text } => {
                if !text.is_empty() {
                    content.push(ContentBlock::Text { text });
                }
            }
            AnthropicContentBlock::ToolUse { id, name, input } => {
                content.push(ContentBlock::ToolUse { id, name, input });
            }
            AnthropicContentBlock::Image { .. } => {
                return Err(LlmError::invalid_response(
                    "Unexpected image block in Anthropic response",
                ));
            }
            AnthropicContentBlock::ToolResult { .. } => {
                return Err(LlmError::invalid_response(
                    "Unexpected tool_result block in Anthropic response",
                ));
            }
        }
    }

    let end_turn = resp.stop_reason.as_deref() == Some("end_turn");

    if content.is_empty() {
        if end_turn {
            // Valid: model completed the tool call loop with nothing further to say.
            // Common with concise models (e.g. haiku) after a simple tool result.
            // Emit an empty text block so the SM receives a well-formed response
            // and transitions to idle normally.
            tracing::debug!(
                stop_reason = ?resp.stop_reason,
                output_tokens = resp.usage.output_tokens,
                raw_block_count = raw_block_count,
                "Anthropic end_turn with empty content — treating as successful completion"
            );
            content.push(ContentBlock::Text {
                text: String::new(),
            });
        } else {
            tracing::warn!(
                stop_reason = ?resp.stop_reason,
                output_tokens = resp.usage.output_tokens,
                raw_block_count = raw_block_count,
                "Anthropic returned empty content after normalization"
            );
            return Err(LlmError::invalid_response(format!(
                "Anthropic returned empty response (no content or tool calls, stop_reason={:?}, output_tokens={}, raw_blocks={})",
                resp.stop_reason, resp.usage.output_tokens, raw_block_count
            )));
        }
    }

    Ok(LlmResponse {
        content,
        end_turn,
        usage: Usage {
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
            cache_creation_tokens: resp.usage.cache_creation_input_tokens.unwrap_or(0),
            cache_read_tokens: resp.usage.cache_read_input_tokens.unwrap_or(0),
        },
    })
}

// Anthropic API types

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: Vec<AnthropicSystemBlock>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct AnthropicSystemBlock {
    r#type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct CacheControl {
    r#type: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AnthropicMessage {
    pub(crate) role: String,
    pub(crate) content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        /// String for text-only results; array of content blocks when images are present.
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AnthropicImageSource {
    pub(crate) r#type: String,
    pub(crate) media_type: String,
    pub(crate) data: String,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicResponse {
    pub(crate) content: Vec<AnthropicContentBlock>,
    pub(crate) stop_reason: Option<String>,
    pub(crate) usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // matches Anthropic API
pub(crate) struct AnthropicUsage {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_input_tokens: Option<u64>,
    pub(crate) cache_read_input_tokens: Option<u64>,
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub(crate) fn normalize_response(
        resp: AnthropicResponse,
    ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
        super::normalize_response(resp)
    }

    pub(crate) fn translate_message(msg: &crate::llm::types::LlmMessage) -> AnthropicMessage {
        super::translate_message(msg)
    }
}
