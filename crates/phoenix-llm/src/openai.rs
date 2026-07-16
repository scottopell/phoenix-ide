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
use futures::{SinkExt, StreamExt};
use reqwest::header::HeaderMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite};

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Determine the full endpoint URL.
/// Priority: `base_url_override` (used as-is) > provider default.
fn resolve_endpoint(base_url_override: Option<&str>) -> String {
    base_url_override.map_or_else(
        || "https://api.openai.com/v1/responses".to_string(),
        std::string::ToString::to_string,
    )
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
        // Non-streaming callers do not consume deltas. Close the receiver so
        // awaited provider sends fail immediately instead of filling a bounded
        // channel and deadlocking before the terminal response.
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(chunk_rx);
        return complete_streaming(
            spec,
            api_key,
            base_url_override,
            custom_headers,
            request_tags,
            request,
            &chunk_tx,
            use_codex_backend,
            None,
        )
        .await;
    }

    let url = resolve_endpoint(base_url_override);
    let mut responses_request =
        translate_to_backend_request(&spec.api_name, request, use_codex_backend);
    responses_request.set_tags(request_tags);

    let client = Client::builder()
        .timeout(Duration::from_mins(5))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if use_codex_backend && supports_responses_lite(&spec.api_name) {
        builder = builder.header("x-openai-internal-codex-responses-lite", "true");
    }
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

    if lower.contains("rate_limit") || lower.contains("quota") || lower.contains("requests_per") {
        LlmError::rate_limit(detail)
    } else if lower.contains("auth")
        || lower.contains("invalid_api_key")
        || lower.contains("permission")
    {
        LlmError::auth(detail)
    } else if lower.contains("context_length")
        || lower.contains("token_limit")
        || lower.contains("max_tokens")
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
    /// Cached-read subset of `input_tokens`.
    cached_tokens: u32,
    /// Cache-write subset of `input_tokens` on GPT-5.6-era models.
    cache_write_tokens: u32,
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
            cache_write_tokens: 0,
            output_items: Vec::new(),
            done: false,
            logged_empty_dispatch: false,
        }
    }

    #[allow(clippy::too_many_lines)] // dispatch table; each arm is small
    async fn process_event(
        &mut self,
        event_type: &str,
        data: &str,
        emit: &tokio::sync::mpsc::Sender<super::TokenChunk>,
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
                        let _ = emit.send(super::TokenChunk::Text(delta.to_string())).await;
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
                    self.cached_tokens = u32::try_from(
                        usage
                            .pointer("/input_tokens_details/cached_tokens")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    )
                    .unwrap_or(0);
                    self.cache_write_tokens = u32::try_from(
                        usage
                            .pointer("/input_tokens_details/cache_write_tokens")
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

    fn output_items_as_values(&self) -> Vec<serde_json::Value> {
        self.output_items
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok())
            .collect()
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
                    cache_write_tokens: self.cache_write_tokens,
                },
            },
        })
    }
}

type CodexSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const CODEX_WS_MAX_SESSIONS: usize = 32;
const CODEX_WS_IDLE_TTL: Duration = Duration::from_secs(10 * 60);
#[cfg(not(test))]
const CODEX_WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const CODEX_WS_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
const CODEX_WS_COOLDOWN_BASE: Duration = Duration::from_secs(1);
const CODEX_WS_COOLDOWN_MAX: Duration = Duration::from_secs(5 * 60);
#[cfg(not(test))]
const CODEX_WS_FRAME_TIMEOUT: Duration = Duration::from_secs(10 * 60);
#[cfg(test)]
const CODEX_WS_FRAME_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
pub(crate) struct CodexWsSessions {
    /// The outer mutex protects entry creation and bounded eviction. Each cache
    /// cohort has its own mutex, so unrelated conversations remain concurrent.
    by_cache_key: HashMap<String, Arc<CodexWsSessionEntry>>,
    /// Transport health belongs to the shared Codex endpoint pool, not to a
    /// prompt-cache cohort. A failed endpoint must be avoided by new conversations too.
    cooldown: std::sync::Mutex<CodexWsCooldown>,
}

#[derive(Debug)]
struct CodexWsSessionEntry {
    session: Mutex<CodexWsSession>,
    /// Set synchronously before an attempt touches a socket. A dropped future
    /// cannot run async cleanup, so its Drop guard leaves this marker set for
    /// the next acquisition to discard the potentially misaligned socket.
    dirty: AtomicBool,
    last_used: std::sync::Mutex<Instant>,
}

impl Default for CodexWsSessionEntry {
    fn default() -> Self {
        Self {
            session: Mutex::new(CodexWsSession::default()),
            dirty: AtomicBool::new(false),
            last_used: std::sync::Mutex::new(Instant::now()),
        }
    }
}

struct AttemptMarker {
    entry: Arc<CodexWsSessionEntry>,
    finished: bool,
}

impl AttemptMarker {
    fn begin(entry: Arc<CodexWsSessionEntry>) -> Self {
        entry.dirty.store(true, Ordering::Release);
        Self {
            entry,
            finished: false,
        }
    }

    fn finish(mut self) {
        self.entry.dirty.store(false, Ordering::Release);
        self.finished = true;
    }
}

impl Drop for AttemptMarker {
    fn drop(&mut self) {
        if !self.finished {
            // Intentionally synchronous: cancellation can occur at every await.
            self.entry.dirty.store(true, Ordering::Release);
        }
    }
}

#[derive(Debug, Default)]
struct CodexWsCooldown {
    consecutive_failures: u32,
    retry_at: Option<Instant>,
}

impl CodexWsCooldown {
    fn is_active(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|retry_at| now < retry_at)
    }

    fn record_transport_failure(&mut self, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let shift = self.consecutive_failures.saturating_sub(1).min(31);
        let multiplier = 1_u32 << shift;
        let delay = CODEX_WS_COOLDOWN_BASE
            .saturating_mul(multiplier)
            .min(CODEX_WS_COOLDOWN_MAX);
        self.retry_at = Some(now + delay);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
struct CodexWsSession {
    socket: Option<CodexSocket>,
    response_id: Option<String>,
    compatibility: Option<serde_json::Value>,
    prefix: Vec<serde_json::Value>,
    connection_identity: Option<[u8; 32]>,
}

fn reset_ws_session(session: &mut CodexWsSession) {
    session.socket = None;
    session.response_id = None;
    session.compatibility = None;
    session.prefix.clear();
}

#[derive(Debug)]
enum CodexWsError {
    Fallback(LlmError),
    Interrupted(LlmError),
    Cooldown,
    Backend(LlmError),
    Reconnect(LlmError),
}

impl CodexWsError {
    fn fallback(error: LlmError) -> Self {
        Self::Fallback(error)
    }
    fn backend(error: LlmError) -> Self {
        Self::Backend(error)
    }
    fn reconnect(error: LlmError) -> Self {
        Self::Reconnect(error)
    }
}

fn websocket_url(http_url: &str) -> Result<String, LlmError> {
    if let Some(rest) = http_url.strip_prefix("https://") {
        Ok(format!("wss://{rest}"))
    } else if let Some(rest) = http_url.strip_prefix("http://") {
        Ok(format!("ws://{rest}"))
    } else {
        Err(LlmError::invalid_request(
            "Responses WebSocket URL must be HTTP(S)",
        ))
    }
}

/// Split a fully typed request into the complete input and an exact fingerprint
/// of every other serialized request property. Adding a field to the request
/// type automatically adds it to this fingerprint; only `input` and the
/// continuation-only `previous_response_id` are excluded.
fn continuation_parts(
    request: &ResponsesBackendRequest,
) -> Result<(serde_json::Value, Vec<serde_json::Value>), LlmError> {
    let mut value = serde_json::to_value(request)
        .map_err(|e| LlmError::invalid_request(format!("serialize WebSocket request: {e}")))?;
    let input = value
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
        .map(std::mem::take)
        .ok_or_else(|| LlmError::invalid_request("Responses request has no input array"))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("previous_response_id");
    }
    Ok((value, input))
}

fn canonical_server_output(output: Vec<serde_json::Value>) -> Option<Vec<serde_json::Value>> {
    output
        .into_iter()
        .map(
            |item| match item.get("type").and_then(serde_json::Value::as_str) {
                Some("message") => {
                    let text = item
                        .get("content")?
                        .as_array()?
                        .iter()
                        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(serde_json::json!({"type":"message", "role":"assistant", "content":text}))
                }
                Some("function_call") => Some(serde_json::json!({
                    "type":"function_call",
                    "call_id":item.get("call_id")?,
                    "name":item.get("name")?,
                    "arguments":item.get("arguments")?
                })),
                unsupported => {
                    tracing::debug!(output_type = ?unsupported,
                    "disabling Codex WebSocket continuation: server output is not representable");
                    None
                }
            },
        )
        .collect()
}

