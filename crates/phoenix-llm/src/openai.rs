//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::headers::apply_source_header;
use super::models::ModelSpec;
use super::rate_limit::{
    parse_active_limit, parse_credits_snapshot, parse_promo_message, parse_rate_limit_for_limit,
    QuotaDetails,
};
use super::types::{ContentBlock, LlmRequest, LlmResponse, MessageRole, Usage};
use super::LlmError;
use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Determine the full endpoint URL for the Responses API.
/// Priority: `base_url_override` (used as-is) > provider default.
fn resolve_endpoint(base_url_override: Option<&str>) -> String {
    base_url_override.map_or_else(
        || "https://api.openai.com/v1/responses".to_string(),
        std::string::ToString::to_string,
    )
}

/// Determine the full endpoint URL for the Chat Completions API.
/// Priority: `base_url_override` (used as-is) > provider default.
fn resolve_chat_endpoint(base_url_override: Option<&str>) -> String {
    base_url_override.map_or_else(
        || "https://api.openai.com/v1/chat/completions".to_string(),
        std::string::ToString::to_string,
    )
}

fn raw_chat_debug_enabled() -> bool {
    std::env::var("PHOENIX_LLM_DEBUG_RAW_CHAT")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

fn raw_chat_debug_path() -> String {
    std::env::var("PHOENIX_LLM_DEBUG_RAW_CHAT_PATH")
        .unwrap_or_else(|_| "phoenix-llm-chat-raw.jsonl".to_string())
}

fn maybe_log_raw_chat_response(model: &str, kind: &str, body: &str) {
    if !raw_chat_debug_enabled() {
        return;
    }
    let path = raw_chat_debug_path();
    let record = serde_json::json!({
        "ts": Utc::now().to_rfc3339(),
        "model": model,
        "kind": kind,
        "body": body,
    });
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{record}"))
    {
        Ok(()) => {}
        Err(error) => tracing::warn!(%error, path, "failed to write raw chat debug response"),
    }
}

// ---------------------------------------------------------------------------
// Responses API
// ---------------------------------------------------------------------------

/// Complete using the `OpenAI` Responses API.
#[allow(clippy::too_many_arguments)]
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
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
            base_url_override,
            custom_headers,
            request_tags,
            request,
            &chunk_tx,
            use_codex_backend,
        )
        .await;
    }

    let url = resolve_endpoint(base_url_override);
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
        .header("Content-Type", "application/json");
    builder = apply_source_header(builder, custom_headers);
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

    // Codex backend errors are handled in `complete_streaming()` — callers with
    // `use_codex_backend == true` short-circuit there above. Reaching this point
    // means we're on the platform Responses API path, which doesn't emit the
    // codex `x-codex-*` headers or `usage_limit_reached` envelopes.
    if !status.is_success() {
        return Err(openai_http_error(status.as_u16(), status.as_str(), &body));
    }

    let responses_response: ResponsesApiResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    normalize_responses_api_response(responses_response)
}

// ---------------------------------------------------------------------------
// Streaming — Responses API
// ---------------------------------------------------------------------------

/// Map a Responses API error `code` + message to a typed `LlmError`.
/// Substring match on `code` because `OpenAI` uses many code variants
/// (e.g. `rate_limit_exceeded`, `requests_per_min_limit`).
///
/// Codex-specific codes (`usage_limit_reached`, `usage_not_included`,
/// `server_is_overloaded`, `slow_down`) route to the same terminal variants
/// `parse_codex_error` uses on the HTTP-status path. SSE-side has no headers,
/// so `QuotaDetails` is empty — the plan-aware formatter handles `plan_type:
/// None` by falling back to generic wording (see PR #77 tests).
fn classify_responses_error(code: &str, message: &str) -> LlmError {
    let detail = if code.is_empty() {
        message.to_string()
    } else {
        format!("{code}: {message}")
    };
    let lower = code.to_ascii_lowercase();

    // Codex-specific terminal signals — match PR 77's HTTP-path semantics.
    if lower == "usage_limit_reached" {
        return LlmError::usage_limit_reached(QuotaDetails {
            plan_type: None,
            resets_at: None,
            limit_id: None,
            limit_name: None,
            primary: None,
            secondary: None,
            credits: None,
            promo_message: None,
        });
    }
    if lower == "usage_not_included" {
        return LlmError::auth(
            "Upgrade required: this plan does not include Codex usage. \
             Visit https://chatgpt.com/codex/settings/usage to upgrade.",
        );
    }
    if lower == "server_is_overloaded" || lower == "slow_down" {
        return LlmError::server_overloaded(
            "Selected model is at capacity. Try a different model.",
        );
    }

    let lower = format!(
        "{} {}",
        code.to_ascii_lowercase(),
        message.to_ascii_lowercase()
    );

    if lower.contains("rate_limit") || lower.contains("quota") || lower.contains("requests_per") {
        LlmError::rate_limit(detail)
    } else if lower.contains("auth")
        || lower.contains("invalid_api_key")
        || lower.contains("permission")
    {
        LlmError::auth(detail)
    } else if lower.contains("context_length")
        || lower.contains("context length")
        || lower.contains("token_limit")
    {
        LlmError::new(super::LlmErrorKind::ContextWindowExceeded, detail)
    } else if lower.contains("content_filter") || lower.contains("safety") {
        LlmError::new(super::LlmErrorKind::ContentFilter, detail)
    } else if lower.contains("invalid") || lower.contains("bad_request") {
        LlmError::invalid_request(detail)
    } else {
        // Default: retryable server error so the executor's retry loop kicks in.
        LlmError::server_error(detail)
    }
}

/// Accumulates state across Responses API SSE stream events.
struct ResponsesStreamAccumulator {
    input_tokens: u32,
    output_tokens: u32,
    /// Cached subset of `input_tokens` (`OpenAI` `input_tokens_details.cached_tokens`).
    cached_tokens: u32,
    /// Completed output items collected from `response.output_item.done` events.
    output_items: Vec<ResponsesApiOutput>,
    /// Set true when `response.done` is received.
    pub done: bool,
    /// Logged-once flag: first empty-`dispatch_type` event per stream gets a
    /// truncated payload dump at debug, subsequent ones are silent. Gateways
    /// that omit the SSE `event:` line **and** the JSON `type` field are
    /// otherwise opaque — capturing one example per stream is enough to
    /// classify the wire shape next time the success path stops working.
    logged_empty_dispatch: bool,
}

impl ResponsesStreamAccumulator {
    fn new() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            output_items: Vec::new(),
            done: false,
            logged_empty_dispatch: false,
        }
    }

    #[allow(clippy::too_many_lines)] // dispatch table; each arm is small
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
            // Top-level stream error. Two shapes observed in the wild:
            //   OpenAI platform: { type:"error", code, message, param, sequence_number }
            //   Codex/ChatGPT:   { type:"error", error:{ type, code, message, param }, sequence_number }
            // Try the nested codex shape first; fall back to flat OpenAI shape.
            "error" => {
                tracing::warn!(
                    event = "error",
                    data = %data,
                    "responses_api SSE error event — full payload"
                );
                let nested = v.get("error");
                let code = nested
                    .and_then(|e| e.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| v.get("code").and_then(serde_json::Value::as_str))
                    .unwrap_or("");
                let message = nested
                    .and_then(|e| e.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| v.get("message").and_then(serde_json::Value::as_str))
                    .unwrap_or("(no message)");
                return Err(classify_responses_error(code, message));
            }
            // Terminal failure event. Shape: { type, response: { status: "failed", error: { code, message } } }
            "response.failed" => {
                tracing::warn!(
                    event = "response.failed",
                    data = %data,
                    "responses_api SSE response.failed event — full payload"
                );
                let err = v.pointer("/response/error");
                let code = err
                    .and_then(|e| e.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let message = err
                    .and_then(|e| e.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("(no message)");
                return Err(classify_responses_error(code, message));
            }
            // Partial response — model stopped early. Shape: { response: { incomplete_details: { reason } } }
            "response.incomplete" => {
                tracing::warn!(
                    event = "response.incomplete",
                    data = %data,
                    "responses_api SSE response.incomplete event — full payload"
                );
                let reason = v
                    .pointer("/response/incomplete_details/reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                return Err(if reason == "content_filter" {
                    LlmError::new(
                        super::LlmErrorKind::ContentFilter,
                        format!("Response incomplete: {reason}"),
                    )
                } else {
                    LlmError::server_error(format!("Response incomplete: {reason}"))
                });
            }
            // OpenAI Responses API terminal event; the wire spec uses "response.completed".
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
                    self.cached_tokens = u32::try_from(
                        usage
                            .pointer("/input_tokens_details/cached_tokens")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    )
                    .unwrap_or(0);
                } else {
                    tracing::warn!(data, "responses_api terminal event had no /response/usage");
                }
                // Fallback: recover the assembled output from the terminal
                // event's `/response/output` array when no per-item.done
                // events arrived. Observed against the AI gateway path on
                // 2026-05-11: stream emitted `response.output_item.added` +
                // `response.content_part.added` + several events with no
                // SSE `event:` line and no JSON `type` field, then
                // `response.completed` — no `response.output_item.done`.
                // Per-item events captured nothing, the terminal payload
                // contained the full assembled message, and Phoenix
                // persisted an empty agent message ("end_turn with empty
                // content"). Reading /response/output as authoritative
                // here removes the single-event dependency.
                if self.output_items.is_empty() {
                    if let Some(arr) = v
                        .pointer("/response/output")
                        .and_then(serde_json::Value::as_array)
                    {
                        for item in arr {
                            match serde_json::from_value::<ResponsesApiOutput>(item.clone()) {
                                Ok(output) => self.output_items.push(output),
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    item = %item,
                                    "responses_api response.completed fallback: output item deserialize failed"
                                ),
                            }
                        }
                        if !self.output_items.is_empty() {
                            tracing::info!(
                                n = self.output_items.len(),
                                "responses_api recovered output from response.completed \
                                 (no per-item.done events seen on stream)"
                            );
                        }
                    }
                }
                self.done = true;
            }
            _ => {
                tracing::debug!(dispatch_type, "responses_api ignoring event");
                if dispatch_type.is_empty() && !self.logged_empty_dispatch {
                    self.logged_empty_dispatch = true;
                    // Char-aware truncation — slicing by byte index would
                    // panic on a non-UTF8 boundary.
                    let truncated: String = data.chars().take(500).collect();
                    let suffix = if data.len() > truncated.len() {
                        format!("…[truncated from {} bytes]", data.len())
                    } else {
                        String::new()
                    };
                    tracing::debug!(
                        event_type = %event_type,
                        data = %format!("{truncated}{suffix}"),
                        "responses_api empty-dispatch event — first occurrence in this stream"
                    );
                }
            }
        }
        Ok(())
    }

    fn into_response(self) -> Result<LlmResponse, LlmError> {
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
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: self.cached_tokens,
                },
            },
        })
    }
}

