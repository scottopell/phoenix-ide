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

        match block_type {
            "text" => {
                self.current_index = Some(idx);
                self.current_is_text = true;
                self.current_text.clear();
            }
            "tool_use" => {
                self.current_index = Some(idx);
                self.current_is_text = false;
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
            "server_tool_use" => {
                // Server-side execution -- log but don't accumulate.
                let name = v
                    .pointer("/content_block/name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                tracing::debug!(name, "Streaming: server_tool_use block (skipping)");
            }
            other => {
                // tool_search_tool_result, web_search_tool_result, etc.
                tracing::debug!(
                    block_type = other,
                    "Streaming: skipping server-handled block"
                );
            }
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

/// Resolve the Anthropic endpoint URL with priority:
/// 1. `base_url_override` (`ANTHROPIC_BASE_URL`) — used as-is, no path appended
/// 2. `gateway` (`LLM_GATEWAY`) — appends `/anthropic/v1/messages`
/// 3. Default: `https://api.anthropic.com/v1/messages`
fn resolve_anthropic_url(gateway: Option<&str>, base_url_override: Option<&str>) -> String {
    if let Some(url) = base_url_override {
        url.to_string()
    } else {
        match gateway {
            Some(gw) => format!("{}/anthropic/v1/messages", gw.trim_end_matches('/')),
            None => "https://api.anthropic.com/v1/messages".to_string(),
        }
    }
}

/// Complete using Anthropic Messages API with streaming.
///
/// Emits `TokenChunk::Text` events via `chunk_tx` as text tokens arrive,
/// then returns the fully assembled `LlmResponse`.
pub async fn complete_streaming(
    spec: &ModelSpec,
    auth: &super::ResolvedAuth,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let base_url = resolve_anthropic_url(gateway, base_url_override);
    let client = Client::builder()
        .timeout(Duration::from_mins(10))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut anthropic_request = translate_request(spec, request);
    anthropic_request.stream = Some(true);

    let has_deferred =
        spec.supports_tool_search && request.tools.iter().any(|t| t.defer_loading);

    let mut builder = client.post(&base_url);
    builder = match auth.style {
        super::AuthStyle::ApiKey => builder.header("x-api-key", &auth.credential),
        super::AuthStyle::Bearer => builder
            .header("Authorization", format!("Bearer {}", auth.credential))
            .header("anthropic-beta", "oauth-2025-04-20"),
        super::AuthStyle::PlainBearer => {
            builder.header("Authorization", format!("Bearer {}", auth.credential))
        }
    };
    // Tool search requires the advanced-tool-use beta header.
    if has_deferred {
        builder = builder.header("anthropic-beta", "advanced-tool-use-2025-11-20");
    }
    builder = builder
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .header("source", LLM_SOURCE_HEADER);
    for (k, v) in custom_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let response = builder.json(&anthropic_request).send().await.map_err(|e| {
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
    auth: &super::ResolvedAuth,
    gateway: Option<&str>,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let base_url = resolve_anthropic_url(gateway, base_url_override);

    let client = Client::builder()
        .timeout(Duration::from_mins(5))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let anthropic_request = translate_request(spec, request);

    let has_deferred =
        spec.supports_tool_search && request.tools.iter().any(|t| t.defer_loading);

    let mut builder = client.post(&base_url);
    builder = match auth.style {
        super::AuthStyle::ApiKey => builder.header("x-api-key", &auth.credential),
        super::AuthStyle::Bearer => builder
            .header("Authorization", format!("Bearer {}", auth.credential))
            .header("anthropic-beta", "oauth-2025-04-20"),
        super::AuthStyle::PlainBearer => {
            builder.header("Authorization", format!("Bearer {}", auth.credential))
        }
    };
    if has_deferred {
        builder = builder.header("anthropic-beta", "advanced-tool-use-2025-11-20");
    }
    builder = builder
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .header("source", LLM_SOURCE_HEADER);
    for (k, v) in custom_headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let response = builder.json(&anthropic_request).send().await.map_err(|e| {
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

fn translate_request(spec: &super::ModelSpec, request: &LlmRequest) -> AnthropicRequest {
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

    let has_deferred = spec.supports_tool_search && request.tools.iter().any(|t| t.defer_loading);

    let mut tools: Vec<AnthropicToolEntry> = request
        .tools
        .iter()
        .map(|t| {
            AnthropicToolEntry::Function(AnthropicFunctionTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
                defer_loading: if has_deferred { t.defer_loading } else { false },
            })
        })
        .collect();

    // Inject tool search tool when deferred tools exist
    if has_deferred {
        let mut variant_map = std::collections::HashMap::new();
        variant_map.insert(TOOL_SEARCH_VARIANT.to_string(), serde_json::json!({}));
        tools.push(AnthropicToolEntry::ToolSearch(AnthropicToolSearchTool {
            r#type: "tool_search".to_string(),
            name: "tool_search".to_string(),
            variant: variant_map,
        }));
    }

    AnthropicRequest {
        model: spec.api_name.clone(),
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
    let mut server_handled_count: usize = 0;

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
            AnthropicContentBlock::ServerToolUse { name, .. } => {
                // Server-side tool execution (tool search, web search, etc.).
                // These blocks are resolved by the API before we see them.
                //
                // OPEN QUESTION: Do these need to be preserved in conversation
                // history for multi-turn? If the API requires them to re-expand
                // deferred tool definitions on subsequent turns, stripping them
                // would break multi-turn tool search. Needs empirical testing.
                // See YF616 Step 2.
                tracing::debug!(name = %name, "Server tool use in response (handled by API)");
                server_handled_count += 1;
            }
            AnthropicContentBlock::Unknown => {
                tracing::debug!("Skipping unknown content block type in response");
                server_handled_count += 1;
            }
        }
    }

    let end_turn = resp.stop_reason.as_deref() == Some("end_turn");

    // Empty content with end_turn is valid and documented Anthropic behavior:
    // the model completed a tool call loop with nothing further to say.
    // Let it through as content: [] — the state machine handles this by
    // transitioning to Idle without persisting an empty agent message.
    //
    // Empty content WITHOUT end_turn is genuinely unexpected.
    if content.is_empty() && !end_turn {
        tracing::warn!(
            stop_reason = ?resp.stop_reason,
            output_tokens = resp.usage.output_tokens,
            raw_block_count = raw_block_count,
            server_handled_count = server_handled_count,
            "Anthropic returned empty content without end_turn"
        );
        return Err(LlmError::invalid_response(format!(
            "Anthropic returned empty response (no content or tool calls, stop_reason={:?}, output_tokens={}, raw_blocks={}, server_handled={})",
            resp.stop_reason, resp.usage.output_tokens, raw_block_count, server_handled_count
        )));
    }

    if content.is_empty() {
        tracing::debug!(
            stop_reason = ?resp.stop_reason,
            output_tokens = resp.usage.output_tokens,
            raw_block_count = raw_block_count,
            server_handled_count = server_handled_count,
            "Anthropic end_turn with empty content — model has nothing to say"
        );
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
    tools: Option<Vec<AnthropicToolEntry>>,
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
    /// Server-side tool invocation (tool search, web search, code execution).
    /// Handled by Anthropic -- client skips these.
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Catch-all for block types we don't recognize (`tool_search_tool_result`,
    /// `web_search_tool_result`, etc.). Prevents deserialization failures when
    /// Anthropic adds new block types.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AnthropicImageSource {
    pub(crate) r#type: String,
    pub(crate) media_type: String,
    pub(crate) data: String,
}

const TOOL_SEARCH_VARIANT: &str = "tool_search_tool_regex_20251119";

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicToolEntry {
    Function(AnthropicFunctionTool),
    ToolSearch(AnthropicToolSearchTool),
}

#[derive(Debug, Serialize)]
struct AnthropicFunctionTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    defer_loading: bool,
}

#[derive(Debug, Serialize)]
struct AnthropicToolSearchTool {
    r#type: String,
    name: String,
    #[serde(flatten)]
    variant: std::collections::HashMap<String, serde_json::Value>,
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
mod tests {
    use super::*;
    use crate::llm::models::{ApiFormat, ModelSpec, Provider};
    use crate::llm::types::{LlmRequest, ToolDefinition};

    fn test_spec(supports_tool_search: bool) -> ModelSpec {
        ModelSpec {
            id: "test-model".into(),
            api_name: "test-model-api".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "test".into(),
            context_window: 200_000,
            recommended: false,
            supports_tool_search,
        }
    }

    fn test_request_with_tools() -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![
                ToolDefinition {
                    name: "bash".into(),
                    description: "Run a bash command".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    defer_loading: false,
                },
                ToolDefinition {
                    name: "mcp_tool".into(),
                    description: "An MCP tool".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    defer_loading: true,
                },
            ],
            max_tokens: None,
        }
    }

    #[test]
    fn test_tool_search_enabled_serialization() {
        let spec = test_spec(true);
        let request = test_request_with_tools();
        let anthropic_req = translate_request(&spec, &request);

        assert_eq!(anthropic_req.model, "test-model-api");

        let json = serde_json::to_value(&anthropic_req).unwrap();
        let tools = json["tools"].as_array().unwrap();

        // 2 function tools + 1 tool_search entry
        assert_eq!(tools.len(), 3);

        // First tool: defer_loading=false -> field omitted
        assert_eq!(tools[0]["name"], "bash");
        assert!(
            tools[0].get("defer_loading").is_none(),
            "defer_loading should be omitted when false"
        );

        // Second tool: defer_loading=true -> field present
        assert_eq!(tools[1]["name"], "mcp_tool");
        assert_eq!(tools[1]["defer_loading"], true);

        // Third entry: tool_search
        assert_eq!(tools[2]["type"], "tool_search");
        assert_eq!(tools[2]["name"], "tool_search");
        assert!(
            tools[2].get(TOOL_SEARCH_VARIANT).is_some(),
            "tool_search entry must contain the variant key"
        );
    }

    #[test]
    fn test_tool_search_disabled_serialization() {
        let spec = test_spec(false);
        let request = test_request_with_tools();
        let anthropic_req = translate_request(&spec, &request);

        let json = serde_json::to_value(&anthropic_req).unwrap();
        let tools = json["tools"].as_array().unwrap();

        // No tool_search entry injected
        assert_eq!(tools.len(), 2);

        // Neither tool has defer_loading in JSON
        for tool in tools {
            assert!(
                tool.get("defer_loading").is_none(),
                "defer_loading should be omitted when tool search is disabled: {}",
                tool
            );
        }

        // No tool_search type present
        assert!(
            !tools
                .iter()
                .any(|t| t.get("type").and_then(|v| v.as_str()) == Some("tool_search")),
            "tool_search entry should not be present when supports_tool_search is false"
        );
    }

    #[test]
    fn test_resolve_anthropic_url_override_takes_priority() {
        let url = resolve_anthropic_url(
            Some("http://gateway.local"),
            Some("https://ai-gateway.us1.ddbuild.io/v1/messages"),
        );
        assert_eq!(url, "https://ai-gateway.us1.ddbuild.io/v1/messages");
    }

    #[test]
    fn test_resolve_anthropic_url_gateway_fallback() {
        let url = resolve_anthropic_url(Some("http://gateway.local"), None);
        assert_eq!(url, "http://gateway.local/anthropic/v1/messages");
    }

    #[test]
    fn test_resolve_anthropic_url_default() {
        let url = resolve_anthropic_url(None, None);
        assert_eq!(url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_resolve_anthropic_url_trailing_slash_stripped() {
        let url = resolve_anthropic_url(Some("http://gateway.local/"), None);
        assert_eq!(url, "http://gateway.local/anthropic/v1/messages");
    }

    #[test]
    fn test_normalize_response_with_server_tool_use() {
        let resp = AnthropicResponse {
            content: vec![
                AnthropicContentBlock::Text {
                    text: "Here is my analysis.".to_string(),
                },
                AnthropicContentBlock::ServerToolUse {
                    id: "srvtoolu_abc123".to_string(),
                    name: "tool_search".to_string(),
                    input: serde_json::json!({"query": "bash"}),
                },
                AnthropicContentBlock::ToolUse {
                    id: "toolu_xyz789".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };

        let result = normalize_response(resp).unwrap();

        // ServerToolUse is skipped -- only Text and ToolUse come through
        assert_eq!(result.content.len(), 2);
        assert!(
            matches!(&result.content[0], ContentBlock::Text { text } if text == "Here is my analysis.")
        );
        assert!(
            matches!(&result.content[1], ContentBlock::ToolUse { id, name, .. } if id == "toolu_xyz789" && name == "bash")
        );
    }

    #[test]
    fn test_normalize_response_unknown_blocks() {
        // Deserialize a block type we don't recognize -- should become Unknown
        let json =
            r#"{"type": "tool_search_tool_result", "tool_use_id": "srvtoolu_123", "content": {}}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();
        assert!(matches!(block, AnthropicContentBlock::Unknown));

        // Also verify web_search_tool_result falls through
        let json2 =
            r#"{"type": "web_search_tool_result", "tool_use_id": "srvtoolu_456", "content": {}}"#;
        let block2: AnthropicContentBlock = serde_json::from_str(json2).unwrap();
        assert!(matches!(block2, AnthropicContentBlock::Unknown));
    }

    #[test]
    fn test_normalize_response_only_server_blocks_with_end_turn() {
        let resp = AnthropicResponse {
            content: vec![AnthropicContentBlock::ServerToolUse {
                id: "srvtoolu_abc".to_string(),
                name: "tool_search".to_string(),
                input: serde_json::json!({}),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 100,
                output_tokens: 10,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };

        let result = normalize_response(resp).unwrap();

        // All blocks were server-handled, content should be empty, and this is OK
        // because stop_reason is end_turn
        assert!(result.content.is_empty());
        assert!(result.end_turn);
    }
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