fn continuation_suffix(
    old: &CodexWsSession,
    compatibility: &serde_json::Value,
    full_input: &[serde_json::Value],
) -> Option<(String, Vec<serde_json::Value>)> {
    (old.compatibility.as_ref() == Some(compatibility) && full_input.starts_with(&old.prefix))
        .then(|| {
            old.response_id
                .as_ref()
                .map(|id| (id.clone(), full_input[old.prefix.len()..].to_vec()))
        })
        .flatten()
}

fn connection_identity(api_key: &str, custom_headers: &[(String, String)]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut canonical = custom_headers
        .iter()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("authorization") && !name.eq_ignore_ascii_case("openai-beta")
        })
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<Vec<_>>();
    canonical.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(api_key.as_bytes());
    for (name, value) in canonical {
        hash.update([0]);
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(value.as_bytes());
    }
    hash.finalize().into()
}

fn is_websocket_connection_limit(value: &serde_json::Value) -> bool {
    value.get("type").and_then(serde_json::Value::as_str) == Some("error")
        && value
            .pointer("/error/code")
            .or_else(|| value.pointer("/error/type"))
            .or_else(|| value.get("code"))
            .and_then(serde_json::Value::as_str)
            == Some("websocket_connection_limit_reached")
}

fn parse_wrapped_codex_websocket_error(value: &serde_json::Value) -> Option<LlmError> {
    if value.get("type").and_then(serde_json::Value::as_str) != Some("error") {
        return None;
    }
    let status = value
        .get("status")
        .or_else(|| value.get("status_code"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .unwrap_or_else(
            || match value.get("code").and_then(serde_json::Value::as_str) {
                Some("websocket_connection_limit_reached" | "rate_limit_exceeded") => 429,
                Some("context_length_exceeded" | "max_tokens") => 400,
                _ => 500,
            },
        );
    let error = value.get("error").unwrap_or(value);
    if std::ptr::eq(error, value) {
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown_error");
        let message = value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(code);
        return Some(classify_responses_error(code, message));
    }
    let body = serde_json::json!({ "error": error }).to_string();
    let mut headers = HeaderMap::new();
    if let Some(raw_headers) = value.get("headers").and_then(serde_json::Value::as_object) {
        for (name, raw_value) in raw_headers {
            let Ok(name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let value = match raw_value {
                serde_json::Value::String(value) => value.clone(),
                serde_json::Value::Number(value) => value.to_string(),
                serde_json::Value::Bool(value) => value.to_string(),
                serde_json::Value::Null
                | serde_json::Value::Array(_)
                | serde_json::Value::Object(_) => continue,
            };
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&value) {
                headers.insert(name, value);
            }
        }
    }
    parse_codex_error(status, &headers, &body)
        .or_else(|| Some(LlmError::from_http_status(status, &body)))
}

fn parse_codex_rate_limits(value: &serde_json::Value) -> Option<QuotaDetails> {
    super::rate_limit::quota_from_codex_rate_limit_event(value)
}

fn evict_ws_sessions(pool: &mut CodexWsSessions, now: Instant) {
    pool.by_cache_key.retain(|_, entry| {
        Arc::strong_count(entry) > 1
            || now.duration_since(*entry.last_used.lock().expect("last_used mutex poisoned"))
                < CODEX_WS_IDLE_TTL
    });
    while pool.by_cache_key.len() >= CODEX_WS_MAX_SESSIONS {
        let oldest = pool
            .by_cache_key
            .iter()
            .filter(|(_, entry)| Arc::strong_count(entry) == 1)
            .min_by_key(|(_, entry)| *entry.last_used.lock().expect("last_used mutex poisoned"))
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            pool.by_cache_key.remove(&key);
        } else {
            break;
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn complete_codex_websocket(
    http_url: &str,
    api_key: &str,
    custom_headers: &[(String, String)],
    cache_key: &str,
    full_request: &ResponsesBackendRequest,
    chunk_tx: &tokio::sync::mpsc::Sender<super::TokenChunk>,
    sessions: &Arc<Mutex<CodexWsSessions>>,
) -> Result<LlmResponse, CodexWsError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let (compatibility, full_input) =
        continuation_parts(full_request).map_err(CodexWsError::backend)?;
    let identity = connection_identity(api_key, custom_headers);
    let entry = {
        let mut guard = sessions.lock().await;
        evict_ws_sessions(&mut guard, Instant::now());
        if !guard.by_cache_key.contains_key(cache_key)
            && guard.by_cache_key.len() >= CODEX_WS_MAX_SESSIONS
        {
            return Err(CodexWsError::fallback(LlmError::network(
                "Codex WebSocket session capacity reached",
            )));
        }
        guard
            .by_cache_key
            .entry(cache_key.to_string())
            .or_insert_with(|| Arc::new(CodexWsSessionEntry::default()))
            .clone()
    };
    let mut session = entry.session.lock().await;
    *entry.last_used.lock().expect("last_used mutex poisoned") = Instant::now();
    if sessions
        .lock()
        .await
        .cooldown
        .lock()
        .expect("cooldown mutex poisoned")
        .is_active(Instant::now())
    {
        return Err(CodexWsError::Cooldown);
    }
    if entry.dirty.swap(false, Ordering::AcqRel) {
        reset_ws_session(&mut session);
    }
    if session.connection_identity != Some(identity) {
        reset_ws_session(&mut session);
        session.connection_identity = Some(identity);
    }
    let attempt = AttemptMarker::begin(entry.clone());

    let mut wire = serde_json::to_value(full_request).map_err(|e| {
        CodexWsError::backend(LlmError::invalid_request(format!(
            "serialize WebSocket request: {e}"
        )))
    })?;
    // Responses Lite is selected per create, not only at WebSocket upgrade.
    // Match upstream codex-rs client metadata so every full or incremental
    // request on a reused connection carries its own parsing contract.
    wire["client_metadata"] = serde_json::json!({
        "ws_request_header_x_openai_internal_codex_responses_lite": "true"
    });
    let full_payload_bytes = serde_json::to_vec(&wire).map_or(0, |bytes| bytes.len());
    let incremental = if let Some((previous_response_id, suffix)) =
        continuation_suffix(&session, &compatibility, &full_input)
    {
        wire["input"] = serde_json::Value::Array(suffix);
        wire["previous_response_id"] = serde_json::Value::String(previous_response_id);
        true
    } else {
        false
    };
    let sent_payload_bytes = serde_json::to_vec(&wire).map_or(0, |bytes| bytes.len());
    let connection_reused = session.socket.is_some();
    tracing::debug!(
        cache_key,
        connection_reused,
        incremental,
        full_payload_bytes,
        sent_payload_bytes,
        "sending Codex Responses WebSocket request"
    );
    let mut envelope = wire;
    envelope["type"] = serde_json::Value::String("response.create".to_string());

    let mut upgrade = websocket_url(http_url)
        .map_err(CodexWsError::backend)?
        .into_client_request()
        .map_err(|e| {
            CodexWsError::fallback(LlmError::network(format!("WebSocket request: {e}")))
        })?;
    let headers = upgrade.headers_mut();
    headers.insert(
        "authorization",
        format!("Bearer {api_key}").parse().map_err(|e| {
            CodexWsError::backend(LlmError::invalid_request(format!(
                "authorization header: {e}"
            )))
        })?,
    );
    headers.insert(
        "openai-beta",
        "responses_websockets=2026-02-06"
            .parse()
            .expect("static header"),
    );
    headers.insert(
        "x-openai-internal-codex-responses-lite",
        "true".parse().expect("static header"),
    );
    for (name, value) in custom_headers {
        if name.eq_ignore_ascii_case("openai-beta") || name.eq_ignore_ascii_case("authorization") {
            continue;
        }
        let name = tungstenite::http::HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
            CodexWsError::backend(LlmError::invalid_request(format!(
                "WebSocket header name: {e}"
            )))
        })?;
        let value = tungstenite::http::HeaderValue::from_str(value).map_err(|e| {
            CodexWsError::backend(LlmError::invalid_request(format!(
                "WebSocket header value: {e}"
            )))
        })?;
        headers.insert(name, value);
    }

    // Once text is observable on the public channel, replaying the request over
    // HTTP could duplicate user-visible output. Quota-only frames do not close
    // this fallback window.
    let mut public_output_started = false;

    let result = async {
        if session.socket.is_none() {
            let (socket, _) =
                tokio::time::timeout(CODEX_WS_CONNECT_TIMEOUT, connect_async(upgrade))
                    .await
                    .map_err(|_| {
                        CodexWsError::fallback(LlmError::network(
                            "WebSocket connect/handshake timeout",
                        ))
                    })?
                    .map_err(|e| {
                        CodexWsError::fallback(LlmError::network(format!("WebSocket connect: {e}")))
                    })?;
            session.socket = Some(socket);
        }
        let socket = session.socket.as_mut().expect("socket initialized");
        tokio::time::timeout(
            CODEX_WS_FRAME_TIMEOUT,
            socket.send(tungstenite::Message::Text(envelope.to_string().into())),
        )
        .await
        .map_err(|_| CodexWsError::fallback(LlmError::network("WebSocket send timeout")))?
        .map_err(|e| CodexWsError::fallback(LlmError::network(format!("WebSocket send: {e}"))))?;
        let mut acc = ResponsesStreamAccumulator::new();
        let mut response_id = None;
        let mut server_output = Vec::new();
        loop {
            let message = tokio::time::timeout(CODEX_WS_FRAME_TIMEOUT, socket.next())
                .await
                .map_err(|_| CodexWsError::fallback(LlmError::network("WebSocket frame timeout")))?
                .ok_or_else(|| {
                    CodexWsError::fallback(LlmError::network(
                        "WebSocket closed before terminal event",
                    ))
                })?
                .map_err(|e| {
                    CodexWsError::fallback(LlmError::network(format!("WebSocket stream: {e}")))
                })?;
            let text = match message {
                tungstenite::Message::Text(text) => text,
                tungstenite::Message::Close(_) => break,
                tungstenite::Message::Binary(_)
                | tungstenite::Message::Ping(_)
                | tungstenite::Message::Pong(_)
                | tungstenite::Message::Frame(_) => continue,
            };
            let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                CodexWsError::fallback(LlmError::invalid_response(format!(
                    "WebSocket event JSON: {e}"
                )))
            })?;
            let event_type = value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if let Some(error) = parse_wrapped_codex_websocket_error(&value) {
                return Err(if is_websocket_connection_limit(&value) {
                    CodexWsError::reconnect(error)
                } else {
                    CodexWsError::backend(error)
                });
            }
            if event_type == "codex.rate_limits" {
                if let Some(snapshot) = parse_codex_rate_limits(&value) {
                    let _ = chunk_tx
                        .send(super::TokenChunk::RateLimitSnapshot(snapshot))
                        .await;
                }
                continue;
            }
            acc.process_event(event_type, &text, chunk_tx)
                .await
                .map_err(CodexWsError::backend)?;
            if event_type == "response.output_text.delta"
                && value
                    .get("delta")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|delta| !delta.is_empty())
            {
                public_output_started = true;
            }
            if event_type == "response.completed" {
                response_id = value
                    .pointer("/response/id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                // The accumulator collects `response.output_item.done` items
                // and also falls back to terminal `/response/output`. Derive
                // continuation from that single authoritative collection so an
                // omitted terminal output cannot make us resend prior output.
                server_output = acc.output_items_as_values();
            }
            if acc.done {
                break;
            }
        }
        let id = response_id.ok_or_else(|| {
            CodexWsError::fallback(LlmError::invalid_response(
                "WebSocket completed without response id",
            ))
        })?;
        let response = acc.into_response().map_err(CodexWsError::backend)?;
        Ok::<_, CodexWsError>((response, id, server_output))
    }
    .await;

    match result {
        Ok((response, response_id, server_output)) => {
            sessions
                .lock()
                .await
                .cooldown
                .lock()
                .expect("cooldown mutex poisoned")
                .reset();
            if let Some(canonical_output) = canonical_server_output(server_output) {
                let mut prefix = full_input;
                prefix.extend(canonical_output);
                session.response_id = Some(response_id);
                session.compatibility = Some(compatibility);
                session.prefix = prefix;
            } else {
                // Keep the healthy socket, but a future create must be full:
                // Phoenix cannot prove prefix equivalence for hidden output.
                session.response_id = None;
                session.compatibility = None;
                session.prefix.clear();
            }
            attempt.finish();
            Ok(response)
        }
        Err(mut error) => {
            if public_output_started {
                error = match error {
                    CodexWsError::Fallback(transport) => {
                        CodexWsError::Interrupted(LlmError::network(format!(
                            "Codex WebSocket interrupted after public output: {}",
                            transport.message
                        )))
                    }
                    CodexWsError::Reconnect(transport) => {
                        CodexWsError::Interrupted(LlmError::network(format!(
                            "Codex WebSocket expired after public output: {}",
                            transport.message
                        )))
                    }
                    other @ (CodexWsError::Interrupted(_)
                    | CodexWsError::Cooldown
                    | CodexWsError::Backend(_)) => other,
                };
            }
            // A protocol or transport failure poisons the stream. Reconnect on
            // the next request and require a full create; never continue from
            // metadata whose terminal response was not observed.
            reset_ws_session(&mut session);
            if matches!(
                error,
                CodexWsError::Fallback(_) | CodexWsError::Interrupted(_)
            ) {
                sessions
                    .lock()
                    .await
                    .cooldown
                    .lock()
                    .expect("cooldown mutex poisoned")
                    .record_transport_failure(Instant::now());
            }
            attempt.finish();
            Err(error)
        }
    }
}