/// Complete with streaming, emitting `TokenChunk::Text` events via `chunk_tx`.
#[allow(clippy::too_many_arguments)]
pub async fn complete_streaming(
    spec: &ModelSpec,
    api_key: &str,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
    use_codex_backend: bool,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_endpoint(base_url_override);
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
        .header("Content-Type", "application/json");
    builder = apply_source_header(builder, custom_headers);
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
        let headers = response.headers().clone();
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::network(format!("Failed to read error response: {e}")))?;
        if use_codex_backend {
            if let Some(err) = parse_codex_error(status.as_u16(), &headers, &body) {
                return Err(err);
            }
        }
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    // Codex bridge emits a fresh quota snapshot in response headers on
    // every successful turn (`x-codex-{plan-type,active-limit,primary-*,
    // secondary-*,credits-*}`). Read them once here and broadcast a single
    // `RateLimitSnapshot` chunk per turn — the WebSocket variant of this
    // backend delivers an equivalent `codex.rate_limits` SSE frame mid-
    // stream, but the HTTP transport Phoenix uses does not. Phoenix's UI
    // (`ui/src/codexQuota.ts`) only cares about the latest value, so a
    // single emission per turn is sufficient.
    if use_codex_backend {
        if let Some(snapshot) =
            super::rate_limit::quota_from_codex_response_headers(response.headers())
        {
            let _ = chunk_tx.send(super::TokenChunk::RateLimitSnapshot(snapshot));
        }
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

    acc.into_response()
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
                // Anthropic-specific server blocks: executed by the Anthropic API,
                // with no representable equivalent in the OpenAI Responses wire
                // format — dropped from the OpenAI request. Logged per-block with
                // the discriminant + id so a provider-switch context gap is
                // diagnosable, not a static content-free line.
                ContentBlock::ServerToolUse { id, .. } | ContentBlock::McpToolUse { id, .. } => {
                    tracing::debug!(
                        block_type = block.type_tag(),
                        block_id = %id,
                        role,
                        "dropping Anthropic server block in OpenAI message translation \
                         — no OpenAI wire equivalent"
                    );
                }
                ContentBlock::ToolSearchToolResult { tool_use_id, .. }
                | ContentBlock::WebSearchToolResult { tool_use_id, .. }
                | ContentBlock::WebFetchToolResult { tool_use_id, .. }
                | ContentBlock::CodeExecutionToolResult { tool_use_id, .. }
                | ContentBlock::BashCodeExecutionToolResult { tool_use_id, .. }
                | ContentBlock::TextEditorCodeExecutionToolResult { tool_use_id, .. }
                | ContentBlock::McpToolResult { tool_use_id, .. } => {
                    tracing::debug!(
                        block_type = block.type_tag(),
                        tool_use_id = %tool_use_id,
                        role,
                        "dropping Anthropic server block in OpenAI message translation \
                         — no OpenAI wire equivalent"
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
                    let mut parts = vec![ResponsesApiContentPart::InputText { text }];
                    for img in images {
                        let ImageSource::Base64 { media_type, data } = img;
                        parts.push(ResponsesApiContentPart::InputImage {
                            image_url: format!("data:{media_type};base64,{data}"),
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
fn normalize_responses_api_response(resp: ResponsesApiResponse) -> Result<LlmResponse, LlmError> {
    let mut content = Vec::new();

    for output in resp.output {
        match output.r#type.as_str() {
            "message" => {
                if let Some(output_content) = output.content {
                    for item in output_content {
                        let text = match item.r#type.as_str() {
                            "output_text" => item.text,
                            // A refusal is the model's actual reply — it
                            // declined. Surface it as text (Anthropic returns
                            // refusals as plain text too) so the turn is
                            // non-empty and the billed-but-empty guard below
                            // does not retry a final answer.
                            "refusal" => item.refusal,
                            other => {
                                tracing::debug!(
                                    part_type = %other,
                                    "ignoring unknown message content part"
                                );
                                None
                            }
                        };
                        if let Some(text) = text {
                            if !text.is_empty() {
                                content.push(ContentBlock::Text { text });
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

    // Billed-but-empty guard: OpenAI reported output tokens but the
    // assembled response carried no content block — the message was
    // lost, most often a gateway dropping the output array. Surface a
    // retryable server error instead of persisting an empty agent turn
    // the user was billed for. Complements the response.completed
    // output-recovery fallback, which handles the partial-loss case.
    if content.is_empty() && resp.usage.output_tokens > 0 {
        tracing::error!(
            output_tokens = resp.usage.output_tokens,
            status = %resp.status,
            "responses_api returned empty content with output tokens billed"
        );
        return Err(LlmError::server_error(format!(
            "OpenAI returned empty response ({} output tokens billed, status={})",
            resp.usage.output_tokens, resp.status
        )));
    }

    Ok(LlmResponse {
        content,
        end_turn,
        usage: {
            // OpenAI's `input_tokens` is inclusive of `cached_tokens`, whereas
            // `Usage::context_window_used()` sums input + cache_read. Split the
            // cached subset out of `input_tokens` so the sum stays accurate
            // and the cached count is no longer silently discarded.
            let cached = u64::from(resp.usage.input_tokens_details.cached_tokens);
            Usage {
                input_tokens: u64::from(resp.usage.input_tokens).saturating_sub(cached),
                output_tokens: u64::from(resp.usage.output_tokens),
                // The Responses API has no cache-*creation* concept; this is a
                // typed sink for OpenAI, not an unparsed field.
                cache_creation_tokens: 0,
                cache_read_tokens: cached,
            }
        },
    })
}

// ===========================================================================
// Codex backend error parsing (REQ-LLM-006a)
// ===========================================================================
//
// Mirrors the codex CLI's `map_api_error` decision tree
// (`codex-rs/codex-api/src/api_bridge.rs:42-121`) for the responses Phoenix
// can encounter when routed through `chatgpt.com/backend-api/codex`. Returns
// `None` when the response doesn't match any codex-specific shape so the
// caller can fall through to the generic `OpenAIErrorResponse` path.

#[derive(Debug, Deserialize)]
struct CodexUsageErrorEnvelope {
    error: CodexUsageError,
}

#[derive(Debug, Deserialize)]
struct CodexUsageError {
    #[serde(rename = "type")]
    error_type: Option<String>,
    plan_type: Option<String>,
    resets_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CodexCodedErrorEnvelope {
    error: CodexCodedError,
}

#[derive(Debug, Deserialize)]
struct CodexCodedError {
    code: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // Available for future surfacing if needed
    message: Option<String>,
}

fn parse_codex_error(status: u16, headers: &HeaderMap, body: &str) -> Option<LlmError> {
    match status {
        429 => {
            let envelope = serde_json::from_str::<CodexUsageErrorEnvelope>(body).ok()?;
            match envelope.error.error_type.as_deref() {
                Some("usage_limit_reached") => {
                    let limit_id = parse_active_limit(headers);
                    let (primary, secondary, limit_name) =
                        parse_rate_limit_for_limit(headers, limit_id.as_deref());
                    let credits = parse_credits_snapshot(headers);
                    let promo_message = parse_promo_message(headers);
                    let resets_at = envelope
                        .error
                        .resets_at
                        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0));
                    Some(LlmError::usage_limit_reached(QuotaDetails {
                        plan_type: envelope.error.plan_type,
                        resets_at,
                        limit_id,
                        limit_name,
                        primary,
                        secondary,
                        credits,
                        promo_message,
                    }))
                }
                Some("usage_not_included") => Some(LlmError::auth(
                    "Upgrade required: this plan does not include Codex usage. \
                     Visit https://chatgpt.com/codex/settings/usage to upgrade.",
                )),
                // Recognised envelope shape but the codex backend didn't flag
                // it as a quota exhaustion — treat as a transient throttle.
                _ => None,
            }
        }
        503 => {
            let envelope = serde_json::from_str::<CodexCodedErrorEnvelope>(body).ok()?;
            match envelope.error.code.as_deref() {
                Some("server_is_overloaded" | "slow_down") => Some(LlmError::server_overloaded(
                    "Selected model is at capacity. Try a different model.",
                )),
                _ => None,
            }
        }
        _ => None,
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
    code: Option<serde_json::Value>,
}

// Responses API types (for codex models)

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

/// Function call output: plain string when text-only, array of parts when images present.
///
/// The Responses API treats a `function_call_output` payload as model *input*,
/// so its content parts use the same `input_text`/`input_image` discriminants as
/// `ResponsesApiContentPart` — not `text`/`image_url`, which the API rejects.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesApiFunctionOutput {
    Text(String),
    Parts(Vec<ResponsesApiContentPart>),
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
    #[serde(default)]
    pub(crate) refusal: Option<String>,
}

/// `usage.input_tokens_details` on the Responses API wire. Only the cached
/// subset is consumed; the field defaults to zero when the gateway omits it.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ResponsesApiInputTokensDetails {
    #[serde(default)]
    pub(crate) cached_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiUsage {
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
    /// `OpenAI`'s `input_tokens` already *includes* `cached_tokens` — cached
    /// is a subset of input, not an additional bucket. This is a typed parse
    /// site (not a bare `0`) so "`OpenAI` doesn't report this" is no longer
    /// indistinguishable from "we forgot to parse it".
    #[serde(default)]
    pub(crate) input_tokens_details: ResponsesApiInputTokensDetails,
}

// ===========================================================================
// Chat Completions API
// ===========================================================================

/// Complete using the `OpenAI` Chat Completions API (non-streaming).
#[allow(clippy::too_many_arguments)]
pub async fn complete_chat(
    spec: &ModelSpec,
    api_key: &str,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_chat_endpoint(base_url_override);
    let mut chat_request = translate_to_chat_request(&spec.api_name, request);
    if !request_tags.is_empty() {
        chat_request.tags = Some(request_tags.clone());
    }

    let client = Client::builder()
        .timeout(Duration::from_mins(5))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    builder = apply_source_header(builder, custom_headers);
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
    maybe_log_raw_chat_response(&spec.api_name, "chat.completions.response", &body);

    if !status.is_success() {
        return Err(openai_http_error(status.as_u16(), status.as_str(), &body));
    }

    let chat_response: ChatCompletionsResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    normalize_chat_response(chat_response, &spec.api_name)
}

/// Complete using the `OpenAI` Chat Completions API (streaming).
#[allow(clippy::too_many_arguments)]
pub async fn complete_streaming_chat(
    spec: &ModelSpec,
    api_key: &str,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::broadcast::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_chat_endpoint(base_url_override);
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
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream");
    builder = apply_source_header(builder, custom_headers);
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
        maybe_log_raw_chat_response(&spec.api_name, "chat.completions.stream.http_error", &body);
        return Err(openai_http_error(status.as_u16(), status.as_str(), &body));
    }

    let mut acc = ChatStreamAccumulator::new();
    let mut sse = super::sse::SseParser::new();
    let mut stream = response.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        for event in sse.push(&chunk) {
            maybe_log_raw_chat_response(
                &spec.api_name,
                "chat.completions.stream.event",
                &event.data,
            );
            if let Err(e) = acc.process_event(&event.data, chunk_tx) {
                tracing::error!(
                    data_len = event.data.len(),
                    "chat SSE event processing failed; dumping parser diagnostics"
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
        maybe_log_raw_chat_response(&spec.api_name, "chat.completions.stream.event", &event.data);
        acc.process_event(&event.data, chunk_tx)?;
    }

    acc.into_response()
}

/// Translate `LlmRequest` to `ChatCompletionsRequest`.
#[allow(clippy::too_many_lines)] // single-pass message translation
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
        let mut text_blocks: Vec<&str> = Vec::new();
        let mut ordered_parts: Vec<ChatContentPart> = Vec::new();
        let mut has_image = false;
        let mut tool_calls: Vec<ChatToolCall> = Vec::new();
        let mut tool_results: Vec<&super::types::ContentBlock> = Vec::new();

        for block in &msg.content {
            match block {
                super::types::ContentBlock::Text { text } => {
                    text_blocks.push(text);
                    ordered_parts.push(ChatContentPart::Text { text: text.clone() });
                }
                super::types::ContentBlock::Image { source } => {
                    let ImageSource::Base64 { media_type, data } = source;
                    has_image = true;
                    ordered_parts.push(ChatContentPart::ImageUrl {
                        image_url: ChatImageUrl {
                            url: format!("data:{media_type};base64,{data}"),
                        },
                    });
                }
                super::types::ContentBlock::ToolUse { id, name, input } => {
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
                super::types::ContentBlock::ToolResult { .. } => tool_results.push(block),
                super::types::ContentBlock::ServerToolUse { id, .. }
                | super::types::ContentBlock::McpToolUse { id, .. } => {
                    tracing::debug!(
                        block_type = block.type_tag(),
                        block_id = %id,
                        role,
                        "dropping Anthropic server block in chat completions translation \
                         — no Chat Completions wire equivalent"
                    );
                }
                super::types::ContentBlock::ToolSearchToolResult { tool_use_id, .. }
                | super::types::ContentBlock::WebSearchToolResult { tool_use_id, .. }
                | super::types::ContentBlock::WebFetchToolResult { tool_use_id, .. }
                | super::types::ContentBlock::CodeExecutionToolResult { tool_use_id, .. }
                | super::types::ContentBlock::BashCodeExecutionToolResult { tool_use_id, .. }
                | super::types::ContentBlock::TextEditorCodeExecutionToolResult {
                    tool_use_id,
                    ..
                }
                | super::types::ContentBlock::McpToolResult { tool_use_id, .. } => {
                    tracing::debug!(
                        block_type = block.type_tag(),
                        tool_use_id = %tool_use_id,
                        role,
                        "dropping Anthropic server block in chat completions translation \
                         — no Chat Completions wire equivalent"
                    );
                }
            }
        }

        if !text_blocks.is_empty() || has_image || !tool_calls.is_empty() {
            let content = if !has_image && !text_blocks.is_empty() {
                Some(ChatContent::Text(text_blocks.join("\n")))
            } else if has_image {
                Some(ChatContent::Parts(ordered_parts))
            } else {
                None
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
            if let super::types::ContentBlock::ToolResult {
                tool_use_id,
                content,
                images,
                is_error,
            } = block
            {
                if !images.is_empty() {
                    tracing::debug!(
                        n = images.len(),
                        "dropping images from chat completions tool result \
                         — unsupported by this wire format"
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

    let has_tools = !request.tools.is_empty();
    ChatCompletionsRequest {
        model: api_name.to_string(),
        messages,
        tools,
        max_tokens: request.max_tokens,
        stream: None,
        stream_options: None,
        tool_choice: if has_tools {
            Some("auto".to_string())
        } else {
            None
        },
        parallel_tool_calls: if has_tools { Some(true) } else { None },
        tags: None,
    }
}

fn is_chat_reasoning_marker_content(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '<')
}

fn log_dropped_reasoning_content(model: &str, text: &str) {
    if !text.is_empty() {
        tracing::debug!(
            model,
            bytes = text.len(),
            "dropping chat completions reasoning_content — reasoning display is unsupported"
        );
    }
}

fn normalize_chat_response(
    resp: ChatCompletionsResponse,
    model: &str,
) -> Result<LlmResponse, LlmError> {
    let Some(choice) = resp.choices.into_iter().next() else {
        return Err(LlmError::invalid_response(
            "Chat completions returned no choices",
        ));
    };
    match choice.finish_reason.as_deref() {
        Some("length") => {
            log_chat_completion_length(
                model,
                resp.usage.as_ref().and_then(ChatUsage::reasoning_tokens),
            );
            return Err(LlmError::output_limit_exceeded(
                "Chat completions hit the output token limit before finishing. \
                 Try again with a larger max_tokens value or a model with a higher output budget."
                    .to_string(),
            ));
        }
        Some("content_filter") => {
            return Err(LlmError::new(
                super::LlmErrorKind::ContentFilter,
                "Chat completions response was blocked by the provider content filter",
            ));
        }
        _ => {}
    }
    chat_message_to_response(choice.message, resp.usage, model)
}

fn log_chat_completion_length(model: &str, reasoning_tokens: Option<u32>) {
    if let Some(tokens) = reasoning_tokens {
        tracing::warn!(
            model,
            reasoning_tokens = tokens,
            "chat completions hit output limit"
        );
        tracing::debug!(
            model,
            reasoning_tokens = tokens,
            "chat completions usage included reasoning_tokens at length"
        );
    } else {
        tracing::warn!(model, "chat completions hit output limit");
    }
}

fn chat_message_to_response(
    message: ChatResponseMessage,
    usage: Option<ChatUsage>,
    model: &str,
) -> Result<LlmResponse, LlmError> {
    let mut content = Vec::new();
    if let Some(reasoning) = message.reasoning_content {
        log_dropped_reasoning_content(model, &reasoning);
    }
    if let Some(text) = message.content {
        if !text.is_empty() && !is_chat_reasoning_marker_content(&text) {
            content.push(ContentBlock::Text { text });
        } else if is_chat_reasoning_marker_content(&text) {
            tracing::debug!(
                model,
                bytes = text.len(),
                "dropping chat completions marker content"
            );
        }
    }
    if content.is_empty() {
        if let Some(refusal) = message.refusal {
            if !refusal.is_empty() {
                content.push(ContentBlock::Text { text: refusal });
            }
        }
    }
    for call in message.tool_calls.unwrap_or_default() {
        let input = serde_json::from_str(&call.function.arguments).unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                arguments = %call.function.arguments,
                "failed to parse chat tool call arguments"
            );
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
            "Chat completions returned empty response",
        ));
    }
    let has_tool_calls = content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse { .. }));
    let usage = usage.unwrap_or_default();
    let cached = u64::from(usage.prompt_tokens_details.cached_tokens);
    Ok(LlmResponse {
        content,
        end_turn: !has_tool_calls,
        usage: Usage {
            input_tokens: u64::from(usage.prompt_tokens).saturating_sub(cached),
            output_tokens: u64::from(usage.completion_tokens),
            cache_creation_tokens: 0,
            cache_read_tokens: cached,
        },
    })
}

struct ChatStreamAccumulator {
    content: String,
    tool_calls: Vec<ChatToolCallBuilder>,
    usage: Option<ChatUsage>,
    done: bool,
    terminal_finish_seen: bool,
}

fn chat_stream_error_code_as_string(code: Option<&serde_json::Value>) -> String {
    match code {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn openai_http_error(status_code: u16, status_display: &str, body: &str) -> LlmError {
    if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(body) {
        let message = error_resp.error.message;
        let code = chat_stream_error_code_as_string(error_resp.error.code.as_ref());
        let classified = classify_responses_error(&code, &message);
        return match classified.kind {
            super::LlmErrorKind::UsageLimitReached
            | super::LlmErrorKind::ServerOverloaded
            | super::LlmErrorKind::ContextWindowExceeded
            | super::LlmErrorKind::ContentFilter
            | super::LlmErrorKind::Auth
            | super::LlmErrorKind::RateLimit
            | super::LlmErrorKind::InvalidRequest
            | super::LlmErrorKind::InvalidResponse => classified,
            super::LlmErrorKind::ServerError if matches!(status_code, 401 | 403) => {
                LlmError::auth(format!("Authentication failed: {message}"))
            }
            super::LlmErrorKind::ServerError if status_code == 429 => {
                LlmError::rate_limit(format!("Rate limit exceeded: {message}"))
            }
            super::LlmErrorKind::ServerError if (400..=499).contains(&status_code) => {
                LlmError::invalid_request(format!("Bad request ({status_display}): {message}"))
            }
            super::LlmErrorKind::ServerError if (500..=599).contains(&status_code) => {
                LlmError::server_error(format!("Server error: {message}"))
            }
            super::LlmErrorKind::Network
            | super::LlmErrorKind::OutputLimitExceeded
            | super::LlmErrorKind::ServerError => {
                LlmError::server_error(format!("Unexpected HTTP {status_display}: {message}"))
            }
        };
    }
    LlmError::from_http_status(status_code, body)
}

impl ChatStreamAccumulator {
    fn new() -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            usage: None,
            done: false,
            terminal_finish_seen: false,
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
        if let Some(err) = event.error {
            let msg = err
                .message
                .unwrap_or_else(|| "gateway returned error chunk".to_string());
            let code = chat_stream_error_code_as_string(err.code.as_ref());
            return Err(classify_responses_error(&code, &msg));
        }
        self.usage = event.usage.or(self.usage.take());
        let reasoning_tokens = self.usage.as_ref().and_then(ChatUsage::reasoning_tokens);
        for choice in event.choices {
            if let Some(reasoning) = choice.delta.reasoning_content {
                log_dropped_reasoning_content("<streaming-chat-completions>", &reasoning);
            }
            if let Some(reason) = choice.finish_reason.as_deref() {
                self.terminal_finish_seen = true;
                match reason {
                    "length" => {
                        log_chat_completion_length(
                            "<streaming-chat-completions>",
                            reasoning_tokens,
                        );
                        return Err(LlmError::output_limit_exceeded(
                            "Chat completions hit the output token limit before finishing. \
                             Try again with a larger max_tokens value or a model with a higher output budget."
                                .to_string(),
                        ));
                    }
                    "content_filter" => {
                        return Err(LlmError::new(
                            super::LlmErrorKind::ContentFilter,
                            "Chat completions response was blocked by the provider content filter",
                        ));
                    }
                    _ => {}
                }
            }
            if let Some(delta) = choice.delta.content {
                if !delta.is_empty() && !is_chat_reasoning_marker_content(&delta) {
                    self.content.push_str(&delta);
                    let _ = chunk_tx.send(super::TokenChunk::Text(delta));
                } else if is_chat_reasoning_marker_content(&delta) {
                    tracing::debug!(
                        bytes = delta.len(),
                        "dropping chat completions marker content"
                    );
                }
            }
            for tool_delta in choice.delta.tool_calls.unwrap_or_default() {
                let index = tool_delta
                    .index
                    .unwrap_or_else(|| self.tool_calls.len().saturating_sub(1));
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
        if !self.done && !self.terminal_finish_seen {
            return Err(LlmError::invalid_response(
                "Chat completions stream ended before a terminal finish_reason or [DONE] sentinel",
            ));
        }
        if self.content.is_empty() && self.tool_calls.is_empty() {
            tracing::warn!(
                done = self.done,
                "chat stream produced no content and no tool_calls"
            );
        }
        let message = ChatResponseMessage {
            reasoning_content: None,
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            refusal: None,
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
        chat_message_to_response(message, self.usage, "<streaming-chat-completions>")
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

// ---------------------------------------------------------------------------
// Chat Completions wire types
// ---------------------------------------------------------------------------

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

#[derive(Debug, Serialize, Deserialize)]
struct ChatToolCall {
    id: String,
    r#type: String,
    function: ChatFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize)]
struct ChatStreamOptions {
    include_usage: bool,
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
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

/// Chat Completions streaming chunk.
///
/// Some gateways emit inline error data events instead of failing the HTTP
/// request — e.g. `{"error": {"message": "...", "code": 400}}`. The `error`
/// field captures these so the caller can surface the gateway's message and
/// code rather than reporting an empty-stream error.
#[derive(Debug, Deserialize)]
struct ChatStreamChunk {
    #[serde(default)]
    choices: Vec<ChatStreamChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
    #[serde(default)]
    error: Option<ChatStreamError>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamError {
    #[serde(default)]
    message: Option<String>,
    /// Code may be an integer or a string depending on the gateway.
    #[serde(default)]
    code: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamChoice {
    delta: ChatDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatDelta {
    #[serde(default)]
    reasoning_content: Option<String>,
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

/// `usage.prompt_tokens_details` on the Chat Completions wire. Baseten and
/// OpenAI-compatible gateways may report prompt cache hits here; omitted details
/// mean no cache-read accounting is available for this response.
#[derive(Debug, Default, Deserialize)]
struct ChatPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u32,
}

#[derive(Debug, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    /// Chat Completions `prompt_tokens` includes `cached_tokens`, so normalization
    /// splits cached reads out before storing Phoenix's uncached input bucket.
    #[serde(default)]
    prompt_tokens_details: ChatPromptTokensDetails,
    #[serde(default)]
    completion_tokens_details: ChatCompletionTokensDetails,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

impl ChatUsage {
    fn reasoning_tokens(&self) -> Option<u32> {
        self.completion_tokens_details.reasoning_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::has_custom_source_header;
    use crate::types::{LlmRequest, PromptCacheKey};

    fn empty_request() -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            cache_key: PromptCacheKey::stable("test"),
        }
    }

    #[test]
    fn custom_source_header_suppresses_default_source_header() {
        assert!(has_custom_source_header(&[(
            "source".to_string(),
            "custom-poc".to_string(),
        )]));
        assert!(has_custom_source_header(&[(
            "Source".to_string(),
            "custom-poc".to_string(),
        )]));
        assert!(!has_custom_source_header(&[(
            "x-source".to_string(),
            "custom-poc".to_string(),
        )]));
    }

    /// A tool result carrying an image (e.g. `read_image`) serialises its
    /// `function_call_output` parts with the Responses API's `input_text` /
    /// `input_image` discriminants. Regression guard: the API rejects the
    /// Chat-Completions-style `text` / `image_url` types with HTTP 400.
    #[test]
    fn tool_result_image_serialises_with_responses_api_part_types() {
        use crate::types::{ContentBlock, ImageSource, LlmMessage, MessageRole};

        let mut req = empty_request();
        req.messages = vec![LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "here is the screenshot".to_string(),
                images: vec![ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                }],
                is_error: false,
            }],
        }];

        let translated = translate_to_responses_request("gpt-5.5", &req, false);
        let json = serde_json::to_value(&translated).unwrap();
        let parts = &json["input"][0]["output"];

        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "here is the screenshot");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,aGVsbG8=");
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

    // Codex backend 429/503 parsing — fixtures mirror
    // codex-rs/codex-api/src/api_bridge_tests.rs.
    mod codex_errors {
        use super::super::parse_codex_error;
        use crate::LlmErrorKind;
        use reqwest::header::{HeaderMap, HeaderValue};

        #[test]
        fn usage_limit_reached_plus_plan_renders_plus_wording() {
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"plus","resets_at":1709568000}}"#;
            let err = parse_codex_error(429, &HeaderMap::new(), body).expect("parsed");
            assert_eq!(err.kind, LlmErrorKind::UsageLimitReached);
            assert!(err.quota.is_some(), "quota payload threaded through");
            assert!(
                err.message.contains("Upgrade to Pro"),
                "got: {}",
                err.message
            );
        }

        #[test]
        fn usage_limit_reached_pro_plan_renders_credits_path() {
            let body =
                r#"{"error":{"type":"usage_limit_reached","plan_type":"pro","resets_at":null}}"#;
            let err = parse_codex_error(429, &HeaderMap::new(), body).expect("parsed");
            assert_eq!(err.kind, LlmErrorKind::UsageLimitReached);
            assert!(
                err.message.contains("purchase more credits"),
                "got: {}",
                err.message
            );
        }

        #[test]
        fn usage_limit_reached_team_plan_renders_admin_path() {
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"team"}}"#;
            let err = parse_codex_error(429, &HeaderMap::new(), body).expect("parsed");
            assert!(err.message.contains("send a request to your admin"));
        }

        #[test]
        fn usage_limit_reached_free_plan_renders_plus_upgrade() {
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"free"}}"#;
            let err = parse_codex_error(429, &HeaderMap::new(), body).expect("parsed");
            assert!(err.message.contains("Upgrade to Plus"));
        }

        #[test]
        fn usage_limit_reached_unknown_plan_falls_back_to_generic() {
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"mystery"}}"#;
            let err = parse_codex_error(429, &HeaderMap::new(), body).expect("parsed");
            assert_eq!(err.message, "You've hit your usage limit. Try again later.");
        }

        #[test]
        fn usage_limit_reached_threads_promo_message_from_headers() {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-codex-promo-message",
                HeaderValue::from_static("Upgrade to Pro at chatgpt.com/explore/pro"),
            );
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"plus"}}"#;
            let err = parse_codex_error(429, &headers, body).expect("parsed");
            assert!(err
                .message
                .contains("Upgrade to Pro at chatgpt.com/explore/pro"));
            assert_eq!(
                err.quota.as_ref().unwrap().promo_message.as_deref(),
                Some("Upgrade to Pro at chatgpt.com/explore/pro")
            );
        }

        #[test]
        fn usage_limit_reached_extracts_limit_name_from_active_family() {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-codex-active-limit",
                HeaderValue::from_static("codex_other"),
            );
            headers.insert(
                "x-codex-other-limit-name",
                HeaderValue::from_static("gpt-5.2-codex-sonic"),
            );
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"pro"}}"#;
            let err = parse_codex_error(429, &headers, body).expect("parsed");
            let quota = err.quota.as_ref().expect("quota");
            assert_eq!(quota.limit_id.as_deref(), Some("codex_other"));
            assert_eq!(quota.limit_name.as_deref(), Some("gpt-5.2-codex-sonic"));
            // The non-codex limit_name branch wins over the plan wording.
            assert!(
                err.message
                    .starts_with("You've hit your usage limit for gpt-5.2-codex-sonic."),
                "got: {}",
                err.message
            );
        }

        #[test]
        // Parsed percentages are exact, representable values from the fixture header.
        #[allow(clippy::float_cmp)]
        fn usage_limit_reached_extracts_secondary_window() {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-codex-secondary-used-percent",
                HeaderValue::from_static("80"),
            );
            headers.insert(
                "x-codex-secondary-window-minutes",
                HeaderValue::from_static("10080"),
            );
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"plus"}}"#;
            let err = parse_codex_error(429, &headers, body).expect("parsed");
            let quota = err.quota.as_ref().expect("quota");
            let secondary = quota.secondary.as_ref().expect("secondary");
            assert_eq!(secondary.used_percent, 80.0);
            assert_eq!(secondary.window_minutes, Some(10080));
        }

        #[test]
        fn usage_not_included_returns_auth_terminal() {
            let body = r#"{"error":{"type":"usage_not_included"}}"#;
            let err = parse_codex_error(429, &HeaderMap::new(), body).expect("parsed");
            assert_eq!(err.kind, LlmErrorKind::Auth);
            assert!(err.message.contains("Upgrade required"));
        }

        #[test]
        fn plain_429_without_recognized_type_falls_through_to_caller() {
            // A 429 from the codex backend with a body the codex CLI would
            // classify as a transient throttle (RetryLimit) — Phoenix lets the
            // generic OpenAIErrorResponse path handle it as RateLimit.
            let body = r#"{"error":{"message":"slow down","type":"rate_limit_exceeded"}}"#;
            assert!(parse_codex_error(429, &HeaderMap::new(), body).is_none());
        }

        #[test]
        fn malformed_429_body_falls_through() {
            assert!(parse_codex_error(429, &HeaderMap::new(), "not json").is_none());
        }

        #[test]
        fn server_overloaded_503_returns_server_overloaded_terminal() {
            let body = r#"{"error":{"code":"server_is_overloaded"}}"#;
            let err = parse_codex_error(503, &HeaderMap::new(), body).expect("parsed");
            assert_eq!(err.kind, LlmErrorKind::ServerOverloaded);
            assert!(err.message.contains("Try a different model"));
        }

        #[test]
        fn slow_down_503_returns_server_overloaded_terminal() {
            let body = r#"{"error":{"code":"slow_down"}}"#;
            let err = parse_codex_error(503, &HeaderMap::new(), body).expect("parsed");
            assert_eq!(err.kind, LlmErrorKind::ServerOverloaded);
        }

        #[test]
        fn unrelated_503_code_falls_through() {
            let body = r#"{"error":{"code":"something_else"}}"#;
            assert!(parse_codex_error(503, &HeaderMap::new(), body).is_none());
        }

        #[test]
        fn other_status_codes_return_none() {
            assert!(parse_codex_error(500, &HeaderMap::new(), "").is_none());
            assert!(parse_codex_error(400, &HeaderMap::new(), "").is_none());
        }
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

    #[test]
    fn classify_responses_error_codex_codes_route_to_terminal_variants() {
        use super::super::LlmErrorKind;
        // Matches PR 77's HTTP-path semantics — keep these two paths in sync.
        assert_eq!(
            classify_responses_error("usage_limit_reached", "x").kind,
            LlmErrorKind::UsageLimitReached
        );
        assert_eq!(
            classify_responses_error("usage_not_included", "x").kind,
            LlmErrorKind::Auth
        );
        assert_eq!(
            classify_responses_error("server_is_overloaded", "x").kind,
            LlmErrorKind::ServerOverloaded
        );
        assert_eq!(
            classify_responses_error("slow_down", "x").kind,
            LlmErrorKind::ServerOverloaded
        );
        // All four terminal — not retryable
        assert!(!classify_responses_error("usage_limit_reached", "x")
            .kind
            .is_auto_retryable());
        assert!(!classify_responses_error("usage_not_included", "x")
            .kind
            .is_auto_retryable());
        assert!(!classify_responses_error("server_is_overloaded", "x")
            .kind
            .is_auto_retryable());
        assert!(!classify_responses_error("slow_down", "x")
            .kind
            .is_auto_retryable());
    }

    #[test]
    fn classify_responses_error_maps_codes() {
        use super::super::LlmErrorKind;
        assert_eq!(
            classify_responses_error("rate_limit_exceeded", "x").kind,
            LlmErrorKind::RateLimit
        );
        assert_eq!(
            classify_responses_error("requests_per_min_limit", "x").kind,
            LlmErrorKind::RateLimit
        );
        assert_eq!(
            classify_responses_error("invalid_api_key", "x").kind,
            LlmErrorKind::Auth
        );
        assert_eq!(
            classify_responses_error("context_length_exceeded", "x").kind,
            LlmErrorKind::ContextWindowExceeded
        );
        assert_eq!(
            classify_responses_error("content_filter", "x").kind,
            LlmErrorKind::ContentFilter
        );
        assert_eq!(
            classify_responses_error("invalid_request_error", "x").kind,
            LlmErrorKind::InvalidRequest
        );
        // Unknown code defaults to retryable server error.
        assert_eq!(
            classify_responses_error("foo_bar_baz", "x").kind,
            LlmErrorKind::ServerError
        );
        // Empty code falls back to message.
        assert_eq!(
            classify_responses_error("", "boom").kind,
            LlmErrorKind::ServerError
        );
    }

    #[test]
    fn process_event_returns_err_on_top_level_error() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{"type":"error","code":"rate_limit_exceeded","message":"slow down"}"#;
        let err = acc.process_event("error", data, &tx).unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::RateLimit);
    }

    // --- streaming SSE accumulator robustness ---

    #[test]
    fn process_event_malformed_sse_data_is_invalid_response_not_panic() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let err = acc.process_event("", "{ not json", &tx).unwrap_err();
        assert!(
            err.message.contains("Failed to parse SSE data"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn process_event_done_sentinel_is_ignored() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // The `[DONE]` sentinel is not JSON; it must be a no-op, not a parse error.
        acc.process_event("", "[DONE]", &tx).unwrap();
        assert!(
            !acc.done,
            "[DONE] sentinel alone does not finalize the stream"
        );
    }

    #[test]
    fn process_event_unknown_dispatch_type_is_ignored() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        acc.process_event("", r#"{"type":"response.in_progress"}"#, &tx)
            .unwrap();
    }

    #[test]
    fn process_event_empty_dispatch_type_is_ignored_and_logged_once() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // An event whose embedded `type` is empty has nothing to dispatch on; it
        // must be tolerated (logged exactly once), never erroring the stream.
        assert!(!acc.logged_empty_dispatch);
        acc.process_event("", r#"{"type":""}"#, &tx).unwrap();
        assert!(
            acc.logged_empty_dispatch,
            "first empty-dispatch event is logged"
        );
        acc.process_event("", r#"{"type":""}"#, &tx).unwrap();
    }

    #[test]
    fn process_event_assembles_message_text_from_output_item_done() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // The primary (non-fallback) assembly path: a completed message item.
        let data = r#"{
            "type":"response.output_item.done",
            "item":{"type":"message","role":"assistant","content":[
                {"type":"output_text","text":"Pong"}
            ]}
        }"#;
        acc.process_event("response.output_item.done", data, &tx)
            .unwrap();
        assert_eq!(acc.output_items.len(), 1);

        let resp = acc.into_response().unwrap();
        assert!(
            resp.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text == "Pong")),
            "assembled content should carry the message text: {:?}",
            resp.content
        );
    }

    #[test]
    fn process_event_handles_codex_nested_error_shape() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // Real codex/ChatGPT-backend payload captured 2026-05-11 via WARN log.
        let data = r#"{"type":"error","error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"Your input exceeds the context window of this model. Please adjust your input and try again.","param":"input"},"sequence_number":2}"#;
        let err = acc.process_event("error", data, &tx).unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::ContextWindowExceeded);
        assert!(!err.kind.is_auto_retryable());
        assert!(err.message.contains("context_length_exceeded"));
        assert!(err.message.contains("Your input exceeds"));
    }

    #[test]
    fn process_event_returns_err_on_response_failed() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"upstream"}}}"#;
        let err = acc.process_event("response.failed", data, &tx).unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::ServerError);
    }

    #[test]
    fn process_event_returns_err_on_response_incomplete_max_tokens() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#;
        let err = acc
            .process_event("response.incomplete", data, &tx)
            .unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::ServerError);
    }

    /// When the stream lacks `response.output_item.done` events but
    /// `response.completed` carries `/response/output: [...]`, the terminal
    /// payload is authoritative — fall back to it instead of dropping the
    /// assembled message. Repro of the 2026-05-11 gateway behaviour where
    /// `support-chat-completions` produced 5 output tokens, was billed for
    /// them, but Phoenix persisted "`end_turn` with empty content".
    #[test]
    fn process_event_recovers_output_from_response_completed_when_no_item_done() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{
            "type":"response.completed",
            "response":{
                "usage":{"input_tokens":320682,"output_tokens":5,"total_tokens":320687},
                "output":[
                    {"type":"message","role":"assistant","content":[
                        {"type":"output_text","text":"Pong"}
                    ]}
                ]
            }
        }"#;
        acc.process_event("response.completed", data, &tx)
            .expect("response.completed handler should not error on valid payload");
        assert!(acc.done, "response.completed should set done");
        assert_eq!(
            acc.output_items.len(),
            1,
            "fallback should recover the message from /response/output"
        );
        assert_eq!(acc.input_tokens, 320_682);
        assert_eq!(acc.output_tokens, 5);
    }

    /// `OpenAI` reports the cached input subset under
    /// `usage.input_tokens_details.cached_tokens`. It must reach `Usage` and
    /// must not double-count: `input_tokens` already includes the cached
    /// portion, so `context_window_used()` (which sums input + `cache_read`)
    /// stays equal to `OpenAI`'s reported input+output.
    #[test]
    fn responses_api_cached_tokens_are_threaded_without_double_counting() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{
            "type":"response.completed",
            "response":{
                "usage":{
                    "input_tokens":1000,
                    "output_tokens":50,
                    "input_tokens_details":{"cached_tokens":800}
                },
                "output":[
                    {"type":"message","role":"assistant","content":[
                        {"type":"output_text","text":"Pong"}
                    ]}
                ]
            }
        }"#;
        acc.process_event("response.completed", data, &tx)
            .expect("handler should not error");
        assert_eq!(acc.cached_tokens, 800);

        let resp = normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: acc.output_items,
            usage: ResponsesApiUsage {
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: acc.cached_tokens,
                },
            },
        })
        .expect("a response with a message item normalizes");
        assert_eq!(resp.usage.cache_read_tokens, 800);
        assert_eq!(resp.usage.input_tokens, 200);
        assert_eq!(resp.usage.cache_creation_tokens, 0);
        assert_eq!(resp.usage.context_window_used(), 1050);
    }

    /// A gateway that omits `input_tokens_details` must not panic or shift
    /// accounting: cached defaults to 0 and `input_tokens` is unchanged.
    /// `output_tokens` is 0 here — an empty, unbilled response, so the
    /// billed-but-empty guard does not fire.
    #[test]
    fn responses_api_usage_without_cached_details_defaults_to_zero() {
        let usage: ResponsesApiUsage =
            serde_json::from_str(r#"{"input_tokens":10,"output_tokens":0}"#).unwrap();
        assert_eq!(usage.input_tokens_details.cached_tokens, 0);
        let resp = normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: vec![],
            usage,
        })
        .expect("an empty, unbilled response normalizes");
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.cache_read_tokens, 0);
        assert_eq!(resp.usage.context_window_used(), 10);
    }

    /// Billed-but-empty guard: `OpenAI` reporting output tokens for a
    /// response with no content block means the assembled message was
    /// lost in transit (a gateway dropping the output array). Normalization
    /// must surface a retryable error, not a silently-empty agent turn.
    #[test]
    fn responses_api_empty_content_with_billed_tokens_is_retryable_error() {
        let err = normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: vec![],
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens: 42,
                input_tokens_details: ResponsesApiInputTokensDetails { cached_tokens: 0 },
            },
        })
        .expect_err("empty content with billed output tokens must fail");
        assert_eq!(err.kind, crate::LlmErrorKind::ServerError);
        assert!(
            err.kind.is_auto_retryable(),
            "a lost-message response must be retryable so the executor retries"
        );
    }

    /// A `refusal` message part is the model's actual reply — it declined.
    /// It must surface as non-empty text content so the billed-but-empty
    /// guard does not mistake a final answer for a lost message and retry.
    #[test]
    fn responses_api_refusal_message_surfaces_as_text_not_retried() {
        let resp = normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: vec![ResponsesApiOutput {
                r#type: "message".to_string(),
                content: Some(vec![ResponsesApiContent {
                    r#type: "refusal".to_string(),
                    text: None,
                    refusal: Some("I can't help with that.".to_string()),
                }]),
                name: None,
                arguments: None,
                call_id: None,
            }],
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens: 7,
                input_tokens_details: ResponsesApiInputTokensDetails { cached_tokens: 0 },
            },
        })
        .expect("a refusal is valid content, not a billed-but-empty failure");
        assert!(resp.end_turn);
        assert_eq!(resp.content.len(), 1);
        // ContentBlock carries ~13 tool/server variants; only Text is expected here
        // and every other variant is an equivalent test failure.
        #[allow(clippy::wildcard_enum_match_arm)]
        match &resp.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "I can't help with that."),
            other => panic!("expected refusal surfaced as Text, got {other:?}"),
        }
    }

    /// If both `response.output_item.done` and `response.completed` carry
    /// output, the per-item events win — don't double-count by appending the
    /// terminal-event payload on top.
    #[test]
    fn process_event_fallback_skips_when_output_items_already_captured() {
        let (tx, _rx) = tokio::sync::broadcast::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let item_done = r#"{
            "type":"response.output_item.done",
            "item":{"type":"message","role":"assistant","content":[
                {"type":"output_text","text":"Pong"}
            ]}
        }"#;
        let completed = r#"{
            "type":"response.completed",
            "response":{
                "usage":{"input_tokens":10,"output_tokens":1},
                "output":[
                    {"type":"message","role":"assistant","content":[
                        {"type":"output_text","text":"Pong"}
                    ]}
                ]
            }
        }"#;
        acc.process_event("response.output_item.done", item_done, &tx)
            .unwrap();
        acc.process_event("response.completed", completed, &tx)
            .unwrap();
        assert_eq!(
            acc.output_items.len(),
            1,
            "fallback must not duplicate items already captured via item.done"
        );
    }

    // -----------------------------------------------------------------------
    // Chat Completions unit tests
    // -----------------------------------------------------------------------
    mod chat {
        use super::super::*;
        use crate::types::{LlmMessage, LlmRequest, MessageRole, PromptCacheKey};
        use crate::SystemContent;

        fn empty_request() -> LlmRequest {
            LlmRequest {
                system: vec![],
                messages: vec![],
                tools: vec![],
                max_tokens: None,
                cache_key: PromptCacheKey::stable("chat-test"),
            }
        }

        // --- endpoint resolution ---

        #[test]
        fn chat_endpoint_defaults_to_chat_completions() {
            let url = resolve_chat_endpoint(None);
            assert_eq!(url, "https://api.openai.com/v1/chat/completions");
        }

        #[test]
        fn chat_endpoint_preserves_responses_override_exactly() {
            let url = resolve_chat_endpoint(Some("https://gw.example.com/openai/v1/responses"));
            assert_eq!(url, "https://gw.example.com/openai/v1/responses");
        }

        #[test]
        fn chat_endpoint_preserves_chat_completions_override_exactly() {
            let url = resolve_chat_endpoint(Some("https://gw.example.com/v1/chat/completions"));
            assert_eq!(url, "https://gw.example.com/v1/chat/completions");
        }

        #[test]
        fn chat_endpoint_preserves_override_without_v1_segment() {
            let url = resolve_chat_endpoint(Some("https://mygateway.example.com/llm"));
            assert_eq!(url, "https://mygateway.example.com/llm");
        }

        // --- request translation ---

        #[test]
        fn chat_request_system_becomes_system_message() {
            let mut req = empty_request();
            req.system = vec![SystemContent::new("You are helpful.")];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            assert_eq!(chat_req.messages.len(), 1);
            assert_eq!(chat_req.messages[0].role, "system");
            let json = serde_json::to_value(&chat_req).unwrap();
            assert_eq!(json["messages"][0]["content"], "You are helpful.");
        }

        #[test]
        fn chat_request_user_assistant_messages_mapped() {
            let mut req = empty_request();
            req.system = vec![SystemContent::new("sys")];
            req.messages = vec![
                LlmMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::Text {
                        text: "hello".to_string(),
                    }],
                },
                LlmMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "hi".to_string(),
                    }],
                },
            ];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            // system + user + assistant
            assert_eq!(chat_req.messages.len(), 3);
            assert_eq!(chat_req.messages[1].role, "user");
            assert_eq!(chat_req.messages[2].role, "assistant");
        }

        #[test]
        fn chat_request_image_block_maps_to_image_url_part() {
            use crate::types::ImageSource;
            let mut req = empty_request();
            req.messages = vec![LlmMessage {
                role: MessageRole::User,
                content: vec![
                    ContentBlock::Text {
                        text: "look at this".to_string(),
                    },
                    ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".to_string(),
                            data: "aGVsbG8=".to_string(),
                        },
                    },
                ],
            }];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            let parts = &json["messages"][0]["content"];
            assert!(parts.is_array(), "content should be parts array");
            assert_eq!(parts[0]["type"], "text");
            assert_eq!(parts[0]["text"], "look at this");
            assert_eq!(parts[1]["type"], "image_url");
            assert_eq!(
                parts[1]["image_url"]["url"],
                "data:image/png;base64,aGVsbG8="
            );
        }

        #[test]
        fn chat_request_preserves_interleaved_text_image_order() {
            use crate::types::ImageSource;
            let mut req = empty_request();
            req.messages = vec![LlmMessage {
                role: MessageRole::User,
                content: vec![
                    ContentBlock::Text {
                        text: "before".to_string(),
                    },
                    ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type: "image/png".to_string(),
                            data: "aGVsbG8=".to_string(),
                        },
                    },
                    ContentBlock::Text {
                        text: "after".to_string(),
                    },
                ],
            }];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            let parts = &json["messages"][0]["content"];
            assert_eq!(parts[0]["type"], "text");
            assert_eq!(parts[0]["text"], "before");
            assert_eq!(parts[1]["type"], "image_url");
            assert_eq!(parts[2]["type"], "text");
            assert_eq!(parts[2]["text"], "after");
        }

        #[test]
        fn chat_request_tool_use_maps_to_tool_calls() {
            let mut req = empty_request();
            req.messages = vec![LlmMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_abc".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/foo"}),
                }],
            }];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            let tool_calls = &json["messages"][0]["tool_calls"];
            assert!(tool_calls.is_array());
            assert_eq!(tool_calls[0]["id"], "call_abc");
            assert_eq!(tool_calls[0]["type"], "function");
            assert_eq!(tool_calls[0]["function"]["name"], "read_file");
        }

        #[test]
        fn chat_request_tool_result_maps_to_tool_role_message() {
            let mut req = empty_request();
            req.messages = vec![LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_abc".to_string(),
                    content: "file contents here".to_string(),
                    images: vec![],
                    is_error: false,
                }],
            }];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            let msg = &json["messages"][0];
            assert_eq!(msg["role"], "tool");
            assert_eq!(msg["tool_call_id"], "call_abc");
            assert_eq!(msg["content"], "file contents here");
        }

        #[test]
        fn chat_request_tool_result_error_prefixes_with_error_marker() {
            let mut req = empty_request();
            req.messages = vec![LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "command not found".to_string(),
                    images: vec![],
                    is_error: true,
                }],
            }];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            assert_eq!(json["messages"][0]["content"], "Error: command not found");
        }

        #[test]
        fn chat_request_tools_definition_serialized() {
            use crate::types::ToolDefinition;
            let mut req = empty_request();
            req.tools = vec![ToolDefinition {
                name: "bash".to_string(),
                description: "Run a bash command".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                defer_loading: false,
            }];
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            assert_eq!(json["tools"][0]["type"], "function");
            assert_eq!(json["tools"][0]["function"]["name"], "bash");
            assert_eq!(json["tool_choice"], "auto");
            assert_eq!(json["parallel_tool_calls"], true);
        }

        #[test]
        fn chat_request_max_tokens_forwarded() {
            let mut req = empty_request();
            req.max_tokens = Some(4096);
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            assert_eq!(json["max_tokens"], 4096);
        }

        #[test]
        fn chat_request_no_tools_omits_tool_choice_and_parallel() {
            let req = empty_request();
            let chat_req = translate_to_chat_request("gpt-4o", &req);
            let json = serde_json::to_value(&chat_req).unwrap();
            assert!(json.get("tool_choice").is_none());
            assert!(json.get("parallel_tool_calls").is_none());
        }

        // --- streaming request sets stream options ---

        #[test]
        fn chat_streaming_request_sets_stream_and_include_usage() {
            let req = empty_request();
            let mut chat_req = translate_to_chat_request("gpt-4o", &req);
            chat_req.stream = Some(true);
            chat_req.stream_options = Some(ChatStreamOptions {
                include_usage: true,
            });
            let json = serde_json::to_value(&chat_req).unwrap();
            assert_eq!(json["stream"], true);
            assert_eq!(json["stream_options"]["include_usage"], true);
        }

        // --- response normalization ---

        #[test]
        fn chat_normalize_text_response() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: Some("Hello!".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(ChatUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    prompt_tokens_details: ChatPromptTokensDetails::default(),
                    completion_tokens_details: ChatCompletionTokensDetails::default(),
                }),
            };
            let result = normalize_chat_response(resp, "test-model").unwrap();
            assert!(result.end_turn);
            assert_eq!(result.usage.input_tokens, 10);
            assert_eq!(result.usage.output_tokens, 5);
            assert_eq!(result.usage.cache_read_tokens, 0);
            assert!(matches!(&result.content[0], ContentBlock::Text { text } if text == "Hello!"));
        }

        #[test]
        fn chat_normalize_refusal_response_surfaces_as_text() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: None,
                        refusal: Some("I can't help with that.".to_string()),
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            };
            let result = normalize_chat_response(resp, "test-model").unwrap();
            assert!(result.end_turn);
            assert!(
                matches!(&result.content[0], ContentBlock::Text { text } if text == "I can't help with that.")
            );
        }

        #[test]
        fn chat_normalize_threads_cached_prompt_tokens_without_double_counting() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: Some("Hello!".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(ChatUsage {
                    prompt_tokens: 1_000,
                    completion_tokens: 50,
                    prompt_tokens_details: ChatPromptTokensDetails { cached_tokens: 800 },
                    completion_tokens_details: ChatCompletionTokensDetails::default(),
                }),
            };
            let result = normalize_chat_response(resp, "baseten/deepseek-ai/DeepSeek-V4-Pro")
                .expect("response should normalize");
            assert_eq!(result.usage.input_tokens, 200);
            assert_eq!(result.usage.output_tokens, 50);
            assert_eq!(result.usage.cache_read_tokens, 800);
            assert_eq!(result.usage.cache_creation_tokens, 0);
            assert_eq!(result.usage.context_window_used(), 1_050);
        }

        #[test]
        fn chat_normalize_drops_reasoning_and_marker_content() {
            use crate::LlmErrorKind;
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: Some("internal reasoning".to_string()),
                        content: Some("<\n\n".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            };
            let err = normalize_chat_response(resp, "deepseek-ai/DeepSeek-V4-Pro").unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::InvalidResponse);
        }

        #[test]
        fn chat_normalize_keeps_final_content_when_reasoning_present() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: Some("internal reasoning".to_string()),
                        content: Some("Final answer".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            };
            let result = normalize_chat_response(resp, "deepseek-ai/DeepSeek-V4-Pro").unwrap();
            assert!(
                matches!(&result.content[0], ContentBlock::Text { text } if text == "Final answer")
            );
        }

        #[test]
        fn chat_normalize_tool_call_response() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: None,
                        refusal: None,
                        tool_calls: Some(vec![ChatToolCall {
                            id: "call_1".to_string(),
                            r#type: "function".to_string(),
                            function: ChatFunctionCall {
                                name: "read_file".to_string(),
                                arguments: r#"{"path":"/tmp/foo"}"#.to_string(),
                            },
                        }]),
                    },
                    finish_reason: Some("tool_calls".to_string()),
                }],
                usage: None,
            };
            let result = normalize_chat_response(resp, "test-model").unwrap();
            assert!(!result.end_turn, "tool calls should not be end_turn");
            assert!(matches!(
                &result.content[0],
                ContentBlock::ToolUse { name, .. } if name == "read_file"
            ));
        }

        #[test]
        fn chat_normalize_invalid_tool_args_warns_and_uses_empty_object() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: None,
                        refusal: None,
                        tool_calls: Some(vec![ChatToolCall {
                            id: "call_1".to_string(),
                            r#type: "function".to_string(),
                            function: ChatFunctionCall {
                                name: "bash".to_string(),
                                arguments: "not valid json".to_string(),
                            },
                        }]),
                    },
                    finish_reason: Some("tool_calls".to_string()),
                }],
                usage: None,
            };
            let result = normalize_chat_response(resp, "test-model").unwrap();
            if let ContentBlock::ToolUse { input, .. } = &result.content[0] {
                assert_eq!(input, &serde_json::Value::Object(serde_json::Map::new()));
            } else {
                panic!("expected ToolUse content block");
            }
        }

        #[test]
        fn chat_normalize_empty_response_returns_invalid_response_error() {
            use crate::LlmErrorKind;
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: None,
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            };
            let err = normalize_chat_response(resp, "test-model").unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::InvalidResponse);
        }

        #[test]
        fn chat_normalize_no_choices_returns_invalid_response_error() {
            use crate::LlmErrorKind;
            let resp = ChatCompletionsResponse {
                choices: vec![],
                usage: None,
            };
            let err = normalize_chat_response(resp, "test-model").unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::InvalidResponse);
        }

        // --- stream accumulator ---

        #[test]
        fn chat_stream_done_sentinel_sets_done_flag() {
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            acc.process_event("[DONE]", &tx).unwrap();
            assert!(acc.done);
        }

        #[test]
        fn chat_stream_text_delta_accumulated_and_emitted() {
            let (tx, mut rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let chunk = serde_json::json!({
                "choices": [{"delta": {"content": "Hello"}}]
            })
            .to_string();
            acc.process_event(&chunk, &tx).unwrap();
            assert_eq!(acc.content, "Hello");
            let received = rx.try_recv().unwrap();
            assert!(matches!(received, crate::TokenChunk::Text(t) if t == "Hello"));
        }

        #[test]
        fn chat_stream_tool_call_accumulation_and_normalization() {
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            // First chunk: tool_call id + name arrive
            let chunk1 = serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_abc",
                            "function": {"name": "bash", "arguments": ""}
                        }]
                    }
                }]
            })
            .to_string();
            // Second chunk: arguments fragment
            let chunk2 = serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": {"arguments": "{\"cmd\":\"ls\"}"}
                        }]
                    }
                }]
            })
            .to_string();
            acc.process_event(&chunk1, &tx).unwrap();
            acc.process_event(&chunk2, &tx).unwrap();
            acc.process_event("[DONE]", &tx).unwrap();

            let resp = acc.into_response().unwrap();
            assert!(!resp.end_turn);
            if let ContentBlock::ToolUse { id, name, input } = &resp.content[0] {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            } else {
                panic!("expected ToolUse content block, got {:?}", resp.content[0]);
            }
        }

        #[test]
        fn chat_stream_tool_call_missing_index_reuses_active_builder() {
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let chunk1 = serde_json::json!({
                "choices": [{"delta": {"tool_calls": [{
                    "id": "call_abc",
                    "function": {"name": "bash", "arguments": ""}
                }]}}]
            })
            .to_string();
            let chunk2 = serde_json::json!({
                "choices": [{"delta": {"tool_calls": [{
                    "function": {"arguments": "{\"cmd\":\"ls\"}"}
                }]}}]
            })
            .to_string();
            acc.process_event(&chunk1, &tx).unwrap();
            acc.process_event(&chunk2, &tx).unwrap();
            acc.process_event("[DONE]", &tx).unwrap();

            let resp = acc.into_response().unwrap();
            assert_eq!(resp.content.len(), 1);
            if let ContentBlock::ToolUse { id, name, input } = &resp.content[0] {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            } else {
                panic!("expected ToolUse content block, got {:?}", resp.content[0]);
            }
        }

        #[test]
        fn chat_stream_inline_error_chunk_classifies_context_exhaustion() {
            use crate::LlmErrorKind;
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let error_chunk = serde_json::json!({
                "error": {"message": "context length exceeded", "code": 400}
            })
            .to_string();
            let err = acc.process_event(&error_chunk, &tx).unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::ContextWindowExceeded);
            assert!(err.message.contains("context length exceeded"));
        }

        #[test]
        fn chat_stream_inline_error_string_code_propagation() {
            use crate::LlmErrorKind;
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let error_chunk = serde_json::json!({
                "error": {"message": "rate limit", "code": "rate_limit_exceeded"}
            })
            .to_string();
            let err = acc.process_event(&error_chunk, &tx).unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::RateLimit);
            assert!(err.message.contains("rate limit"));
            assert!(err.message.contains("rate_limit_exceeded"));
        }

        #[test]
        fn chat_stream_drops_reasoning_and_marker_content() {
            let (tx, mut rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let reasoning_chunk = serde_json::json!({
                "choices": [{"delta": {"content": null, "reasoning_content": "internal"}}]
            })
            .to_string();
            let marker_chunk = serde_json::json!({
                "choices": [{"delta": {"content": "<\n\n"}}]
            })
            .to_string();
            let final_chunk = serde_json::json!({
                "choices": [{"delta": {"content": "Final"}}]
            })
            .to_string();
            acc.process_event(&reasoning_chunk, &tx).unwrap();
            acc.process_event(&marker_chunk, &tx).unwrap();
            acc.process_event(&final_chunk, &tx).unwrap();
            acc.process_event("[DONE]", &tx).unwrap();
            let streamed = rx.try_recv().expect("final text should stream");
            assert!(matches!(streamed, crate::TokenChunk::Text(text) if text == "Final"));
            assert!(
                rx.try_recv().is_err(),
                "reasoning and marker chunks must not stream"
            );
            let resp = acc.into_response().unwrap();
            assert!(matches!(&resp.content[0], ContentBlock::Text { text } if text == "Final"));
        }

        #[test]
        fn chat_stream_length_finish_reason_is_output_limit_exceeded() {
            use crate::LlmErrorKind;
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let length_chunk = serde_json::json!({
                "choices": [{"delta": {}, "finish_reason": "length"}],
                "usage": {"completion_tokens_details": {"reasoning_tokens": 12}}
            })
            .to_string();
            let err = acc.process_event(&length_chunk, &tx).unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::OutputLimitExceeded);
            assert!(err.message.contains("output token limit"));
        }

        #[test]
        fn chat_stream_content_filter_finish_reason_is_error() {
            use crate::LlmErrorKind;
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let chunk = serde_json::json!({
                "choices": [{"delta": {"content": "partial"}, "finish_reason": "content_filter"}]
            })
            .to_string();
            let err = acc.process_event(&chunk, &tx).unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::ContentFilter);
        }

        #[test]
        fn chat_stream_without_terminal_marker_is_invalid_response() {
            use crate::LlmErrorKind;
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let chunk = serde_json::json!({
                "choices": [{"delta": {"content": "partial"}}]
            })
            .to_string();
            acc.process_event(&chunk, &tx).unwrap();
            let err = acc.into_response().unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::InvalidResponse);
            assert!(err.message.contains("terminal finish_reason"));
        }

        #[test]
        fn chat_nonstream_length_finish_reason_is_output_limit_exceeded() {
            use crate::LlmErrorKind;
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: Some("partial".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("length".to_string()),
                }],
                usage: Some(ChatUsage {
                    prompt_tokens: 10,
                    completion_tokens: 7,
                    prompt_tokens_details: ChatPromptTokensDetails::default(),
                    completion_tokens_details: ChatCompletionTokensDetails {
                        reasoning_tokens: Some(3),
                    },
                }),
            };
            let err = normalize_chat_response(resp, "test-model").unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::OutputLimitExceeded);
            assert!(err.message.contains("output token limit"));
        }

        #[test]
        fn chat_nonstream_content_filter_finish_reason_is_content_filter() {
            use crate::LlmErrorKind;
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: Some("partial".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("content_filter".to_string()),
                }],
                usage: None,
            };
            let err = normalize_chat_response(resp, "test-model").unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::ContentFilter);
        }

        #[test]
        fn chat_normalize_stop_finish_reason_unchanged() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: Some("Hello!".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            };
            let result = normalize_chat_response(resp, "test-model").unwrap();
            assert!(result.end_turn);
        }

        #[test]
        fn chat_normalize_tool_calls_finish_reason_unchanged() {
            let resp = ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: None,
                        refusal: None,
                        tool_calls: Some(vec![ChatToolCall {
                            id: "call_1".to_string(),
                            r#type: "function".to_string(),
                            function: ChatFunctionCall {
                                name: "read_file".to_string(),
                                arguments: r#"{"path":"/tmp/foo"}"#.to_string(),
                            },
                        }]),
                    },
                    finish_reason: Some("tool_calls".to_string()),
                }],
                usage: None,
            };
            let result = normalize_chat_response(resp, "test-model").unwrap();
            assert!(!result.end_turn);
        }

        #[test]
        fn chat_usage_reasoning_tokens_defaults_without_breaking_old_responses() {
            let usage = serde_json::from_value::<ChatUsage>(serde_json::json!({
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "prompt_tokens_details": {"cached_tokens": 0}
            }))
            .unwrap();
            assert_eq!(usage.reasoning_tokens(), None);
        }

        #[test]
        fn openai_http_error_extracts_provider_message() {
            use crate::LlmErrorKind;
            let body = r#"{"error":{"message":"invalid input"}}"#;
            let err = openai_http_error(400, "400 Bad Request", body);
            assert_eq!(err.kind, LlmErrorKind::InvalidRequest);
            assert!(err.message.contains("invalid input"));
            assert!(!err.message.contains("{\"error\""));
        }

        #[test]
        fn openai_http_error_classifies_context_length_code() {
            use crate::LlmErrorKind;
            let body =
                r#"{"error":{"message":"too many tokens","code":"context_length_exceeded"}}"#;
            let err = openai_http_error(400, "400 Bad Request", body);
            assert_eq!(err.kind, LlmErrorKind::ContextWindowExceeded);
            assert!(err.message.contains("context_length_exceeded"));
        }

        #[test]
        fn openai_http_error_classifies_numeric_context_length_message() {
            use crate::LlmErrorKind;
            let body = r#"{"error":{"message":"context length exceeded","code":400}}"#;
            let err = openai_http_error(400, "400 Bad Request", body);
            assert_eq!(err.kind, LlmErrorKind::ContextWindowExceeded);
        }

        #[test]
        fn openai_http_error_keeps_unknown_4xx_non_retryable() {
            use crate::LlmErrorKind;
            let body =
                r#"{"error":{"message":"unsupported parameter","code":"unsupported_parameter"}}"#;
            let err = openai_http_error(400, "400 Bad Request", body);
            assert_eq!(err.kind, LlmErrorKind::InvalidRequest);
            assert!(err.message.contains("unsupported parameter"));
        }

        #[test]
        fn openai_http_error_does_not_treat_unsupported_max_tokens_as_context_exhausted() {
            use crate::LlmErrorKind;
            let body = r#"{"error":{"message":"Unsupported parameter: 'max_tokens'. Use 'max_completion_tokens'.","code":"unsupported_parameter"}}"#;
            let err = openai_http_error(400, "400 Bad Request", body);
            assert_eq!(err.kind, LlmErrorKind::InvalidRequest);
            assert!(err.message.contains("Unsupported parameter"));
        }

        #[test]
        fn openai_http_error_preserves_status_specific_fallbacks_for_unknown_codes() {
            use crate::LlmErrorKind;
            let rate = r#"{"error":{"message":"Too many requests","code":"provider_specific"}}"#;
            let err = openai_http_error(429, "429 Too Many Requests", rate);
            assert_eq!(err.kind, LlmErrorKind::RateLimit);

            let auth = r#"{"error":{"message":"Forbidden","code":"provider_specific"}}"#;
            let err = openai_http_error(403, "403 Forbidden", auth);
            assert_eq!(err.kind, LlmErrorKind::Auth);
        }

        #[test]
        fn openai_http_error_prefers_coded_terminal_errors_over_status() {
            use crate::LlmErrorKind;
            let quota = r#"{"error":{"message":"quota exhausted","code":"usage_limit_reached"}}"#;
            let err = openai_http_error(429, "429 Too Many Requests", quota);
            assert_eq!(err.kind, LlmErrorKind::UsageLimitReached);

            let overload = r#"{"error":{"message":"busy","code":"server_is_overloaded"}}"#;
            let err = openai_http_error(503, "503 Service Unavailable", overload);
            assert_eq!(err.kind, LlmErrorKind::ServerOverloaded);
        }

        #[test]
        fn chat_stream_empty_stream_returns_invalid_response() {
            use crate::LlmErrorKind;
            let acc = ChatStreamAccumulator::new();
            let err = acc.into_response().unwrap_err();
            assert_eq!(err.kind, LlmErrorKind::InvalidResponse);
        }

        #[test]
        fn chat_stream_usage_chunk_accumulated() {
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let mut acc = ChatStreamAccumulator::new();
            let text_chunk = serde_json::json!({
                "choices": [{"delta": {"content": "Hi"}}]
            })
            .to_string();
            let usage_chunk = serde_json::json!({
                "choices": [],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 2,
                    "prompt_tokens_details": {"cached_tokens": 80}
                }
            })
            .to_string();
            acc.process_event(&text_chunk, &tx).unwrap();
            acc.process_event(&usage_chunk, &tx).unwrap();
            acc.process_event("[DONE]", &tx).unwrap();
            let resp = acc.into_response().unwrap();
            assert_eq!(resp.usage.input_tokens, 20);
            assert_eq!(resp.usage.output_tokens, 2);
            assert_eq!(resp.usage.cache_read_tokens, 80);
            assert_eq!(resp.usage.context_window_used(), 102);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub fn translate_to_responses_request(
        api_name: &str,
        request: &crate::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, false)
    }

    pub fn translate_to_responses_request_codex(
        api_name: &str,
        request: &crate::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, true)
    }
}