/// Complete with streaming, emitting `TokenChunk::Text` events via `chunk_tx`.
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub async fn complete_streaming(
    spec: &ModelSpec,
    api_key: &str,
    base_url_override: Option<&str>,
    custom_headers: &[(String, String)],
    request_tags: &BTreeMap<String, String>,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::mpsc::Sender<super::TokenChunk>,
    use_codex_backend: bool,
    ws_sessions: Option<&Arc<Mutex<CodexWsSessions>>>,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_endpoint(base_url_override);
    let mut responses_request =
        translate_to_backend_request(&spec.api_name, request, use_codex_backend);
    responses_request.set_streaming();
    responses_request.set_tags(request_tags);

    if use_codex_backend && supports_responses_lite(&spec.api_name) {
        if let Some(sessions) = ws_sessions {
            match complete_codex_websocket(
                &url,
                api_key,
                custom_headers,
                request.cache_key.as_str(),
                &responses_request,
                chunk_tx,
                sessions,
            )
            .await
            {
                Ok(response) => return Ok(response),
                Err(CodexWsError::Cooldown) => tracing::debug!(
                    cache_key = request.cache_key.as_str(),
                    "Codex WebSocket transport cooldown active; using HTTP/SSE"
                ),
                Err(CodexWsError::Backend(error) | CodexWsError::Interrupted(error)) => {
                    return Err(error);
                }
                Err(CodexWsError::Reconnect(error)) => {
                    tracing::debug!(error = %error.message,
                        "Codex WebSocket lifetime exhausted; retrying once on a fresh socket");
                    match complete_codex_websocket(
                        &url,
                        api_key,
                        custom_headers,
                        request.cache_key.as_str(),
                        &responses_request,
                        chunk_tx,
                        sessions,
                    )
                    .await
                    {
                        Ok(response) => return Ok(response),
                        Err(
                            CodexWsError::Backend(error)
                            | CodexWsError::Interrupted(error)
                            | CodexWsError::Reconnect(error),
                        ) => return Err(error),
                        Err(CodexWsError::Cooldown) => tracing::debug!(
                            "Codex WebSocket cooldown became active; using HTTP/SSE"
                        ),
                        Err(CodexWsError::Fallback(error)) => {
                            tracing::warn!(error = %error.message,
                                "fresh Codex WebSocket failed; falling back once to full HTTP/SSE");
                        }
                    }
                }
                Err(CodexWsError::Fallback(error)) => tracing::warn!(error = %error.message,
                    "Codex WebSocket transport/protocol failed; falling back once to full HTTP/SSE"),
            }
        }
    }

    let client = Client::builder()
        .timeout(Duration::from_mins(10))
        .build()
        .map_err(|e| LlmError::network(format!("Failed to create HTTP client: {e}")))?;

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if use_codex_backend && supports_responses_lite(&spec.api_name) {
        builder = builder.header("x-openai-internal-codex-responses-lite", "true");
    }
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
            let _ = chunk_tx
                .send(super::TokenChunk::RateLimitSnapshot(snapshot))
                .await;
        }
    }

    let mut acc = ResponsesStreamAccumulator::new();
    let mut sse = super::sse::SseParser::new();
    let mut stream = response.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        for event in sse.push(&chunk) {
            if let Err(e) = acc
                .process_event(&event.event_type, &event.data, chunk_tx)
                .await
            {
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
        acc.process_event(&event.event_type, &event.data, chunk_tx)
            .await?;
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
                let mut parts: Vec<ResponsesApiMessagePart> = text_blocks
                    .iter()
                    .map(|t| ResponsesApiMessagePart::InputText {
                        text: (*t).to_string(),
                        prompt_cache_breakpoint: None,
                    })
                    .collect();
                for source in &image_blocks {
                    let ImageSource::Base64 { media_type, data } = source;
                    parts.push(ResponsesApiMessagePart::InputImage {
                        image_url: format!("data:{media_type};base64,{data}"),
                        prompt_cache_breakpoint: None,
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
                    let mut parts = vec![ResponsesApiFunctionOutputPart::InputText { text }];
                    for img in images {
                        let ImageSource::Base64 { media_type, data } = img;
                        parts.push(ResponsesApiFunctionOutputPart::InputImage {
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

    let explicit_cache_supported = !use_codex_backend && supports_explicit_prompt_cache(api_name);
    if explicit_cache_supported {
        place_explicit_cache_breakpoints(&mut input_items);
    }

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
        prompt_cache_options: explicit_cache_supported.then_some(PromptCacheOptions {
            mode: PromptCacheMode::Implicit,
            ttl: PromptCacheTtl::ThirtyMinutes,
        }),
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

fn translate_to_backend_request(
    api_name: &str,
    request: &LlmRequest,
    use_codex_backend: bool,
) -> ResponsesBackendRequest {
    let platform = translate_to_responses_request(api_name, request, use_codex_backend);
    if use_codex_backend && supports_responses_lite(api_name) {
        ResponsesBackendRequest::CodexLite(CodexResponsesLiteRequest::from_platform(platform))
    } else {
        ResponsesBackendRequest::Platform(platform)
    }
}

fn supports_responses_lite(api_name: &str) -> bool {
    api_name == "gpt-5.6" || api_name.starts_with("gpt-5.6-")
}

fn supports_explicit_prompt_cache(api_name: &str) -> bool {
    api_name == "gpt-5.6" || api_name.starts_with("gpt-5.6-")
}

/// Preserve `OpenAI`'s historical read boundaries while leaving the latest
/// message to implicit mode. The service considers the latest 50 explicit
/// markers for reads and decides which newest markers consume its write slots.
fn place_explicit_cache_breakpoints(items: &mut [ResponsesApiInputItem]) {
    const READ_BREAKPOINT_LIMIT: usize = 50;

    let mut messages = items.iter_mut().filter_map(|item| match item {
        ResponsesApiInputItem::Message { role, content } => Some((role, content)),
        ResponsesApiInputItem::AdditionalTools { .. }
        | ResponsesApiInputItem::FunctionCall { .. }
        | ResponsesApiInputItem::FunctionCallOutput { .. } => None,
    });
    let Some(_implicit_latest_message) = messages.next_back() else {
        return;
    };
    for (role, content) in messages.rev().take(READ_BREAKPOINT_LIMIT) {
        // An explicit cache marker turns text content into an `input_text`
        // part. That discriminant is only valid on input-role messages; the
        // Responses API rejects it on an assistant message, whose parts must
        // be `output_text`/`refusal`. Assistant turns are model output — leave
        // them a plain string and mark only input-role history.
        if role == "assistant" {
            continue;
        }
        content.mark_last_block();
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
            // Both detail buckets are subsets of OpenAI's inclusive
            // `input_tokens`. Split them out so Phoenix's additive Usage shape
            // preserves the provider-reported context total.
            let cached = u64::from(resp.usage.input_tokens_details.cached_tokens);
            let written = u64::from(resp.usage.input_tokens_details.cache_write_tokens);
            Usage {
                input_tokens: u64::from(resp.usage.input_tokens)
                    .saturating_sub(cached.saturating_add(written)),
                output_tokens: u64::from(resp.usage.output_tokens),
                cache_creation_tokens: written,
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
    code: Option<String>,
}

// Responses API types (for codex models)

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ResponsesBackendRequest {
    Platform(ResponsesApiRequest),
    CodexLite(CodexResponsesLiteRequest),
}

impl ResponsesBackendRequest {
    fn set_streaming(&mut self) {
        match self {
            Self::Platform(request) => request.stream = Some(true),
            Self::CodexLite(request) => request.stream = Some(true),
        }
    }

    fn set_tags(&mut self, tags: &BTreeMap<String, String>) {
        if tags.is_empty() {
            return;
        }
        match self {
            Self::Platform(request) => request.tags = Some(tags.clone()),
            Self::CodexLite(request) => request.tags = Some(tags.clone()),
        }
    }
}

/// ChatGPT-backend Responses Lite wire shape. Unlike the platform type, this
/// type cannot represent top-level instructions/tools or explicit cache policy.
#[derive(Debug, Serialize)]
struct CodexResponsesLiteRequest {
    model: String,
    input: Vec<ResponsesApiInputItem>,
    store: bool,
    prompt_cache_key: String,
    parallel_tool_calls: bool,
    tool_choice: CodexResponsesLiteToolChoice,
    reasoning: CodexResponsesLiteReasoning,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CodexResponsesLiteToolChoice {
    Auto,
}

#[derive(Debug, Serialize)]
struct CodexResponsesLiteReasoning {
    context: CodexReasoningContext,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CodexReasoningContext {
    AllTurns,
}

impl CodexResponsesLiteRequest {
    fn from_platform(mut request: ResponsesApiRequest) -> Self {
        let tools = request.tools.take().unwrap_or_default();
        let instructions = request
            .instructions
            .take()
            .unwrap_or_else(|| "You are a helpful assistant.".to_string());
        let mut input = Vec::with_capacity(request.input.len() + 2);
        input.push(ResponsesApiInputItem::AdditionalTools {
            role: "developer".to_string(),
            tools,
        });
        input.push(ResponsesApiInputItem::Message {
            role: "developer".to_string(),
            content: ResponsesApiMessageContent::Parts(vec![ResponsesApiMessagePart::InputText {
                text: instructions,
                prompt_cache_breakpoint: None,
            }]),
        });
        input.append(&mut request.input);
        Self {
            model: request.model,
            input,
            store: false,
            prompt_cache_key: request
                .prompt_cache_key
                .expect("LlmRequest always supplies a prompt cache key"),
            parallel_tool_calls: false,
            tool_choice: CodexResponsesLiteToolChoice::Auto,
            reasoning: CodexResponsesLiteReasoning {
                context: CodexReasoningContext::AllTurns,
            },
            stream: request.stream,
            tags: request.tags,
        }
    }
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
    /// GPT-5.6-era request-wide cache policy. Omitted for older models and
    /// the ChatGPT/Codex bridge, which reject the new platform fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_cache_options: Option<PromptCacheOptions>,
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
pub(crate) struct PromptCacheOptions {
    mode: PromptCacheMode,
    ttl: PromptCacheTtl,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PromptCacheMode {
    Implicit,
}

#[derive(Debug, Serialize)]
enum PromptCacheTtl {
    #[serde(rename = "30m")]
    ThirtyMinutes,
}

#[derive(Debug, Serialize)]
pub(crate) struct PromptCacheBreakpoint {
    mode: PromptCacheBreakpointMode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PromptCacheBreakpointMode {
    Explicit,
}

impl PromptCacheBreakpoint {
    fn explicit() -> Self {
        Self {
            mode: PromptCacheBreakpointMode::Explicit,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum ResponsesApiInputItem {
    #[serde(rename = "additional_tools")]
    AdditionalTools {
        role: String,
        tools: Vec<ResponsesApiTool>,
    },
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
    Parts(Vec<ResponsesApiMessagePart>),
}

impl ResponsesApiMessageContent {
    fn mark_last_block(&mut self) {
        match self {
            Self::Text(text) => {
                *self = Self::Parts(vec![ResponsesApiMessagePart::InputText {
                    text: std::mem::take(text),
                    prompt_cache_breakpoint: Some(PromptCacheBreakpoint::explicit()),
                }]);
            }
            Self::Parts(parts) => {
                if let Some(part) = parts.last_mut() {
                    part.set_breakpoint();
                }
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesApiMessagePart {
    InputText {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_breakpoint: Option<PromptCacheBreakpoint>,
    },
    InputImage {
        image_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_breakpoint: Option<PromptCacheBreakpoint>,
    }, // "data:{media_type};base64,{data}"
}

impl ResponsesApiMessagePart {
    fn set_breakpoint(&mut self) {
        match self {
            Self::InputText {
                prompt_cache_breakpoint,
                ..
            }
            | Self::InputImage {
                prompt_cache_breakpoint,
                ..
            } => *prompt_cache_breakpoint = Some(PromptCacheBreakpoint::explicit()),
        }
    }
}

/// Function call output: plain string when text-only, array of parts when images present.
///
/// The Responses API treats a `function_call_output` payload as model *input*,
/// so its content parts use the same `input_text`/`input_image` discriminants as
/// `ResponsesApiFunctionOutputPart` — not `text`/`image_url`, which the API
/// rejects. This surface-specific type intentionally cannot represent a cache
/// breakpoint, which `OpenAI` rejects on function-call outputs.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesApiFunctionOutput {
    Text(String),
    Parts(Vec<ResponsesApiFunctionOutputPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesApiFunctionOutputPart {
    InputText { text: String },
    InputImage { image_url: String },
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesApiTool {
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

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ResponsesApiOutput {
    pub(crate) r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<Vec<ResponsesApiContent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) arguments: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) call_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ResponsesApiContent {
    pub(crate) r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refusal: Option<String>,
}

/// `usage.input_tokens_details` on the Responses API wire. Detail buckets
/// default to zero for older models and gateways that omit them.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ResponsesApiInputTokensDetails {
    #[serde(default)]
    pub(crate) cached_tokens: u32,
    #[serde(default)]
    pub(crate) cache_write_tokens: u32,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::has_custom_source_header;
    use crate::types::{LlmMessage, LlmRequest, PromptCacheKey};

    use crate::models::{ModelBackend, ModelSource};
    use crate::types::MessageRole;
    use axum::extract::{ws::Message as AxumWsMessage, State, WebSocketUpgrade};
    use axum::http::HeaderMap as AxumHeaderMap;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Default)]
    struct MockResponsesState {
        connections: Arc<AtomicUsize>,
        http_requests: Arc<AtomicUsize>,
        ws_requests: Arc<Mutex<Vec<(usize, serde_json::Value)>>>,
        ws_headers: Arc<Mutex<Vec<AxumHeaderMap>>>,
    }

    #[allow(clippy::too_many_lines)]
    async fn mock_ws(
        ws: WebSocketUpgrade,
        headers: AxumHeaderMap,
        State(state): State<MockResponsesState>,
    ) -> Response {
        state.ws_headers.lock().await.push(headers);
        let connection = state.connections.fetch_add(1, Ordering::SeqCst);
        ws.on_upgrade(move |mut socket| async move {
            while let Some(Ok(AxumWsMessage::Text(text))) = socket.recv().await {
                let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                state.ws_requests.lock().await.push((connection, request.clone()));
                assert_eq!(request["type"], "response.create");
                assert!(request.get("response").is_none());
                assert!(request["model"].is_string());
                assert_eq!(request["tool_choice"], "auto");
                assert_eq!(
                    request["client_metadata"]
                        ["ws_request_header_x_openai_internal_codex_responses_lite"],
                    "true"
                );
                let marker = request["input"].to_string();
                if marker.contains("connection-limit") && connection == 0 {
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type": "error",
                        "status": 429,
                        "error": {
                            "type": "websocket_connection_limit_reached",
                            "code": "websocket_connection_limit_reached",
                            "message": "connection lifetime exhausted"
                        }
                    }).to_string())).await.unwrap();
                    continue;
                }
                if marker.contains("ws-fail") {
                    socket
                        .send(AxumWsMessage::Text(
                            serde_json::json!({"type":"response.output_text.delta","delta":"speculative"})
                                .to_string(),
                        ))
                        .await
                        .unwrap();
                    return;
                }
                if marker.contains("stall") {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
                if marker.contains("rate-error") {
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type":"error", "code":"rate_limit_exceeded", "message":"slow down"
                    }).to_string())).await.unwrap();
                    continue;
                }
                if marker.contains("rate-snapshot") {
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type":"codex.rate_limits",
                        "rate_limits":{"plan_type":"plus","resets_at":null,"limit_id":"codex","limit_name":null,
                        "primary":{"used_percent":42.0,"window_minutes":60,"resets_at":1_700_000_000},
                        "secondary":null,"credits":null,"promo_message":null}
                    }).to_string())).await.unwrap();
                }
                if marker.contains("many-deltas") {
                    for i in 0..1_300 {
                        socket.send(AxumWsMessage::Text(serde_json::json!({
                            "type":"response.output_text.delta", "delta":format!("{i},")
                        }).to_string())).await.unwrap();
                    }
                }
                if marker.contains("terminal") {
                    socket
                        .send(AxumWsMessage::Text(
                            serde_json::json!({"type":"error","code":"context_length_exceeded","message":"too long"})
                                .to_string(),
                        ))
                        .await
                        .unwrap();
                    continue;
                }
                let n = state.ws_requests.lock().await.len();
                let answer = format!("answer-{n}");
                if marker.contains("unsupported-output") {
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type":"response.completed",
                        "response":{
                            "id":format!("resp-{n}"),
                            "usage":{"input_tokens":10,"output_tokens":1},
                            "output":[
                                {"type":"reasoning","id":"reasoning-1","summary":[]},
                                {"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}
                            ]
                        }
                    }).to_string())).await.unwrap();
                    continue;
                }
                if marker.contains("item-done-only") {
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type":"response.output_item.done",
                        "item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}
                    }).to_string())).await.unwrap();
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type":"response.completed",
                        "response":{"id":format!("resp-{n}"),"usage":{"input_tokens":10,"output_tokens":1}}
                    }).to_string())).await.unwrap();
                    continue;
                }
                socket
                    .send(AxumWsMessage::Text(
                        serde_json::json!({
                            "type":"response.completed",
                            "response":{
                                "id":format!("resp-{n}"),
                                "usage":{"input_tokens":10,"output_tokens":1},
                                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}]
                            }
                        })
                        .to_string(),
                    ))
                    .await
                    .unwrap();
            }
        })
    }

    async fn mock_http(State(state): State<MockResponsesState>) -> impl IntoResponse {
        state.http_requests.fetch_add(1, Ordering::SeqCst);
        (
            [("content-type", "text/event-stream")],
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"http-1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1},\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"http-answer\"}]}]}}\n\n",
        )
    }

    async fn mock_server() -> (String, MockResponsesState) {
        let state = MockResponsesState::default();
        let app = Router::new()
            .route("/responses", get(mock_ws).post(mock_http))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/responses"), state)
    }

    fn codex_spec() -> ModelSpec {
        ModelSpec {
            id: "gpt-5.6".into(),
            api_name: "gpt-5.6".into(),
            backend: ModelBackend::OpenAIResponses,
            description: String::new(),
            context_window: 100_000,
            recommended: false,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
        }
    }

    fn request_with(messages: &[(&str, MessageRole)]) -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: messages
                .iter()
                .map(|(text, role)| LlmMessage {
                    role: *role,
                    content: vec![ContentBlock::text(*text)],
                })
                .collect(),
            tools: vec![],
            max_tokens: None,
            cache_key: PromptCacheKey::stable("integration"),
        }
    }

    #[test]
    fn wrapped_websocket_usage_limit_preserves_quota_headers() {
        let error = parse_wrapped_codex_websocket_error(&serde_json::json!({
            "type": "error",
            "status": 429,
            "error": {
                "type": "usage_limit_reached",
                "message": "The usage limit has been reached",
                "plan_type": "pro",
                "resets_at": 1_738_888_888
            },
            "headers": {
                "x-codex-primary-used-percent": "100.0",
                "x-codex-primary-window-minutes": 15
            }
        }))
        .expect("wrapped error maps");
        assert_eq!(error.kind, crate::LlmErrorKind::UsageLimitReached);
        let quota = error.quota.expect("quota details");
        assert_eq!(quota.plan_type.as_deref(), Some("pro"));
        assert_eq!(quota.primary.as_ref().map(|w| w.used_percent), Some(100.0));
        assert_eq!(
            quota.primary.as_ref().and_then(|w| w.window_minutes),
            Some(15)
        );
    }

    #[test]
    fn flat_websocket_connection_limit_requests_reconnect() {
        let value = serde_json::json!({
            "type": "error",
            "code": "websocket_connection_limit_reached",
            "message": "connection lifetime exhausted"
        });
        assert!(is_websocket_connection_limit(&value));
        assert!(parse_wrapped_codex_websocket_error(&value).is_some());
    }

    #[test]
    fn wrapped_websocket_status_code_alias_maps_invalid_request() {
        let error = parse_wrapped_codex_websocket_error(&serde_json::json!({
            "type": "error",
            "status_code": 400,
            "error": {
                "type": "invalid_request_error",
                "message": "unsupported input"
            }
        }))
        .expect("wrapped error maps");
        assert_eq!(error.kind, crate::LlmErrorKind::InvalidRequest);
    }

    #[tokio::test]
    async fn websocket_reuses_connection_continues_resets_and_falls_back_safely() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let call = |request: LlmRequest| {
            let url = url.clone();
            let sessions = sessions.clone();
            let tx = tx.clone();
            async move {
                complete_streaming(
                    &codex_spec(),
                    "account-a",
                    Some(&url),
                    &[],
                    &BTreeMap::new(),
                    &request,
                    &tx,
                    true,
                    Some(&sessions),
                )
                .await
            }
        };

        call(request_with(&[("one", MessageRole::User)]))
            .await
            .unwrap();
        call(request_with(&[
            ("one", MessageRole::User),
            ("answer-1", MessageRole::Assistant),
            ("two", MessageRole::User),
        ]))
        .await
        .unwrap();
        let requests = state.ws_requests.lock().await.clone();
        assert_eq!(state.connections.load(Ordering::SeqCst), 1);
        assert!(requests[0].1.get("previous_response_id").is_none());
        assert_eq!(requests[1].1["previous_response_id"], "resp-1");
        assert_eq!(
            requests[0].1["client_metadata"]
                ["ws_request_header_x_openai_internal_codex_responses_lite"],
            "true"
        );
        assert_eq!(
            requests[1].1["client_metadata"]
                ["ws_request_header_x_openai_internal_codex_responses_lite"],
            "true"
        );
        assert_eq!(requests[1].1["input"].as_array().unwrap().len(), 1);

        // A non-prefix request is a full create, but the healthy transport is retained.
        call(request_with(&[("changed", MessageRole::User)]))
            .await
            .unwrap();
        let requests = state.ws_requests.lock().await.clone();
        assert!(requests[2].1.get("previous_response_id").is_none());
        assert_eq!(state.connections.load(Ordering::SeqCst), 1);

        // A cache-relevant property change also sends a full create on the same
        // live connection rather than applying stale continuation metadata.
        let mut property_changed = request_with(&[("changed", MessageRole::User)]);
        property_changed.max_tokens = Some(42);
        call(property_changed).await.unwrap();
        let requests = state.ws_requests.lock().await.clone();
        assert!(requests[3].1.get("previous_response_id").is_none());
        assert_eq!(state.connections.load(Ordering::SeqCst), 1);

        // Once a text delta is public, transport interruption is returned without
        // replaying the request over HTTP.
        let err = call(request_with(&[("ws-fail", MessageRole::User)]))
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::LlmErrorKind::Network);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 0);
        let chunks: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(chunks
            .iter()
            .any(|c| matches!(c, super::super::TokenChunk::Text(t) if t == "speculative")));
        // The cohort is now cooling down, so the next turn goes straight to
        // HTTP without another WebSocket connection attempt.
        call(request_with(&[("cooldown-skip", MessageRole::User)]))
            .await
            .unwrap();
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 1);
        assert_eq!(state.connections.load(Ordering::SeqCst), 1);

        {
            let pool = sessions.lock().await;
            pool.cooldown.lock().unwrap().reset();
        }
        sessions.lock().await.by_cache_key.clear();
        // Reconnect after failure, but a terminal model error is returned as-is
        // rather than changing semantics by replaying it over HTTP.
        let err = call(request_with(&[("terminal", MessageRole::User)]))
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::LlmErrorKind::ContextWindowExceeded);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 1);
        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn websocket_unsupported_output_disables_continuation_but_keeps_socket() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let call = |request: LlmRequest| {
            let (url, sessions, tx) = (url.clone(), sessions.clone(), tx.clone());
            async move {
                complete_streaming(
                    &codex_spec(),
                    "secret",
                    Some(&url),
                    &[],
                    &BTreeMap::new(),
                    &request,
                    &tx,
                    true,
                    Some(&sessions),
                )
                .await
            }
        };

        call(request_with(&[("unsupported-output", MessageRole::User)]))
            .await
            .unwrap();
        call(request_with(&[
            ("unsupported-output", MessageRole::User),
            ("answer-1", MessageRole::Assistant),
            ("next", MessageRole::User),
        ]))
        .await
        .unwrap();

        assert_eq!(state.connections.load(Ordering::SeqCst), 1);
        let requests = state.ws_requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests[1].1.get("previous_response_id").is_none());
        assert_eq!(requests[1].1["input"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn websocket_connection_limit_reconnects_once_without_http_fallback() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _rx) = tokio::sync::mpsc::channel(8);

        complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &[],
            &BTreeMap::new(),
            &request_with(&[("connection-limit", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();

        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 0);
        let requests = state.ws_requests.lock().await;
        assert!(requests[0].1.get("previous_response_id").is_none());
        assert!(requests[1].1.get("previous_response_id").is_none());
    }

    #[tokio::test]
    async fn websocket_continuation_includes_item_done_output_when_terminal_omits_output() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _) = tokio::sync::mpsc::channel(8);
        let call = |request: LlmRequest| {
            let (url, sessions, tx) = (url.clone(), sessions.clone(), tx.clone());
            async move {
                complete_streaming(
                    &codex_spec(),
                    "secret",
                    Some(&url),
                    &[],
                    &BTreeMap::new(),
                    &request,
                    &tx,
                    true,
                    Some(&sessions),
                )
                .await
            }
        };

        call(request_with(&[("item-done-only", MessageRole::User)]))
            .await
            .unwrap();
        call(request_with(&[
            ("item-done-only", MessageRole::User),
            ("answer-1", MessageRole::Assistant),
            ("genuinely-new", MessageRole::User),
        ]))
        .await
        .unwrap();

        let requests = state.ws_requests.lock().await;
        assert_eq!(requests[1].1["previous_response_id"], "resp-1");
        let suffix = requests[1].1["input"].as_array().unwrap();
        assert_eq!(suffix.len(), 1);
        assert!(suffix[0].to_string().contains("genuinely-new"));
        assert!(!suffix[0].to_string().contains("answer-1"));
    }

    #[tokio::test]
    async fn websocket_replays_more_than_public_capacity_without_loss() {
        let (url, _) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let receiver = tokio::spawn(async move {
            let mut text = String::new();
            while let Some(chunk) = rx.recv().await {
                if let super::super::TokenChunk::Text(delta) = chunk {
                    text.push_str(&delta);
                    tokio::task::yield_now().await;
                }
            }
            text
        });
        complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &[],
            &BTreeMap::new(),
            &request_with(&[("many-deltas", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();
        drop(tx);
        let text = receiver.await.unwrap();
        let expected = (0..1_300).fold(String::new(), |mut output, i| {
            use std::fmt::Write;
            write!(output, "{i},").expect("write to String");
            output
        });
        assert_eq!(text, expected);
    }

    #[tokio::test]
    async fn websocket_cooldown_backoff_skip_expiry_and_reset_are_deterministic() {
        let start = Instant::now();
        let mut cooldown = CodexWsCooldown::default();
        cooldown.record_transport_failure(start);
        assert!(cooldown.is_active(start));
        assert!(!cooldown.is_active(start + CODEX_WS_COOLDOWN_BASE));
        cooldown.record_transport_failure(start + CODEX_WS_COOLDOWN_BASE);
        assert!(cooldown.is_active(start + CODEX_WS_COOLDOWN_BASE * 2));
        assert!(!cooldown.is_active(start + CODEX_WS_COOLDOWN_BASE * 3));
        cooldown.reset();
        assert!(!cooldown.is_active(start));
        assert_eq!(cooldown.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn websocket_preserves_beta_headers_forwards_quota_and_does_not_fallback_backend_errors()
    {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let headers = vec![
            ("OpenAI-Beta".into(), "responses=experimental".into()),
            ("chatgpt-account-id".into(), "workspace-a".into()),
            ("originator".into(), "phoenix-ide".into()),
        ];
        complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &headers,
            &BTreeMap::new(),
            &request_with(&[("rate-snapshot", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();
        let snapshot = std::iter::from_fn(|| rx.try_recv().ok())
            .find_map(|chunk| match chunk {
                super::super::TokenChunk::RateLimitSnapshot(snapshot) => Some(snapshot),
                super::super::TokenChunk::Text(_) => None,
            })
            .expect("rate-limit snapshot");
        assert!((snapshot.primary.unwrap().used_percent - 42.0).abs() < f64::EPSILON);
        let received = state.ws_headers.lock().await;
        assert_eq!(
            received[0]["openai-beta"],
            "responses_websockets=2026-02-06"
        );
        assert_eq!(received[0]["chatgpt-account-id"], "workspace-a");
        assert_eq!(received[0]["originator"], "phoenix-ide");
        drop(received);

        let error = complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &headers,
            &BTreeMap::new(),
            &request_with(&[("rate-error", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, crate::LlmErrorKind::RateLimit);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn websocket_frame_timeout_falls_back_and_header_identity_reconnects() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _) = tokio::sync::mpsc::channel(8);
        let headers_a = vec![("chatgpt-account-id".into(), "a".into())];
        complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &headers_a,
            &BTreeMap::new(),
            &request_with(&[("stall", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 1);
        sessions.lock().await.by_cache_key.clear();
        {
            let pool = sessions.lock().await;
            pool.cooldown.lock().unwrap().reset();
        }
        let headers_b = vec![("ChatGPT-Account-ID".into(), "b".into())];
        complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &headers_b,
            &BTreeMap::new(),
            &request_with(&[("healthy", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();
        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn websocket_cancelled_attempt_is_dropped_before_reuse() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _) = tokio::sync::mpsc::channel(8);
        let task = tokio::spawn({
            let (url, sessions, tx) = (url.clone(), sessions.clone(), tx.clone());
            async move {
                complete_streaming(
                    &codex_spec(),
                    "secret",
                    Some(&url),
                    &[],
                    &BTreeMap::new(),
                    &request_with(&[("stall", MessageRole::User)]),
                    &tx,
                    true,
                    Some(&sessions),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while state.ws_requests.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &[],
            &BTreeMap::new(),
            &request_with(&[("healthy", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();
        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn websocket_pool_evicts_idle_and_oldest_capacity_entries_without_credentials() {
        let mut pool = CodexWsSessions::default();
        let now = Instant::now();
        for i in 0..CODEX_WS_MAX_SESSIONS {
            let entry = Arc::new(CodexWsSessionEntry::default());
            *entry.last_used.lock().unwrap() =
                now.checked_sub(Duration::from_secs(i as u64)).unwrap();
            pool.by_cache_key.insert(format!("cohort-{i}"), entry);
        }
        evict_ws_sessions(&mut pool, now);
        assert_eq!(pool.by_cache_key.len(), CODEX_WS_MAX_SESSIONS - 1);
        assert!(!pool
            .by_cache_key
            .contains_key(&format!("cohort-{}", CODEX_WS_MAX_SESSIONS - 1)));
        let idle = Arc::new(CodexWsSessionEntry::default());
        *idle.last_used.lock().unwrap() = now.checked_sub(CODEX_WS_IDLE_TTL).unwrap();
        pool.by_cache_key.insert("idle".into(), idle);
        evict_ws_sessions(&mut pool, now);
        assert!(!pool.by_cache_key.contains_key("idle"));
    }

    fn ws_session(
        prefix: Vec<serde_json::Value>,
        compatibility: serde_json::Value,
    ) -> CodexWsSession {
        CodexWsSession {
            response_id: Some("resp-1".into()),
            compatibility: Some(compatibility),
            prefix,
            ..CodexWsSession::default()
        }
    }

    #[tokio::test]
    async fn websocket_continuation_requires_exact_prefix_including_server_output() {
        let compatibility = serde_json::json!({"model":"gpt-5.6","stream":true});
        let prefix = vec![
            serde_json::json!({"type":"message","role":"user","content":"one"}),
            serde_json::json!({"type":"message","role":"assistant","content":"two"}),
        ];
        let old = ws_session(prefix.clone(), compatibility.clone());
        let mut next = prefix;
        next.push(serde_json::json!({"type":"message","role":"user","content":"three"}));
        assert_eq!(
            continuation_suffix(&old, &compatibility, &next),
            Some(("resp-1".into(), vec![next[2].clone()]))
        );
        next[1]["content"] = serde_json::json!("changed output");
        assert!(continuation_suffix(&old, &compatibility, &next).is_none());
    }

    #[tokio::test]
    async fn websocket_continuation_resets_on_non_prefix_or_property_change() {
        let compatibility = serde_json::json!({"model":"gpt-5.6","stream":true});
        let old = ws_session(vec![serde_json::json!("a")], compatibility.clone());
        assert!(continuation_suffix(&old, &compatibility, &[serde_json::json!("x")]).is_none());
        assert!(continuation_suffix(
            &old,
            &serde_json::json!({"model":"gpt-5.6-x","stream":true}),
            &[serde_json::json!("a"), serde_json::json!("b")]
        )
        .is_none());
    }

    #[tokio::test]
    async fn websocket_server_output_canonicalizes_to_next_request_input_shape() {
        let output = vec![
            serde_json::json!({"type":"message","content":[{"type":"output_text","text":"answer"}]}),
        ];
        assert_eq!(
            canonical_server_output(output).expect("supported output"),
            vec![serde_json::json!({"type":"message","role":"assistant","content":"answer"})]
        );
    }

    fn empty_request() -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            cache_key: PromptCacheKey::stable("test"),
        }
    }

    #[tokio::test]
    async fn custom_source_header_suppresses_default_source_header() {
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
    #[tokio::test]
    async fn tool_result_image_serialises_with_responses_api_part_types() {
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

    /// The explicit prompt-cache pass (enabled for `gpt-5.6-*`) marks history
    /// messages by converting their text into an `input_text` part. That
    /// discriminant is only valid on input-role messages — the Responses API
    /// rejects it on an assistant message (parts must be `output_text` /
    /// `refusal`) with HTTP 400. Regression guard: a replayed assistant text
    /// turn must stay a plain string and never gain an `input_text` part.
    #[tokio::test]
    async fn explicit_cache_never_marks_assistant_message_with_input_text() {
        use crate::types::{ContentBlock, LlmMessage, MessageRole};

        let mut req = empty_request();
        req.messages = vec![
            LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text("first question")],
            },
            LlmMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("prior answer")],
            },
            LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text("follow-up question")],
            },
        ];

        let translated = translate_to_responses_request("gpt-5.6-sol", &req, false);
        let json = serde_json::to_value(&translated).unwrap();
        let input = json["input"].as_array().expect("input array");

        for item in input {
            if item["role"] == "assistant" {
                let content = &item["content"];
                assert!(
                    content.is_string(),
                    "assistant content must stay a plain string, not an \
                     input_text parts array; got {content}"
                );
            }
            if let Some(parts) = item["content"].as_array() {
                for part in parts {
                    if part["type"] == "input_text" {
                        assert_ne!(
                            item["role"], "assistant",
                            "assistant message must never carry an input_text part"
                        );
                    }
                }
            }
        }

        // The earlier user turn is still eligible for an explicit breakpoint,
        // so the pass has not been disabled wholesale.
        let earlier_user_marked = input.iter().any(|item| {
            item["role"] == "user"
                && item["content"]
                    .as_array()
                    .and_then(|parts| parts.first())
                    .map(|part| part["type"] == "input_text")
                    .unwrap_or(false)
        });
        assert!(
            earlier_user_marked,
            "explicit cache pass should still mark the earlier user message"
        );
    }

    #[tokio::test]
    async fn codex_continuation_input_including_prompt_fits_typed_item_limit() {
        use crate::types::{ContentBlock, LlmMessage, MessageRole};

        let limits = crate::ContinuationRequestLimits::codex_bridge();
        let history_cap = limits
            .max_history_messages(1)
            .expect("Codex continuation item cap");
        let mut req = empty_request();
        req.messages = (0..history_cap)
            .map(|i| LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("history {i}"))],
            })
            .collect();
        req.messages.push(LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text("prepare continuation handoff")],
        });

        let translated = translate_to_responses_request("gpt-5.5", &req, true);
        assert_eq!(translated.input.len(), history_cap + 1);
        assert!(
            translated.input.len() <= limits.max_input_items().unwrap(),
            "translated history plus continuation prompt exceeds route limit"
        );
    }

    #[tokio::test]
    async fn codex_lite_continuation_reserves_provider_prefix_items() {
        use crate::types::{ContentBlock, LlmMessage, MessageRole};

        let limits = crate::ContinuationRequestLimits::codex_responses_lite();
        let history_cap = limits
            .max_history_messages(1)
            .expect("Codex Lite continuation item cap");
        let mut req = empty_request();
        req.messages = (0..history_cap)
            .map(|i| LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("history {i}"))],
            })
            .collect();
        req.messages.push(LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text("prepare continuation handoff")],
        });

        let translated = translate_to_backend_request("gpt-5.6-sol", &req, true);
        let ResponsesBackendRequest::CodexLite(translated) = translated else {
            panic!("GPT-5.6 Codex must use Responses Lite");
        };
        assert_eq!(translated.input.len(), history_cap + 1 + 2);
        assert_eq!(translated.input.len(), limits.max_input_items().unwrap());
    }

    #[tokio::test]
    async fn test_request_tags_omitted_when_none() {
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

    #[tokio::test]
    async fn test_request_tags_serialized_when_set() {
        let mut req = translate_to_responses_request("gpt-5.5", &empty_request(), false);
        let mut tags = BTreeMap::new();
        tags.insert("disable_data_logging".to_string(), "true".to_string());
        tags.insert("foo".to_string(), "bar".to_string());
        req.tags = Some(tags);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["tags"]["disable_data_logging"], "true");
        assert_eq!(json["tags"]["foo"], "bar");
    }

    #[tokio::test]
    async fn classify_responses_error_codex_codes_route_to_terminal_variants() {
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

    #[tokio::test]
    async fn classify_responses_error_maps_codes() {
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

    #[tokio::test]
    async fn process_event_returns_err_on_top_level_error() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{"type":"error","code":"rate_limit_exceeded","message":"slow down"}"#;
        let err = acc.process_event("error", data, &tx).await.unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::RateLimit);
    }

    // --- streaming SSE accumulator robustness ---

    #[tokio::test]
    async fn process_event_malformed_sse_data_is_invalid_response_not_panic() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let err = acc.process_event("", "{ not json", &tx).await.unwrap_err();
        assert!(
            err.message.contains("Failed to parse SSE data"),
            "got: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn process_event_done_sentinel_is_ignored() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // The `[DONE]` sentinel is not JSON; it must be a no-op, not a parse error.
        acc.process_event("", "[DONE]", &tx).await.unwrap();
        assert!(
            !acc.done,
            "[DONE] sentinel alone does not finalize the stream"
        );
    }

    #[tokio::test]
    async fn process_event_unknown_dispatch_type_is_ignored() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        acc.process_event("", r#"{"type":"response.in_progress"}"#, &tx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn process_event_empty_dispatch_type_is_ignored_and_logged_once() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // An event whose embedded `type` is empty has nothing to dispatch on; it
        // must be tolerated (logged exactly once), never erroring the stream.
        assert!(!acc.logged_empty_dispatch);
        acc.process_event("", r#"{"type":""}"#, &tx).await.unwrap();
        assert!(
            acc.logged_empty_dispatch,
            "first empty-dispatch event is logged"
        );
        acc.process_event("", r#"{"type":""}"#, &tx).await.unwrap();
    }

    #[tokio::test]
    async fn process_event_assembles_message_text_from_output_item_done() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // The primary (non-fallback) assembly path: a completed message item.
        let data = r#"{
            "type":"response.output_item.done",
            "item":{"type":"message","role":"assistant","content":[
                {"type":"output_text","text":"Pong"}
            ]}
        }"#;
        acc.process_event("response.output_item.done", data, &tx)
            .await
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

    #[tokio::test]
    async fn process_event_handles_codex_nested_error_shape() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        // Real codex/ChatGPT-backend payload captured 2026-05-11 via WARN log.
        let data = r#"{"type":"error","error":{"type":"invalid_request_error","code":"context_length_exceeded","message":"Your input exceeds the context window of this model. Please adjust your input and try again.","param":"input"},"sequence_number":2}"#;
        let err = acc.process_event("error", data, &tx).await.unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::ContextWindowExceeded);
        assert!(!err.kind.is_auto_retryable());
        assert!(err.message.contains("context_length_exceeded"));
        assert!(err.message.contains("Your input exceeds"));
    }

    #[tokio::test]
    async fn process_event_returns_err_on_response_failed() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"upstream"}}}"#;
        let err = acc
            .process_event("response.failed", data, &tx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::ServerError);
    }

    #[tokio::test]
    async fn process_event_returns_err_on_response_incomplete_max_tokens() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#;
        let err = acc
            .process_event("response.incomplete", data, &tx)
            .await
            .unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::ServerError);
    }

    /// When the stream lacks `response.output_item.done` events but
    /// `response.completed` carries `/response/output: [...]`, the terminal
    /// payload is authoritative — fall back to it instead of dropping the
    /// assembled message. Repro of the 2026-05-11 gateway behaviour where
    /// `support-chat-completions` produced 5 output tokens, was billed for
    /// them, but Phoenix persisted "`end_turn` with empty content".
    #[tokio::test]
    async fn process_event_recovers_output_from_response_completed_when_no_item_done() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
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
            .await
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

    /// `OpenAI`'s cached-read and cache-write details are both subsets of
    /// `input_tokens`; normalization splits both without changing context usage.
    #[tokio::test]
    async fn responses_api_cache_details_are_threaded_without_double_counting() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new();
        let data = r#"{
            "type":"response.completed",
            "response":{
                "usage":{
                    "input_tokens":1000,
                    "output_tokens":50,
                    "input_tokens_details":{"cached_tokens":600,"cache_write_tokens":200}
                },
                "output":[
                    {"type":"message","role":"assistant","content":[
                        {"type":"output_text","text":"Pong"}
                    ]}
                ]
            }
        }"#;
        acc.process_event("response.completed", data, &tx)
            .await
            .expect("handler should not error");
        assert_eq!(acc.cached_tokens, 600);
        assert_eq!(acc.cache_write_tokens, 200);

        let resp = normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: acc.output_items,
            usage: ResponsesApiUsage {
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: acc.cached_tokens,
                    cache_write_tokens: acc.cache_write_tokens,
                },
            },
        })
        .expect("a response with a message item normalizes");
        assert_eq!(resp.usage.cache_read_tokens, 600);
        assert_eq!(resp.usage.input_tokens, 200);
        assert_eq!(resp.usage.cache_creation_tokens, 200);
        assert_eq!(resp.usage.context_window_used(), 1050);
    }

    #[tokio::test]
    async fn gpt56_emits_valid_breakpoints_but_older_and_codex_models_do_not() {
        let mut request = empty_request();
        for i in 0..5 {
            request.messages.push(LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("stable-{i}"))],
            });
        }
        request.messages.push(LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-1".into(),
                content: "tool output".into(),
                images: vec![],
                is_error: false,
            }],
        });

        let wire = serde_json::to_value(translate_to_responses_request(
            "gpt-5.6-2026-07-01",
            &request,
            false,
        ))
        .unwrap();
        assert_eq!(wire["prompt_cache_options"]["mode"], "implicit");
        assert_eq!(wire["prompt_cache_options"]["ttl"], "30m");
        let serialized = serde_json::to_string(&wire).unwrap();
        assert_eq!(serialized.matches("prompt_cache_breakpoint").count(), 4);
        let messages: Vec<_> = wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "message")
            .collect();
        assert!(!messages
            .last()
            .unwrap()
            .to_string()
            .contains("prompt_cache_breakpoint"));
        let output = wire["input"].as_array().unwrap().last().unwrap();
        assert_eq!(output["type"], "function_call_output");
        assert!(output.get("prompt_cache_breakpoint").is_none());
        assert!(!output.to_string().contains("prompt_cache_breakpoint"));

        for (model, codex) in [("gpt-5.5", false), ("gpt-5.6", true)] {
            let legacy =
                serde_json::to_value(translate_to_responses_request(model, &request, codex))
                    .unwrap();
            assert!(legacy.get("prompt_cache_options").is_none());
            assert!(!legacy.to_string().contains("prompt_cache_breakpoint"));
        }
    }

    #[tokio::test]
    async fn explicit_cache_breakpoints_preserve_fifty_read_boundaries() {
        let mut request = empty_request();
        for i in 0..55 {
            request.messages.push(LlmMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("stable-{i}"))],
            });
        }

        let wire = serde_json::to_value(translate_to_responses_request("gpt-5.6", &request, false))
            .unwrap();
        let input = wire["input"].as_array().unwrap();
        assert_eq!(
            wire.to_string().matches("prompt_cache_breakpoint").count(),
            50
        );
        assert!(!input
            .last()
            .unwrap()
            .to_string()
            .contains("prompt_cache_breakpoint"));
        assert!(!input[3].to_string().contains("prompt_cache_breakpoint"));
        assert!(input[4].to_string().contains("prompt_cache_breakpoint"));
    }

    /// A gateway that omits `input_tokens_details` must not panic or shift
    /// accounting: cached defaults to 0 and `input_tokens` is unchanged.
    /// `output_tokens` is 0 here — an empty, unbilled response, so the
    /// billed-but-empty guard does not fire.
    #[tokio::test]
    async fn responses_api_usage_without_cached_details_defaults_to_zero() {
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
    #[tokio::test]
    async fn responses_api_empty_content_with_billed_tokens_is_retryable_error() {
        let err = normalize_responses_api_response(ResponsesApiResponse {
            status: "completed".to_string(),
            output: vec![],
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens: 42,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
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
    #[tokio::test]
    async fn responses_api_refusal_message_surfaces_as_text_not_retried() {
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
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
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
    #[tokio::test]
    async fn process_event_fallback_skips_when_output_items_already_captured() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
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
            .await
            .unwrap();
        acc.process_event("response.completed", completed, &tx)
            .await
            .unwrap();
        assert_eq!(
            acc.output_items.len(),
            1,
            "fallback must not duplicate items already captured via item.done"
        );
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

    pub fn translate_to_backend_request_wire(
        api_name: &str,
        request: &crate::types::LlmRequest,
        use_codex_backend: bool,
    ) -> serde_json::Value {
        serde_json::to_value(super::translate_to_backend_request(
            api_name,
            request,
            use_codex_backend,
        ))
        .expect("request serializes")
    }

    pub fn translate_to_responses_request_codex(
        api_name: &str,
        request: &crate::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, true)
    }
}
