//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::headers::apply_source_header;
use super::models::ModelSpec;
use super::rate_limit::{
    normalize_credit_depletion, parse_active_limit, parse_credits_snapshot, parse_promo_message,
    parse_rate_limit_for_limit, parse_rate_limit_reached_type, QuotaDetails,
};
use super::stream_telemetry::{GenerationKind, StreamTelemetryRecorder};
use super::types::{ContentBlock, LlmRequest, LlmResponse, MessageRole, ModelEffort, Usage};
use super::LlmError;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use phoenix_core::domain::llm_types::ProviderRequestTier;
use phoenix_core::domain::provider_replay::ProviderReplayUpdate;
use phoenix_core::domain::responses_replay::ResponsesResponseSet;
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

pub(crate) fn is_official_responses_route(base_url_override: Option<&str>) -> bool {
    base_url_override.is_none_or(|url| url == "https://api.openai.com/v1/responses")
}

fn resolve_chat_endpoint(base_url_override: Option<&str>) -> String {
    base_url_override.map_or_else(
        || "https://api.openai.com/v1/chat/completions".to_string(),
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
    validate_responses_replay(request, &spec.api_name)?;
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
    if let Some(telemetry) = request.telemetry.as_ref() {
        telemetry
            .attempt_capture
            .set_transport(crate::LlmTransport::HttpJson);
    }
    let mut responses_request = translate_to_backend_request(
        &spec.api_name,
        request,
        use_codex_backend,
        use_codex_backend || is_official_responses_route(base_url_override),
    );
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
        return Err(responses_http_error(status.as_u16(), &body));
    }

    let responses_response: ResponsesApiResponse = serde_json::from_str(&body).map_err(|_| {
        tracing::debug!(body_bytes = body.len(), "malformed Responses API response");
        LlmError::invalid_response("Failed to parse Responses API response")
    })?;

    bind_responses_model(
        normalize_responses_api_response(responses_response)?,
        &spec.api_name,
    )
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
fn responses_error_detail(code: &str, message: &str) -> String {
    if code.is_empty() {
        message.to_string()
    } else {
        format!("{code}: {message}")
    }
}

fn classify_known_responses_error(code: &str, message: &str) -> Option<LlmError> {
    let detail = responses_error_detail(code, message);
    let lower = code.to_ascii_lowercase();

    // Codex-specific terminal signals — match PR 77's HTTP-path semantics.
    if lower == "usage_limit_reached" {
        return Some(LlmError::usage_limit_reached(QuotaDetails {
            plan_type: None,
            resets_at: None,
            limit_id: None,
            limit_name: None,
            primary: None,
            secondary: None,
            additional_limits: Vec::new(),
            credits: None,
            individual_limit: None,
            promo_message: None,
            rate_limit_reached_type: None,
        }));
    }
    if lower == "usage_not_included" {
        return Some(LlmError::auth(
            "Upgrade required: this plan does not include Codex usage. \
             Visit https://chatgpt.com/codex/settings/usage to upgrade.",
        ));
    }
    if lower == "server_is_overloaded" || lower == "slow_down" {
        return Some(LlmError::server_overloaded(
            "Selected model is at capacity. Try a different model.",
        ));
    }
    if lower == "invalid_prompt" {
        return Some(LlmError::prompt_rejected(detail));
    }

    if lower.contains("rate_limit") || lower.contains("quota") || lower.contains("requests_per") {
        Some(LlmError::rate_limit(detail))
    } else if lower.contains("auth")
        || lower.contains("invalid_api_key")
        || lower.contains("permission")
    {
        Some(LlmError::auth(detail))
    } else if lower.contains("context_length")
        || lower.contains("token_limit")
        || lower.contains("max_tokens")
    {
        Some(LlmError::new(
            super::LlmErrorKind::ContextWindowExceeded,
            detail,
        ))
    } else if lower.contains("content_filter") || lower.contains("safety") {
        Some(LlmError::new(super::LlmErrorKind::ContentFilter, detail))
    } else if lower.contains("invalid") || lower.contains("bad_request") {
        Some(LlmError::invalid_request(detail))
    } else {
        None
    }
}

fn classify_responses_error(code: &str, message: &str) -> LlmError {
    classify_known_responses_error(code, message).unwrap_or_else(|| {
        // A streaming error has no HTTP status to fall back to. Unknown codes
        // remain retryable so the executor can recover from provider failures.
        LlmError::server_error(responses_error_detail(code, message))
    })
}

fn responses_http_error(status: u16, body: &str) -> LlmError {
    if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(body) {
        let message = error_resp.error.message;
        if let Some(error) = error_resp
            .error
            .code
            .as_deref()
            .and_then(|code| classify_known_responses_error(code, &message))
        {
            return error;
        }
        return match status {
            401 | 403 => LlmError::auth(format!("Authentication failed: {message}")),
            429 => LlmError::rate_limit(format!("Rate limit exceeded: {message}")),
            400..=499 => LlmError::invalid_request(format!("Bad request ({status}): {message}")),
            500..=599 => LlmError::server_error(format!("Server error: {message}")),
            _ => LlmError::server_error(format!("Unexpected HTTP {status}: {message}")),
        };
    }
    LlmError::from_http_status(status, body)
}

/// Accumulates state across Responses API SSE stream events.
struct ResponsesStreamAccumulator {
    input_tokens: u32,
    output_tokens: u32,
    reasoning_tokens: Option<u32>,
    /// Cached-read subset of `input_tokens`.
    cached_tokens: u32,
    /// Cache-write subset of `input_tokens` on GPT-5.6-era models.
    cache_write_tokens: u32,
    /// Completed output items collected from `response.output_item.done` events.
    output_items: BTreeMap<usize, ResponsesApiOutput>,
    response_id: String,
    model: String,
    /// Set true when `response.done` is received.
    pub done: bool,
    /// Logged-once flag: first empty-`dispatch_type` event per stream gets a
    /// truncated payload dump at debug, subsequent ones are silent. Gateways
    /// that omit the SSE `event:` line **and** the JSON `type` field are
    /// otherwise opaque — capturing one example per stream is enough to
    /// classify the wire shape next time the success path stops working.
    logged_empty_dispatch: bool,
    telemetry: StreamTelemetryRecorder,
    observed_non_reasoning_output: bool,
}

fn has_visible_content_part(part: &serde_json::Value) -> bool {
    let nonempty = |field| {
        part.get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };

    match part.get("type").and_then(serde_json::Value::as_str) {
        Some("output_text") => nonempty("text"),
        Some("refusal") => nonempty("refusal"),
        _ => false,
    }
}

fn has_streamed_visible_output(event_type: &str, event: &serde_json::Value) -> bool {
    match event_type {
        "response.output_text.done" => event
            .get("text")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty()),
        "response.refusal.done" => event
            .get("refusal")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|refusal| !refusal.is_empty()),
        "response.content_part.added" | "response.content_part.done" => {
            event.get("part").is_some_and(has_visible_content_part)
        }
        "response.output_item.added" => event
            .pointer("/item/content")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|content| content.iter().any(has_visible_content_part)),
        _ => false,
    }
}

impl ResponsesStreamAccumulator {
    fn new(dispatch_at: Instant, request: &LlmRequest) -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: None,
            cached_tokens: 0,
            cache_write_tokens: 0,
            output_items: BTreeMap::new(),
            response_id: String::new(),
            model: String::new(),
            done: false,
            logged_empty_dispatch: false,
            telemetry: StreamTelemetryRecorder::new(
                dispatch_at,
                request
                    .telemetry
                    .as_ref()
                    .map(|t| t.attempt_capture.clone()),
            ),
            observed_non_reasoning_output: false,
        }
    }

    #[allow(clippy::too_many_lines)] // dispatch table; each arm is small
    async fn process_event(
        &mut self,
        event_type: &str,
        data: &str,
        emit: &tokio::sync::mpsc::Sender<super::TokenChunk>,
    ) -> Result<(), LlmError> {
        let now = Instant::now();
        self.telemetry.record_provider_event_at(now);
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

        match dispatch_type {
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(delta) = v.get("delta").and_then(serde_json::Value::as_str) {
                    if !delta.is_empty() {
                        self.telemetry
                            .record_generation_event_at(now, GenerationKind::Text);
                        self.telemetry.record_visible_text_at(now);
                        self.observed_non_reasoning_output = true;
                        let _ = emit.send(super::TokenChunk::Text(delta.to_string())).await;
                    }
                }
            }
            "response.output_text.done"
            | "response.refusal.done"
            | "response.content_part.added"
            | "response.content_part.done" => {
                if has_streamed_visible_output(dispatch_type, &v) {
                    self.telemetry
                        .record_generation_event_at(now, GenerationKind::Text);
                    self.telemetry.record_visible_text_at(now);
                    self.observed_non_reasoning_output = true;
                }
            }
            "response.reasoning.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta" => {
                if v.get("delta")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|delta| !delta.is_empty())
                {
                    self.telemetry
                        .record_generation_event_at(now, GenerationKind::Reasoning);
                }
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                let field = if dispatch_type == "response.function_call_arguments.done" {
                    "arguments"
                } else {
                    "delta"
                };
                if v.get(field)
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|arguments| !arguments.is_empty())
                {
                    self.telemetry
                        .record_generation_event_at(now, GenerationKind::Tool);
                    self.observed_non_reasoning_output = true;
                }
            }
            "response.output_item.added" => {
                let observed_tool = v.pointer("/item/type").and_then(serde_json::Value::as_str)
                    == Some("function_call")
                    && v.pointer("/item/name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| !name.is_empty());
                let observed_text = has_streamed_visible_output(dispatch_type, &v);
                if observed_tool {
                    self.telemetry
                        .record_generation_event_at(now, GenerationKind::Tool);
                } else if observed_text {
                    self.telemetry
                        .record_generation_event_at(now, GenerationKind::Text);
                    self.telemetry.record_visible_text_at(now);
                }
                self.observed_non_reasoning_output |= observed_tool || observed_text;
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    match serde_json::from_value::<ResponsesApiOutput>(item.clone()) {
                        Ok(output) => {
                            tracing::debug!(
                                output_type = %output.output_type(),
                                "responses_api output item collected"
                            );
                            match output.output_type() {
                                "function_call" => self
                                    .telemetry
                                    .record_generation_event_at(now, GenerationKind::Tool),
                                "reasoning" => self
                                    .telemetry
                                    .record_generation_event_at(now, GenerationKind::Reasoning),
                                "message" => self
                                    .telemetry
                                    .record_generation_event_at(now, GenerationKind::Structured),
                                _ => {}
                            }
                            let index = v
                                .get("output_index")
                                .and_then(serde_json::Value::as_u64)
                                .and_then(|index| usize::try_from(index).ok())
                                .unwrap_or(self.output_items.len());
                            self.output_items.insert(index, output);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                item_type = item.get("type").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
                                item_bytes = item.to_string().len(),
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
                    data_len = data.len(),
                    "responses_api SSE error event"
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
                    data_len = data.len(),
                    "responses_api SSE failure event"
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
                    data_len = data.len(),
                    "responses_api SSE incomplete event"
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
                if let Some(id) = v
                    .pointer("/response/id")
                    .and_then(serde_json::Value::as_str)
                {
                    self.response_id = id.to_string();
                }
                if let Some(model) = v
                    .pointer("/response/model")
                    .and_then(serde_json::Value::as_str)
                {
                    self.model = model.to_string();
                }
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
                    self.reasoning_tokens = usage
                        .pointer("/output_tokens_details/reasoning_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok());
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
                    tracing::warn!(
                        data_len = data.len(),
                        "responses_api terminal event had no /response/usage"
                    );
                }
                if let Some(items) = v
                    .pointer("/response/output")
                    .and_then(serde_json::Value::as_array)
                {
                    self.complete_output_items(items);
                }
                self.done = true;
            }
            _ => {
                tracing::debug!(dispatch_type, "responses_api ignoring event");
                if dispatch_type.is_empty() && !self.logged_empty_dispatch {
                    self.logged_empty_dispatch = true;
                    // Char-aware truncation — slicing by byte index would
                    // panic on a non-UTF8 boundary.
                    tracing::debug!(
                        event_type = %event_type,
                        data_len = data.len(),
                        "responses_api empty-dispatch event — first occurrence in this stream"
                    );
                }
            }
        }
        Ok(())
    }

    fn complete_output_items(&mut self, terminal: &[serde_json::Value]) {
        if self.output_items.iter().all(|(ordinal, collected)| {
            terminal
                .get(*ordinal)
                .is_some_and(|item| preserves_completed_output(&collected.0, item))
        }) {
            self.output_items.extend(
                terminal
                    .iter()
                    .enumerate()
                    .map(|(ordinal, item)| (ordinal, ResponsesApiOutput(item.clone()))),
            );
            return;
        }
        for (ordinal, item) in terminal.iter().enumerate() {
            let index = if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                self.output_items.iter().find_map(|(index, collected)| {
                    (collected.0.get("id").and_then(serde_json::Value::as_str) == Some(id))
                        .then_some(*index)
                })
            } else if terminal.len() == self.output_items.len() {
                self.output_items
                    .get(&ordinal)
                    .filter(|collected| collected.0.get("id").is_none())
                    .map(|_| ordinal)
            } else {
                None
            };
            if let Some(collected) = index.and_then(|index| self.output_items.get_mut(&index)) {
                if preserves_completed_output(&collected.0, item) {
                    collected.0 = item.clone();
                } else {
                    tracing::debug!("retaining completed stream item over conflicting or incomplete terminal output");
                }
            } else {
                tracing::debug!(
                    "ignoring terminal enrichment without a matching completed stream item"
                );
            }
        }
    }

    fn output_items_as_values(&self) -> Vec<serde_json::Value> {
        self.output_items
            .values()
            .map(|item| item.0.clone())
            .collect()
    }

    fn into_response(self) -> Result<LlmResponse, LlmError> {
        tracing::debug!(
            output_items = self.output_items.len(),
            input_tokens = self.input_tokens,
            output_tokens = self.output_tokens,
            "responses_api stream accumulator finalizing"
        );
        let telemetry = self.telemetry;
        let mut response = normalize_responses_api_response_with_evidence(
            ResponsesApiResponse {
                id: self.response_id,
                model: self.model,
                status: "completed".to_string(),
                output: self.output_items.into_values().collect(),
                usage: ResponsesApiUsage {
                    input_tokens: self.input_tokens,
                    output_tokens: self.output_tokens,
                    input_tokens_details: ResponsesApiInputTokensDetails {
                        cached_tokens: self.cached_tokens,
                        cache_write_tokens: self.cache_write_tokens,
                    },
                    output_tokens_details: self.reasoning_tokens.map(|reasoning_tokens| {
                        ResponsesApiOutputTokensDetails {
                            reasoning_tokens: Some(reasoning_tokens),
                        }
                    }),
                },
            },
            self.observed_non_reasoning_output,
        )?;
        telemetry.attach_success(&mut response);
        Ok(response)
    }
}

fn preserves_completed_output(collected: &serde_json::Value, terminal: &serde_json::Value) -> bool {
    match (collected, terminal) {
        (serde_json::Value::Object(collected), serde_json::Value::Object(terminal)) => {
            collected.iter().all(|(key, value)| {
                terminal
                    .get(key)
                    .is_some_and(|terminal| preserves_completed_output(value, terminal))
            })
        }
        (serde_json::Value::Array(collected), serde_json::Value::Array(terminal)) => {
            collected.len() == terminal.len()
                && collected
                    .iter()
                    .zip(terminal)
                    .all(|(collected, terminal)| preserves_completed_output(collected, terminal))
        }
        _ => collected == terminal,
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

#[derive(Debug)]
pub(crate) struct CodexWsSessions {
    /// The outer mutex protects entry creation and bounded eviction. Each cache
    /// cohort has its own mutex, so unrelated conversations remain concurrent.
    by_cache_key: HashMap<String, Arc<CodexWsSessionEntry>>,
    /// Transport health belongs to the shared Codex endpoint pool, not to a
    /// prompt-cache cohort. A failed endpoint must be avoided by new conversations too.
    cooldown: std::sync::Mutex<CodexWsCooldown>,
    frame_timeout: Duration,
}

impl Default for CodexWsSessions {
    fn default() -> Self {
        Self {
            by_cache_key: HashMap::new(),
            cooldown: std::sync::Mutex::new(CodexWsCooldown::default()),
            frame_timeout: CODEX_WS_FRAME_TIMEOUT,
        }
    }
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
    continuation_id: Option<String>,
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
    parse_codex_error(status, &headers, &body).or_else(|| {
        let code = error.get("code").and_then(serde_json::Value::as_str);
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| code.unwrap_or("unknown error"));
        if let Some(classified) =
            code.and_then(|code| classify_known_responses_error(code, message))
        {
            return Some(classified);
        }
        if code.is_none() {
            if let Some(classified) = error
                .get("type")
                .and_then(serde_json::Value::as_str)
                .and_then(|error_type| classify_known_responses_error(error_type, message))
            {
                return Some(classified);
            }
        }
        Some(responses_http_error(status, &body))
    })
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
#[allow(clippy::too_many_arguments)]
async fn complete_codex_websocket(
    http_url: &str,
    api_key: &str,
    custom_headers: &[(String, String)],
    cache_key: &str,
    full_request: &ResponsesBackendRequest,
    request: &LlmRequest,
    chunk_tx: &tokio::sync::mpsc::Sender<super::TokenChunk>,
    sessions: &Arc<Mutex<CodexWsSessions>>,
) -> Result<LlmResponse, CodexWsError> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let (compatibility, full_input) =
        continuation_parts(full_request).map_err(CodexWsError::backend)?;
    let identity = connection_identity(api_key, custom_headers);
    let (entry, frame_timeout) = {
        let mut guard = sessions.lock().await;
        evict_ws_sessions(&mut guard, Instant::now());
        if !guard.by_cache_key.contains_key(cache_key)
            && guard.by_cache_key.len() >= CODEX_WS_MAX_SESSIONS
        {
            return Err(CodexWsError::fallback(LlmError::network(
                "Codex WebSocket session capacity reached",
            )));
        }
        let entry = guard
            .by_cache_key
            .entry(cache_key.to_string())
            .or_insert_with(|| Arc::new(CodexWsSessionEntry::default()))
            .clone();
        (entry, guard.frame_timeout)
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
    if session.continuation_id.as_deref() != request.tool_availability.continuation_id() {
        reset_ws_session(&mut session);
        session.continuation_id = request
            .tool_availability
            .continuation_id()
            .map(str::to_owned);
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
        let dispatch_at = Instant::now();
        tokio::time::timeout(
            frame_timeout,
            socket.send(tungstenite::Message::Text(envelope.to_string().into())),
        )
        .await
        .map_err(|_| CodexWsError::fallback(LlmError::network("WebSocket send timeout")))?
        .map_err(|e| CodexWsError::fallback(LlmError::network(format!("WebSocket send: {e}"))))?;
        let mut acc = ResponsesStreamAccumulator::new(dispatch_at, request);
        let mut response_id = None;
        let mut server_output = Vec::new();
        loop {
            let message = tokio::time::timeout(frame_timeout, socket.next())
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
                        .send(super::TokenChunk::RateLimitSnapshot(Box::new(snapshot)))
                        .await;
                }
                continue;
            }
            acc.process_event(event_type, &text, chunk_tx)
                .await
                .map_err(CodexWsError::backend)?;
            if matches!(
                event_type,
                "response.output_text.delta" | "response.refusal.delta"
            ) && value
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
        let model = match full_request {
            ResponsesBackendRequest::Platform(request) => &request.model,
            ResponsesBackendRequest::CodexLite(request) => &request.model,
        };
        let response = finalize_websocket_response(acc, model)?;
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
            let continuation_output = match &response.provider_replay {
                Some(ProviderReplayUpdate::Responses(set)) => Some(set.output_items.clone()),
                _ => canonical_server_output(server_output),
            };
            if let Some(output) = continuation_output {
                let mut prefix = full_input;
                prefix.extend(output);
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

fn finalize_websocket_response(
    acc: ResponsesStreamAccumulator,
    model: &str,
) -> Result<LlmResponse, CodexWsError> {
    bind_responses_model(acc.into_response().map_err(CodexWsError::backend)?, model)
        .map_err(CodexWsError::backend)
}

fn finalize_responses_stream(
    acc: ResponsesStreamAccumulator,
    model: &str,
) -> Result<LlmResponse, LlmError> {
    if !acc.done {
        return Err(LlmError::network(
            "Responses stream ended before a terminal event",
        ));
    }
    bind_responses_model(acc.into_response()?, model)
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
    validate_responses_replay(request, &spec.api_name)?;
    let url = resolve_endpoint(base_url_override);
    let mut responses_request = translate_to_backend_request(
        &spec.api_name,
        request,
        use_codex_backend,
        use_codex_backend || is_official_responses_route(base_url_override),
    );
    responses_request.set_streaming();
    responses_request.set_tags(request_tags);

    if use_codex_backend && supports_responses_lite(&spec.api_name) {
        if let Some(sessions) = ws_sessions {
            if let Some(telemetry) = request.telemetry.as_ref() {
                telemetry
                    .attempt_capture
                    .set_transport(crate::LlmTransport::Websocket);
            }
            match complete_codex_websocket(
                &url,
                api_key,
                custom_headers,
                request.cache_key.as_str(),
                &responses_request,
                request,
                chunk_tx,
                sessions,
            )
            .await
            {
                Ok(response) => {
                    tracing::Span::current().record("transport", "websocket");
                    return Ok(response);
                }
                Err(CodexWsError::Cooldown) => {
                    tracing::debug!("Codex WebSocket transport cooldown active; using HTTP/SSE");
                }
                Err(CodexWsError::Backend(error) | CodexWsError::Interrupted(error)) => {
                    return Err(error);
                }
                Err(CodexWsError::Reconnect(error)) => {
                    tracing::debug!(error_kind = ?error.kind,
                        "Codex WebSocket lifetime exhausted; retrying once on a fresh socket");
                    match complete_codex_websocket(
                        &url,
                        api_key,
                        custom_headers,
                        request.cache_key.as_str(),
                        &responses_request,
                        request,
                        chunk_tx,
                        sessions,
                    )
                    .await
                    {
                        Ok(response) => {
                            tracing::Span::current().record("transport", "websocket");
                            return Ok(response);
                        }
                        Err(
                            CodexWsError::Backend(error)
                            | CodexWsError::Interrupted(error)
                            | CodexWsError::Reconnect(error),
                        ) => return Err(error),
                        Err(CodexWsError::Cooldown) => tracing::debug!(
                            "Codex WebSocket cooldown became active; using HTTP/SSE"
                        ),
                        Err(CodexWsError::Fallback(error)) => {
                            tracing::warn!(error_kind = ?error.kind,
                                "fresh Codex WebSocket failed; falling back once to full HTTP/SSE");
                        }
                    }
                }
                Err(CodexWsError::Fallback(error)) => tracing::warn!(error_kind = ?error.kind,
                    "Codex WebSocket transport/protocol failed; falling back once to full HTTP/SSE"),
            }
        }
    }

    tracing::Span::current().record("transport", "http_sse");
    if let Some(telemetry) = request.telemetry.as_ref() {
        telemetry
            .attempt_capture
            .set_transport(crate::LlmTransport::HttpSse);
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
    let dispatch_at = Instant::now();
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
        return Err(responses_http_error(status.as_u16(), &body));
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
                .send(super::TokenChunk::RateLimitSnapshot(Box::new(snapshot)))
                .await;
        }
    }

    let mut acc = ResponsesStreamAccumulator::new(dispatch_at, request);
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
                let diagnostics = sse.diagnostics();
                tracing::error!(?diagnostics, "SSE parser diagnostics");
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

    finalize_responses_stream(acc, &spec.api_name)
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
    official_openai_route: bool,
) -> ResponsesApiRequest {
    use super::types::ImageSource;

    let mut input_items = Vec::new();

    let mut instructions = if request.system.is_empty() {
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

    if !official_openai_route || use_codex_backend {
        append_advisory(&mut instructions, request);
    }

    // Process each message as a unit to allow grouping text + images
    for msg in &request.messages {
        if let Some(replay) = request.responses_replay.iter().find(|set| {
            msg.source_message_id.as_deref() == Some(set.owner_message_id.as_str())
                && msg.content == set.public_content
                && set.model == api_name
        }) {
            input_items.extend(
                replay
                    .output_items
                    .iter()
                    .cloned()
                    .map(ResponsesApiInputItem::Replay),
            );
            continue;
        }
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

        // Emit a message item for text + image content. User turns are model
        // input (input_text/input_image parts, cache-markable); assistant turns
        // are model output and serialize as a plain string, which structurally
        // cannot carry an input_text part.
        match msg.role {
            MessageRole::User => {
                if !text_blocks.is_empty() || !image_blocks.is_empty() {
                    let content = if image_blocks.is_empty() {
                        InputMessageContent::Text(text_blocks.join("\n"))
                    } else {
                        let mut parts: Vec<InputMessagePart> = text_blocks
                            .iter()
                            .map(|t| InputMessagePart::InputText {
                                text: (*t).to_string(),
                                prompt_cache_breakpoint: None,
                            })
                            .collect();
                        for source in &image_blocks {
                            let ImageSource::Base64 { media_type, data } = source;
                            parts.push(InputMessagePart::InputImage {
                                image_url: format!("data:{media_type};base64,{data}"),
                                prompt_cache_breakpoint: None,
                            });
                        }
                        InputMessageContent::Parts(parts)
                    };
                    input_items.push(ResponsesApiInputItem::InputMessage {
                        role: InputMessageRole::User,
                        content,
                    });
                }
            }
            MessageRole::Assistant => {
                // Assistant content is text-only. An image on an assistant turn
                // is not representable in the OpenAI assistant wire form —
                // log-drop it so the capability gap is visible, not silent.
                if !image_blocks.is_empty() {
                    tracing::debug!(
                        n = image_blocks.len(),
                        "dropping images on assistant message — OpenAI assistant \
                         content cannot carry images"
                    );
                }
                if !text_blocks.is_empty() {
                    input_items.push(ResponsesApiInputItem::AssistantMessage {
                        role: AssistantMessageRole::Assistant,
                        content: text_blocks.join("\n"),
                    });
                }
            }
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

    let tools: Option<Vec<ResponsesApiTool>> =
        if request.tool_availability.declarations().is_empty() {
            None
        } else {
            Some(
                request
                    .tool_availability
                    .declarations()
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

    let explicit_cache_supported =
        !use_codex_backend && official_openai_route && supports_explicit_prompt_cache(api_name);
    if explicit_cache_supported {
        place_explicit_cache_breakpoints(&mut input_items);
    }

    let has_tools = !request.tool_availability.declarations().is_empty();
    ResponsesApiRequest {
        model: api_name.to_string(),
        input: input_items,
        instructions,
        tools,
        max_output_tokens: if use_codex_backend {
            None
        } else {
            Some(
                request
                    .raised_output_token_ceiling()
                    .unwrap_or_else(|| default_output_headroom(request.effective_effort.level())),
            )
        },
        stream: None,
        store: if use_codex_backend || official_openai_route {
            Some(false)
        } else {
            None
        },
        prompt_cache_key: Some(request.cache_key.as_str().to_string()),
        prompt_cache_options: explicit_cache_supported.then_some(PromptCacheOptions {
            mode: PromptCacheMode::Implicit,
            ttl: PromptCacheTtl::ThirtyMinutes,
        }),
        reasoning: request
            .effective_effort
            .explicit_level()
            .map(platform_reasoning),
        service_tier: ProviderRequestTier::from_effective_service_tier(
            request.service_tier,
            use_codex_backend || (official_openai_route && is_known_gpt_6(api_name)),
        )
        .responses_request_value()
        .map(str::to_string),
        tool_choice: responses_tool_choice(request, official_openai_route && !use_codex_backend),
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
        include: if official_openai_route || use_codex_backend {
            vec!["reasoning.encrypted_content".to_string()]
        } else {
            Vec::new()
        },
        tags: None,
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesToolChoice {
    Mode(ToolChoiceMode),
    Allowed {
        r#type: AllowedToolsType,
        mode: AutoToolMode,
        tools: Vec<ResponsesAllowedTool>,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ChatToolChoice {
    Mode(ToolChoiceMode),
    Allowed {
        r#type: AllowedToolsType,
        allowed_tools: ChatAllowedTools,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolChoiceMode {
    Auto,
    None,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutoToolMode {
    Auto,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AllowedToolsType {
    AllowedTools,
}
#[derive(Debug, Serialize)]
pub(crate) struct ResponsesAllowedTool {
    r#type: FunctionToolType,
    name: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum FunctionToolType {
    Function,
}
#[derive(Debug, Serialize)]
struct ChatAllowedTools {
    mode: AutoToolMode,
    tools: Vec<ChatAllowedTool>,
}
#[derive(Debug, Serialize)]
struct ChatAllowedTool {
    r#type: FunctionToolType,
    function: ChatAllowedFunction,
}
#[derive(Debug, Serialize)]
struct ChatAllowedFunction {
    name: String,
}

fn responses_tool_choice(request: &LlmRequest, supported: bool) -> Option<ResponsesToolChoice> {
    let policy = &request.tool_availability;
    if policy.declarations().is_empty() {
        return None;
    }
    if supported && policy.callable_names().is_empty() {
        Some(ResponsesToolChoice::Mode(ToolChoiceMode::None))
    } else if supported && policy.callable_names().len() < policy.declarations().len() {
        Some(ResponsesToolChoice::Allowed {
            r#type: AllowedToolsType::AllowedTools,
            mode: AutoToolMode::Auto,
            tools: policy
                .callable_names()
                .iter()
                .map(|name| ResponsesAllowedTool {
                    r#type: FunctionToolType::Function,
                    name: name.clone(),
                })
                .collect(),
        })
    } else {
        Some(ResponsesToolChoice::Mode(ToolChoiceMode::Auto))
    }
}

fn chat_tool_choice(request: &LlmRequest, supported: bool) -> Option<ChatToolChoice> {
    let policy = &request.tool_availability;
    if policy.declarations().is_empty() {
        return None;
    }
    if supported && policy.callable_names().is_empty() {
        Some(ChatToolChoice::Mode(ToolChoiceMode::None))
    } else if supported && policy.callable_names().len() < policy.declarations().len() {
        Some(ChatToolChoice::Allowed {
            r#type: AllowedToolsType::AllowedTools,
            allowed_tools: ChatAllowedTools {
                mode: AutoToolMode::Auto,
                tools: policy
                    .callable_names()
                    .iter()
                    .map(|name| ChatAllowedTool {
                        r#type: FunctionToolType::Function,
                        function: ChatAllowedFunction { name: name.clone() },
                    })
                    .collect(),
            },
        })
    } else {
        Some(ChatToolChoice::Mode(ToolChoiceMode::Auto))
    }
}

fn append_advisory(instructions: &mut Option<String>, request: &LlmRequest) {
    let unavailable: Vec<_> = request
        .tool_availability
        .declarations()
        .iter()
        .filter(|tool| !request.tool_availability.is_callable(&tool.name))
        .map(|tool| tool.name.as_str())
        .collect();
    if unavailable.is_empty() {
        return;
    }
    let text = instructions.get_or_insert_with(String::new);
    text.push_str("\n\nThese tools are unavailable for new calls: ");
    text.push_str(&unavailable.join(", "));
    text.push_str(
        ". Their definitions are retained for historical context. Choose available tools instead.",
    );
    tracing::debug!(
        provider = "openai",
        n = unavailable.len(),
        "using advisory tool restriction on route without native support"
    );
}

fn bind_responses_model(
    mut response: LlmResponse,
    api_name: &str,
) -> Result<LlmResponse, LlmError> {
    if let Some(ProviderReplayUpdate::Responses(set)) = response.provider_replay.as_mut() {
        if set.response_id.is_empty() {
            return Err(LlmError::invalid_response(
                "Responses continuation response has no id",
            ));
        }
        set.model = api_name.to_string();
    }
    Ok(response)
}

fn validate_responses_replay(request: &LlmRequest, api_name: &str) -> Result<(), LlmError> {
    let mut owners_seen = std::collections::BTreeSet::new();
    for set in &request.responses_replay {
        if !owners_seen.insert(&set.owner_message_id) {
            return Err(LlmError::invalid_request(
                "duplicate Responses replay owner",
            ));
        }
        set.validate().map_err(LlmError::invalid_request)?;
        if set.model != api_name {
            return Err(LlmError::invalid_request(
                "Responses replay belongs to another model",
            ));
        }
        let owners: Vec<_> = request
            .messages
            .iter()
            .filter(|message| {
                message.source_message_id.as_deref() == Some(set.owner_message_id.as_str())
            })
            .collect();
        if owners.len() != 1
            || owners[0].role != MessageRole::Assistant
            || owners[0].content != set.public_content
        {
            return Err(LlmError::invalid_request(
                "Responses replay owner was removed or rewritten",
            ));
        }
    }
    Ok(())
}

fn default_output_headroom(effort: Option<ModelEffort>) -> u32 {
    if effort.is_some_and(ModelEffort::needs_extended_output_headroom) {
        64_000
    } else {
        16_384
    }
}

fn platform_reasoning(effort: ModelEffort) -> ResponsesApiReasoning {
    ResponsesApiReasoning { effort }
}

fn codex_lite_reasoning(effort: Option<ModelEffort>) -> CodexResponsesLiteReasoning {
    CodexResponsesLiteReasoning {
        context: CodexReasoningContext::AllTurns,
        effort,
    }
}

fn translate_to_backend_request(
    api_name: &str,
    request: &LlmRequest,
    use_codex_backend: bool,
    official_openai_route: bool,
) -> ResponsesBackendRequest {
    let platform =
        translate_to_responses_request(api_name, request, use_codex_backend, official_openai_route);
    if use_codex_backend && supports_responses_lite(api_name) {
        ResponsesBackendRequest::CodexLite(CodexResponsesLiteRequest::from_platform(platform))
    } else {
        ResponsesBackendRequest::Platform(platform)
    }
}

fn is_known_gpt_6(api_name: &str) -> bool {
    matches!(
        api_name,
        "gpt-6-astra" | "gpt-6.1-sol" | "gpt-6-sol" | "gpt-6-luna"
    )
}

fn supports_modern_responses(api_name: &str) -> bool {
    api_name == "gpt-5.6" || api_name.starts_with("gpt-5.6-") || is_known_gpt_6(api_name)
}

pub(crate) fn supports_responses_lite(api_name: &str) -> bool {
    supports_modern_responses(api_name)
}

fn supports_explicit_prompt_cache(api_name: &str) -> bool {
    supports_modern_responses(api_name)
}

/// Preserve `OpenAI`'s historical read boundaries while leaving the latest
/// message to implicit mode. The service considers the latest 50 explicit
/// markers for reads and decides which newest markers consume its write slots.
fn place_explicit_cache_breakpoints(items: &mut [ResponsesApiInputItem]) {
    const READ_BREAKPOINT_LIMIT: usize = 50;

    // The final message of the request stays in implicit-cache mode, so it is
    // never marked. Assistant messages are model output and cannot carry an
    // explicit marker (their content type has no marker field), but they still
    // count when locating that trailing implicit boundary.
    let Some(last_message_idx) = items.iter().rposition(|item| {
        matches!(
            item,
            ResponsesApiInputItem::InputMessage { .. }
                | ResponsesApiInputItem::AssistantMessage { .. }
        )
    }) else {
        return;
    };

    // Mark input-role messages newest-first, up to the read limit. Assistant
    // messages are structurally excluded here, so they never consume read-marker
    // budget — the full limit stays available for markable input-role history in
    // a long alternating conversation.
    items[..last_message_idx]
        .iter_mut()
        .rev()
        .filter_map(|item| match item {
            ResponsesApiInputItem::InputMessage { content, .. } => Some(content),
            ResponsesApiInputItem::AssistantMessage { .. }
            | ResponsesApiInputItem::AdditionalTools { .. }
            | ResponsesApiInputItem::FunctionCall { .. }
            | ResponsesApiInputItem::FunctionCallOutput { .. }
            | ResponsesApiInputItem::Replay(_) => None,
        })
        .take(READ_BREAKPOINT_LIMIT)
        .for_each(InputMessageContent::mark_last_block);
}

fn validate_responses_terminal_content(
    status: &str,
    usage: &ResponsesApiUsage,
    output_items: &[serde_json::Value],
    content_is_empty: bool,
    observed_non_reasoning_output: bool,
) -> Result<(), LlmError> {
    if !content_is_empty {
        return Ok(());
    }
    if observed_non_reasoning_output {
        tracing::error!(
            output_tokens = usage.output_tokens,
            reasoning_tokens = usage
                .output_tokens_details
                .as_ref()
                .and_then(|details| details.reasoning_tokens),
            output_item_count = output_items.len(),
            status,
            "responses_api lost observed non-reasoning output before terminal assembly"
        );
        return Err(LlmError::server_error(
            "OpenAI streamed non-reasoning output without a completed terminal item",
        ));
    }
    if usage.output_tokens == 0 && output_items.is_empty() {
        return Ok(());
    }

    let output_item_count = output_items.len();
    let reasoning_items: Vec<ResponsesReasoningOutputView> = output_items
        .iter()
        .filter_map(|item| serde_json::from_value(item.clone()).ok())
        .collect();
    let reasoning_item_count = reasoning_items.len();
    let message_items: Vec<ResponsesEmptyMessageOutputView> = output_items
        .iter()
        .filter_map(|item| serde_json::from_value(item.clone()).ok())
        .collect();
    let message_item_count = message_items.len();
    let function_call_item_count = output_items
        .iter()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("function_call"))
        .count();
    let reasoning_tokens = usage
        .output_tokens_details
        .as_ref()
        .and_then(|details| details.reasoning_tokens);
    let valid_reasoning_items = reasoning_items.iter().all(|item| {
        item.r#type == "reasoning"
            && !item.id.is_empty()
            && item
                .status
                .as_deref()
                .is_none_or(|value| value == "completed")
            && item
                .summary
                .iter()
                .all(|summary| summary.r#type == "summary_text")
    });
    let has_reasoning_usage =
        reasoning_tokens.is_some_and(|tokens| tokens > 0 && tokens <= usage.output_tokens);
    let valid_empty_messages = message_items
        .iter()
        .all(ResponsesEmptyMessageOutputView::is_completed_empty_assistant_message);
    let completed_quiet_reasoning = status == "completed"
        && reasoning_item_count > 0
        && reasoning_item_count + message_item_count == output_item_count
        && valid_reasoning_items
        && valid_empty_messages
        && has_reasoning_usage;

    if completed_quiet_reasoning {
        tracing::debug!(
            output_tokens = usage.output_tokens,
            reasoning_tokens,
            output_item_count,
            status,
            message_item_count,
            "responses_api completed a quiet reasoning turn"
        );
        return Ok(());
    }

    tracing::error!(
        output_tokens = usage.output_tokens,
        reasoning_tokens,
        output_item_count,
        reasoning_item_count,
        message_item_count,
        function_call_item_count,
        status,
        "responses_api returned no terminal content with output tokens billed"
    );
    Err(LlmError::server_error(format!(
        "OpenAI returned empty response ({} output tokens billed, status={status})",
        usage.output_tokens
    )))
}

/// Normalize `ResponsesApiResponse` to `LlmResponse`.
fn normalize_responses_api_response(resp: ResponsesApiResponse) -> Result<LlmResponse, LlmError> {
    normalize_responses_api_response_with_evidence(resp, false)
}

fn normalize_responses_api_response_with_evidence(
    resp: ResponsesApiResponse,
    observed_non_reasoning_output: bool,
) -> Result<LlmResponse, LlmError> {
    let output_items: Vec<serde_json::Value> =
        resp.output.iter().map(|item| item.0.clone()).collect();
    let mut content = Vec::new();

    for output in resp.output {
        let output: ResponsesOutputView = serde_json::from_value(output.0).map_err(|_| {
            tracing::debug!("malformed Responses API output item");
            LlmError::invalid_response("Failed to parse Responses API output item")
        })?;
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
                        tracing::warn!(error = %e, arguments_len = arguments.len(), "Failed to parse function call arguments");
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

    validate_responses_terminal_content(
        &resp.status,
        &resp.usage,
        &output_items,
        content.is_empty(),
        observed_non_reasoning_output,
    )?;

    let mut response = LlmResponse::non_streaming(content, end_turn, {
        // Both detail buckets are subsets of OpenAI's inclusive
        // `input_tokens`. Split them out so Phoenix's additive Usage shape
        // preserves the provider-reported context total.
        let cached = u64::from(resp.usage.input_tokens_details.cached_tokens);
        let written = u64::from(resp.usage.input_tokens_details.cache_write_tokens);
        let reasoning = resp
            .usage
            .output_tokens_details
            .and_then(|details| details.reasoning_tokens.map(u64::from));
        Usage {
            input_tokens: u64::from(resp.usage.input_tokens)
                .saturating_sub(cached.saturating_add(written)),
            output_tokens: u64::from(resp.usage.output_tokens),
            reasoning_tokens: reasoning,
            cache_creation_tokens: written,
            cache_read_tokens: cached,
        }
    });
    response.provider_replay = if has_tool_calls {
        Some(ProviderReplayUpdate::Responses(ResponsesResponseSet {
            response_id: resp.id,
            model: resp.model,
            owner_message_id: String::new(),
            public_content: response.content.clone(),
            output_items,
        }))
    } else if end_turn {
        Some(ProviderReplayUpdate::Clear)
    } else {
        None
    };
    Ok(response)
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
                    let rate_limit_reached_type = normalize_credit_depletion(
                        &credits,
                        parse_rate_limit_reached_type(headers),
                    );
                    Some(LlmError::usage_limit_reached(QuotaDetails {
                        plan_type: envelope.error.plan_type,
                        resets_at,
                        limit_id,
                        limit_name,
                        primary,
                        secondary,
                        credits,
                        additional_limits: Vec::new(),
                        promo_message,
                        individual_limit: None,
                        rate_limit_reached_type,
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
    service_tier: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<ModelEffort>,
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
        input.push(ResponsesApiInputItem::InputMessage {
            role: InputMessageRole::Developer,
            content: InputMessageContent::Parts(vec![InputMessagePart::InputText {
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
            reasoning: codex_lite_reasoning(request.reasoning.as_ref().map(|r| r.effort)),
            service_tier: request.service_tier,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<ResponsesApiReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) service_tier: Option<String>,
    /// Tool selection strategy. `"auto"` is the server-side default; sent
    /// explicitly to stabilise the wire shape and to make non-default
    /// strategies a smaller change later. Omitted when no tools are sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<ResponsesToolChoice>,
    /// Allow the model to emit multiple tool calls in one response. The
    /// server-side default is `true`; sent explicitly to stabilise wire
    /// shape. Omitted when no tools are sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parallel_tool_calls: Option<bool>,
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
pub(crate) struct ResponsesApiReasoning {
    effort: ModelEffort,
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
    /// A model-*input* message (user or developer). Its content parts use the
    /// `input_text`/`input_image` discriminants and are the only messages that
    /// may carry an explicit prompt-cache breakpoint.
    #[serde(rename = "message")]
    InputMessage {
        role: InputMessageRole,
        content: InputMessageContent,
    },
    /// A replayed assistant turn — model *output* fed back as input. The
    /// Responses API requires assistant content parts to be
    /// `output_text`/`refusal` and rejects an `input_text` part; a plain string
    /// is the always-valid form and the only shape this translator produces
    /// (assistant turns are text-only — tool calls become `FunctionCall` items
    /// and images never occur on model output). Modeling the content as a bare
    /// `String` makes an assistant `input_text` part unrepresentable, and the
    /// absence of a cache-marker field makes marking one structurally
    /// impossible.
    #[serde(rename = "message")]
    AssistantMessage {
        role: AssistantMessageRole,
        content: String,
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
    #[serde(untagged)]
    Replay(serde_json::Value),
}

/// Roles whose message content is model input. `input_text`/`input_image` parts
/// and explicit cache breakpoints are valid only for these roles.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputMessageRole {
    User,
    Developer,
}

/// The assistant role as a single-variant enum: an assistant message's role is
/// fixed by construction and cannot be set to an input role.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssistantMessageRole {
    Assistant,
}

/// Input-role message content: a plain string when text-only, or an array of
/// parts when images are present or an explicit cache breakpoint is placed.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum InputMessageContent {
    Text(String),
    Parts(Vec<InputMessagePart>),
}

impl InputMessageContent {
    fn mark_last_block(&mut self) {
        match self {
            Self::Text(text) => {
                *self = Self::Parts(vec![InputMessagePart::InputText {
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
pub(crate) enum InputMessagePart {
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

impl InputMessagePart {
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
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) model: String,
    pub(crate) status: String,
    pub(crate) output: Vec<ResponsesApiOutput>,
    pub(crate) usage: ResponsesApiUsage,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub(crate) struct ResponsesApiOutput(serde_json::Value);

impl ResponsesApiOutput {
    fn output_type(&self) -> &str {
        self.0
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    }
}

#[derive(Debug, Deserialize)]
struct ResponsesReasoningOutputView {
    r#type: String,
    id: String,
    #[serde(default)]
    status: Option<String>,
    summary: Vec<ResponsesReasoningSummaryView>,
}

#[derive(Debug, Deserialize)]
struct ResponsesReasoningSummaryView {
    r#type: String,
    #[allow(dead_code)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct ResponsesEmptyMessageOutputView {
    r#type: String,
    id: String,
    status: String,
    role: String,
    content: Vec<ResponsesApiContent>,
}

impl ResponsesEmptyMessageOutputView {
    fn is_completed_empty_assistant_message(&self) -> bool {
        self.r#type == "message"
            && !self.id.is_empty()
            && self.status == "completed"
            && self.role == "assistant"
            && self.content.iter().all(|part| match part.r#type.as_str() {
                "output_text" => part.text.as_deref() == Some("") && part.refusal.is_none(),
                "refusal" => part.refusal.as_deref() == Some("") && part.text.is_none(),
                _ => false,
            })
    }
}

#[derive(Debug, Deserialize)]
struct ResponsesOutputView {
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
pub(crate) struct ResponsesApiOutputTokensDetails {
    pub(crate) reasoning_tokens: Option<u32>,
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
    #[serde(default)]
    pub(crate) output_tokens_details: Option<ResponsesApiOutputTokensDetails>,
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
    let mut chat_request = translate_to_chat_request_with_route(
        &spec.api_name,
        request,
        base_url_override.is_none_or(|url| url == "https://api.openai.com/v1/chat/completions"),
    );
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

    if !status.is_success() {
        return Err(openai_http_error(status.as_u16(), status.as_str(), &body));
    }

    let chat_response: ChatCompletionsResponse = serde_json::from_str(&body).map_err(|error| {
        tracing::debug!(error = %error, body_bytes = body.len(), "failed to parse chat completions response");
        LlmError::invalid_response("Failed to parse Chat Completions response")
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
    chunk_tx: &tokio::sync::mpsc::Sender<super::TokenChunk>,
) -> Result<LlmResponse, LlmError> {
    use futures::StreamExt;

    let url = resolve_chat_endpoint(base_url_override);
    let mut chat_request = translate_to_chat_request_with_route(
        &spec.api_name,
        request,
        base_url_override.is_none_or(|url| url == "https://api.openai.com/v1/chat/completions"),
    );
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
    let dispatch_at = Instant::now();
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
        return Err(openai_http_error(status.as_u16(), status.as_str(), &body));
    }

    let mut acc = ChatStreamAccumulator::new(dispatch_at, request);
    let mut sse = super::sse::SseParser::new();
    let mut stream = response.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError::network(format!("Stream error: {e}")))?;
        for event in sse.push(&chunk) {
            if let Err(e) = acc.process_event(&event.data, chunk_tx).await {
                tracing::error!(
                    data_len = event.data.len(),
                    "chat SSE event processing failed; dumping parser diagnostics"
                );
                tracing::error!(diagnostics = ?sse.diagnostics(), "chat SSE parser diagnostics");
                return Err(e);
            }
            if acc.done {
                break 'outer;
            }
        }
    }

    for event in sse.finish() {
        acc.process_event(&event.data, chunk_tx).await?;
    }

    acc.into_response()
}

/// Translate `LlmRequest` to `ChatCompletionsRequest`.
#[cfg(test)]
fn translate_to_chat_request(api_name: &str, request: &LlmRequest) -> ChatCompletionsRequest {
    translate_to_chat_request_with_route(api_name, request, false)
}

#[allow(clippy::too_many_lines)]
fn translate_to_chat_request_with_route(
    api_name: &str,
    request: &LlmRequest,
    official_route: bool,
) -> ChatCompletionsRequest {
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

    if !official_route {
        let mut advisory = None;
        append_advisory(&mut advisory, request);
        if let Some(text) = advisory {
            messages.push(ChatMessage {
                role: "system".into(),
                content: Some(ChatContent::Text(text)),
                tool_calls: None,
                tool_call_id: None,
            });
        }
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
                    if msg.role == MessageRole::Assistant {
                        tracing::debug!(
                            role,
                            "dropping assistant image in chat completions translation — image_url content is user-role only"
                        );
                        continue;
                    }
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

    let tools = if request.tool_availability.declarations().is_empty() {
        None
    } else {
        Some(
            request
                .tool_availability
                .declarations()
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

    let has_tools = !request.tool_availability.declarations().is_empty();
    ChatCompletionsRequest {
        model: api_name.to_string(),
        messages,
        tools,
        max_tokens: request.max_tokens,
        reasoning_effort: request.effective_effort.explicit_level(),
        stream: None,
        stream_options: None,
        tool_choice: chat_tool_choice(request, official_route),
        parallel_tool_calls: if has_tools { Some(true) } else { None },
        tags: None,
    }
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
    if resp.choices.len() != 1 {
        return Err(LlmError::invalid_response(
            "Chat completions must return exactly one choice",
        ));
    }
    let choice = resp
        .choices
        .into_iter()
        .next()
        .expect("length checked above");
    match choice.finish_reason.as_deref() {
        Some("length") => {
            log_chat_completion_length(
                model,
                resp.usage.as_ref().and_then(ChatUsage::reasoning_tokens),
            );
            return Err(LlmError::invalid_response(
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
    let visible_text = message.content.filter(|text| !text.is_empty());
    let refusal = message.refusal.filter(|text| !text.is_empty());
    if refusal.is_some() && !message.tool_calls.as_deref().unwrap_or_default().is_empty() {
        return Err(LlmError::invalid_response(
            "Chat completions refusal included tool calls",
        ));
    }
    if visible_text.is_some() && refusal.is_some() {
        return Err(LlmError::invalid_response(
            "Chat completions returned both content and refusal text",
        ));
    }
    if let Some(text) = visible_text.or(refusal) {
        content.push(ContentBlock::Text { text });
    }
    for call in message.tool_calls.unwrap_or_default() {
        if call.r#type != "function" {
            return Err(LlmError::invalid_response(
                "Chat completions returned a non-function tool call",
            ));
        }
        if call.id.is_empty() || call.function.name.is_empty() {
            return Err(LlmError::invalid_response(
                "Chat completions tool call omitted its id or function name",
            ));
        }
        let input = serde_json::from_str(&call.function.arguments).map_err(|error| {
            tracing::warn!(
                error = %error,
                arguments_bytes = call.function.arguments.len(),
                "failed to parse chat tool call arguments"
            );
            LlmError::invalid_response("Chat completions returned malformed tool arguments")
        })?;
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
    if usage.prompt_tokens_details.cached_tokens > usage.prompt_tokens {
        return Err(LlmError::invalid_response(
            "Chat completions cached prompt tokens exceed total prompt tokens",
        ));
    }
    let cached = u64::from(usage.prompt_tokens_details.cached_tokens);
    Ok(LlmResponse::non_streaming(
        content,
        !has_tool_calls,
        Usage {
            input_tokens: u64::from(usage.prompt_tokens).saturating_sub(cached),
            output_tokens: u64::from(usage.completion_tokens),
            cache_creation_tokens: 0,
            cache_read_tokens: cached,
            reasoning_tokens: usage.reasoning_tokens().map(u64::from),
        },
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChatVisibleKind {
    Content,
    Refusal,
}

struct ChatStreamAccumulator {
    content: String,
    visible_kind: Option<ChatVisibleKind>,
    choice_index: Option<usize>,
    tool_calls: Vec<ChatToolCallBuilder>,
    usage: Option<ChatUsage>,
    done: bool,
    terminal_finish_seen: bool,
    telemetry: StreamTelemetryRecorder,
}

fn classify_chat_stream_error(code: Option<&serde_json::Value>, message: &str) -> LlmError {
    match code {
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .map_or_else(
                || LlmError::server_error(message),
                |status| LlmError::from_http_status(status, message),
            ),
        Some(serde_json::Value::String(value)) => value.parse::<u16>().map_or_else(
            |_| classify_responses_error(value, message),
            |status| LlmError::from_http_status(status, message),
        ),
        Some(value) => classify_responses_error(&value.to_string(), message),
        None => classify_responses_error("", message),
    }
}

fn openai_http_error(status_code: u16, status_display: &str, body: &str) -> LlmError {
    if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(body) {
        let message = error_resp.error.message;
        let code = error_resp.error.code.as_deref().unwrap_or("");
        if matches!(code, "server_is_overloaded" | "slow_down") {
            return classify_responses_error(code, &message);
        }
        return match status_code {
            401 | 403 => LlmError::auth(format!("Authentication failed: {message}")),
            429 => LlmError::rate_limit(format!("Rate limit exceeded: {message}")),
            400..=499 if code.is_empty() => {
                LlmError::invalid_request(format!("Bad request ({status_code}): {message}"))
            }
            400..=499 => classify_responses_error(code, &message),
            500..=599 => LlmError::server_error(format!("Server error: {message}")),
            _ => LlmError::server_error(format!("Unexpected HTTP {status_display}: {message}")),
        };
    }
    LlmError::from_http_status(status_code, body)
}

impl ChatStreamAccumulator {
    fn new(dispatch_at: Instant, request: &LlmRequest) -> Self {
        Self {
            content: String::new(),
            visible_kind: None,
            choice_index: None,
            tool_calls: Vec::new(),
            usage: None,
            done: false,
            terminal_finish_seen: false,
            telemetry: StreamTelemetryRecorder::new(
                dispatch_at,
                request
                    .telemetry
                    .as_ref()
                    .map(|telemetry| telemetry.attempt_capture.clone()),
            ),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn process_event(
        &mut self,
        data: &str,
        chunk_tx: &tokio::sync::mpsc::Sender<super::TokenChunk>,
    ) -> Result<(), LlmError> {
        if data == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let now = Instant::now();
        self.telemetry.record_provider_event_at(now);
        let event: ChatStreamChunk = serde_json::from_str(data).map_err(|e| {
            LlmError::invalid_response(format!("Failed to parse chat SSE data: {e}"))
        })?;
        if let Some(err) = event.error {
            let msg = err
                .message
                .unwrap_or_else(|| "gateway returned error chunk".to_string());
            return Err(classify_chat_stream_error(err.code.as_ref(), &msg));
        }
        self.usage = event.usage.or(self.usage.take());
        let reasoning_tokens = self.usage.as_ref().and_then(ChatUsage::reasoning_tokens);
        if event.choices.len() > 1 {
            return Err(LlmError::invalid_response(
                "Chat completions stream returned multiple choices",
            ));
        }
        for choice in event.choices {
            if self
                .choice_index
                .is_some_and(|existing| existing != choice.index)
            {
                return Err(LlmError::invalid_response(
                    "Chat completions stream changed choice index",
                ));
            }
            self.choice_index = Some(choice.index);
            if let Some(reasoning) = choice.delta.reasoning_content {
                log_dropped_reasoning_content("<streaming-chat-completions>", &reasoning);
                if !reasoning.is_empty() {
                    self.telemetry
                        .record_generation_event_at(now, GenerationKind::Reasoning);
                }
            }
            if let Some(reason) = choice.finish_reason.as_deref() {
                self.terminal_finish_seen = true;
                match reason {
                    "length" => {
                        log_chat_completion_length(
                            "<streaming-chat-completions>",
                            reasoning_tokens,
                        );
                        return Err(LlmError::invalid_response(
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
            for (kind, delta) in [
                (ChatVisibleKind::Content, choice.delta.content),
                (ChatVisibleKind::Refusal, choice.delta.refusal),
            ] {
                let Some(delta) = delta.filter(|delta| !delta.is_empty()) else {
                    continue;
                };
                if self.visible_kind.is_some_and(|existing| existing != kind) {
                    return Err(LlmError::invalid_response(
                        "Chat completions stream mixed content and refusal text",
                    ));
                }
                if kind == ChatVisibleKind::Refusal && !self.tool_calls.is_empty() {
                    return Err(LlmError::invalid_response(
                        "Chat completions refusal included tool calls",
                    ));
                }
                self.visible_kind = Some(kind);
                self.content.push_str(&delta);
                self.telemetry
                    .record_generation_event_at(now, GenerationKind::Text);
                self.telemetry.record_visible_text_at(now);
                let _ = chunk_tx.send(super::TokenChunk::Text(delta)).await;
            }
            if self.visible_kind == Some(ChatVisibleKind::Refusal)
                && choice
                    .delta
                    .tool_calls
                    .as_deref()
                    .is_some_and(|calls| !calls.is_empty())
            {
                return Err(LlmError::invalid_response(
                    "Chat completions refusal included tool calls",
                ));
            }
            for tool_delta in choice.delta.tool_calls.unwrap_or_default() {
                self.telemetry
                    .record_generation_event_at(now, GenerationKind::Tool);
                let Some(index) = tool_delta.index else {
                    return Err(LlmError::invalid_response(
                        "Chat completions tool delta omitted its index",
                    ));
                };
                if index > self.tool_calls.len() {
                    return Err(LlmError::invalid_response(
                        "Chat completions tool delta used a sparse index",
                    ));
                }
                if index == self.tool_calls.len() {
                    self.tool_calls.push(ChatToolCallBuilder::default());
                }
                let builder = &mut self.tool_calls[index];
                if let Some(tool_type) = tool_delta.r#type {
                    if tool_type != "function" {
                        return Err(LlmError::invalid_response(
                            "Chat completions returned a non-function tool delta",
                        ));
                    }
                }
                if let Some(id) = tool_delta.id {
                    if !builder.id.is_empty() && builder.id != id {
                        return Err(LlmError::invalid_response(
                            "Chat completions tool call changed id while streaming",
                        ));
                    }
                    builder.id = id;
                }
                if let Some(function) = tool_delta.function {
                    if let Some(name) = function.name {
                        if !builder.name.is_empty() && builder.name != name {
                            return Err(LlmError::invalid_response(
                                "Chat completions tool call changed function name while streaming",
                            ));
                        }
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
        let telemetry = self.telemetry;
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
                        .collect::<Result<Vec<_>, _>>()?,
                )
            },
        };
        let mut response =
            chat_message_to_response(message, self.usage, "<streaming-chat-completions>")?;
        telemetry.attach_success(&mut response);
        Ok(response)
    }
}

#[derive(Default)]
struct ChatToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

impl ChatToolCallBuilder {
    fn build(self) -> Result<ChatToolCall, LlmError> {
        if self.id.is_empty() || self.name.is_empty() {
            return Err(LlmError::invalid_response(
                "Chat completions tool call omitted its id or function name",
            ));
        }
        Ok(ChatToolCall {
            id: self.id,
            r#type: "function".to_string(),
            function: ChatFunctionCall {
                name: self.name,
                arguments: self.arguments,
            },
        })
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
    reasoning_effort: Option<ModelEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<ChatStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ChatToolChoice>,
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
    #[serde(default)]
    index: usize,
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
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    r#type: Option<String>,
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
    use crate::types::{LlmMessage, LlmRequest, PromptCacheKey};

    use crate::models::{ModelBackend, ModelSource};
    use crate::types::MessageRole;
    use axum::extract::{ws::Message as AxumWsMessage, State, WebSocketUpgrade};
    use axum::http::HeaderMap as AxumHeaderMap;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Json;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Default)]
    struct MockResponsesState {
        connections: Arc<AtomicUsize>,
        http_requests: Arc<AtomicUsize>,
        ws_requests: Arc<Mutex<Vec<(usize, serde_json::Value)>>>,
        ws_headers: Arc<Mutex<Vec<AxumHeaderMap>>>,
        shared_delay_started: Arc<std::sync::atomic::AtomicBool>,
        shared_delay_release: Arc<tokio::sync::Notify>,
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
            while let Some(Ok(AxumWsMessage::Text(text))) = tokio::time::timeout(
                Duration::from_secs(5),
                socket.recv(),
            )
            .await
            .expect("mock WebSocket request arrives before the fixture deadline")
            {
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
                    if marker.contains("shared-deadline-fallback") {
                        state
                            .shared_delay_started
                            .store(true, Ordering::SeqCst);
                        tokio::time::timeout(
                            Duration::from_secs(30),
                            state.shared_delay_release.notified(),
                        )
                        .await
                        .expect("shared deadline test releases delayed 429");
                    }
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
                if marker.contains("shared-deadline-fallback") && connection > 0 {
                    socket
                        .send(AxumWsMessage::Text(
                            serde_json::json!({
                                "type": "response.reasoning_summary_text.delta",
                                "delta": "ws-reasoning"
                            })
                            .to_string(),
                        ))
                        .await
                        .unwrap();
                    return;
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
                    // test-timing-allow: scripted server stall is the timeout behavior under test
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
                if marker.contains("ws-tool-round") && n <= 2 {
                    let mut response = reasoning_response(&format!("resp-{n}"), &format!("call-{n}"));
                    response["model"] = request["model"].clone();
                    if n == 1 {
                        response["output"].as_array_mut().unwrap().remove(0);
                        response["output"][0]["phase"] = serde_json::json!("commentary");
                    }
                    socket.send(AxumWsMessage::Text(serde_json::json!({
                        "type":"response.completed", "response":response
                    }).to_string())).await.unwrap();
                    continue;
                }
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

    async fn mock_http(
        State(state): State<MockResponsesState>,
        Json(request): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        state.http_requests.fetch_add(1, Ordering::SeqCst);
        if request["input"]
            .to_string()
            .contains("shared-deadline-fallback")
        {
            // test-timing-allow: paused time proves HTTP fallback gets only remaining budget
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        (
            [("content-type", "text/event-stream")],
            "event: response.created\ndata: {\"type\":\"response.created\"}\n\n\
             event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"http-answer\"}\n\n\
             event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"http-1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1},\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"http-answer\"}]}]}}\n\n",
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
            family: "OpenAI".into(),
            description: String::new(),
            context_window: 100_000,
            max_output_tokens: Some(crate::DEFAULT_MAX_OUTPUT_TOKENS),
            recommended: false,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: crate::EffortCapabilities::unknown(),
            service_tier_capabilities: crate::models::ServiceTierCapabilities::Unsupported,
        }
    }

    fn request_with(messages: &[(&str, MessageRole)]) -> LlmRequest {
        LlmRequest {
            system: vec![],
            messages: messages
                .iter()
                .map(|(text, role)| LlmMessage {
                    source_message_id: None,
                    role: *role,
                    content: vec![ContentBlock::text(*text)],
                })
                .collect(),
            provider_replay: None,
            responses_replay: vec![],
            tool_availability: phoenix_core::domain::tool_availability::ToolAvailability::all(
                vec![],
            ),
            max_tokens: None,
            effective_effort: phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
            service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
            telemetry: None,
            cache_key: PromptCacheKey::stable("integration"),
        }
    }

    #[test]
    fn responses_provider_preserves_bash_label_schema_without_prevalidation_cap() {
        let mut request = request_with(&[("run a command", MessageRole::User)]);
        request.tool_availability =
            phoenix_core::domain::tool_availability::ToolAvailability::all(vec![
                phoenix_core::domain::llm_types::ToolDefinition {
                    name: "bash".to_string(),
                    description: "shell".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "Prefer 64 characters or fewer"
                            }
                        }
                    }),
                    defer_loading: false,
                },
            ]);

        let wire = serde_json::to_value(translate_to_responses_request(
            "gpt-test", &request, false, true,
        ))
        .unwrap();
        let label = &wire["tools"][0]["parameters"]["properties"]["label"];
        assert!(label.get("maxLength").is_none());
        assert_eq!(label["description"], "Prefer 64 characters or fewer");
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

    #[test]
    fn wrapped_websocket_invalid_prompt_is_user_resumable() {
        let error = parse_wrapped_codex_websocket_error(&serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "code": "invalid_prompt",
                "message": "prompt rejected by policy"
            }
        }))
        .expect("wrapped error maps");
        assert_eq!(error.kind, crate::LlmErrorKind::PromptRejected);
        assert!(error.kind.is_user_resumable());
    }

    #[tokio::test]
    async fn nonstreaming_malformed_response_never_exposes_private_output() {
        let app = Router::new().route(
            "/responses",
            axum::routing::post(|Json(request): Json<serde_json::Value>| async move {
                assert_eq!(request["model"], "gpt-5.6");
                Json(serde_json::json!({
                    "id":"private-response",
                    "model":"gpt-5.6",
                    "status":"completed",
                    "output":[{
                        "type":"reasoning",
                        "id":"private-item",
                        "summary":[],
                        "encrypted_content":"PRIVATE_ENCRYPTED_SENTINEL",
                        "unknown_private":"PRIVATE_UNKNOWN_SENTINEL"
                    }],
                    "usage":{"input_tokens":"PRIVATE_SERDE_SENTINEL","output_tokens":1}
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/responses", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let request = empty_request();
        let official_request = serde_json::to_value(translate_to_backend_request(
            "gpt-5.6", &request, false, true,
        ))
        .unwrap();
        assert_eq!(
            official_request["include"],
            serde_json::json!(["reasoning.encrypted_content"])
        );
        let error = complete(
            &codex_spec(),
            "test-key",
            Some(&url),
            &[],
            &BTreeMap::new(),
            &request,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, crate::LlmErrorKind::InvalidResponse);
        assert_eq!(error.message, "Failed to parse Responses API response");
        for sentinel in [
            "PRIVATE_ENCRYPTED_SENTINEL",
            "PRIVATE_UNKNOWN_SENTINEL",
            "PRIVATE_SERDE_SENTINEL",
        ] {
            assert!(!error.message.contains(sentinel));
        }
        server.abort();
    }

    #[test]
    fn malformed_private_output_view_returns_content_free_error() {
        let mut wire = reasoning_response("r1", "c1");
        wire["output"][0]["content"] = serde_json::json!("PRIVATE_VIEW_SENTINEL");
        wire["output"][0]["encrypted_content"] = serde_json::json!("PRIVATE_ENCRYPTED_SENTINEL");
        let error =
            normalize_responses_api_response(serde_json::from_value(wire).unwrap()).unwrap_err();
        assert_eq!(error.kind, crate::LlmErrorKind::InvalidResponse);
        assert_eq!(error.message, "Failed to parse Responses API output item");
        assert!(!error.message.contains("PRIVATE_VIEW_SENTINEL"));
        assert!(!error.message.contains("PRIVATE_ENCRYPTED_SENTINEL"));
    }

    #[tokio::test]
    async fn websocket_tool_rounds_continue_with_exact_private_output_prefix() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let mut request = request_with(&[("ws-tool-round", MessageRole::User)]);
        request.tool_availability = restricted_request(&["bash"])
            .tool_availability
            .with_continuation_id("tool-context".into())
            .unwrap();
        for round in 1..=3 {
            let response = complete_streaming(
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
            .unwrap();
            if round == 3 {
                assert!(response.end_turn);
                break;
            }
            let Some(ProviderReplayUpdate::Responses(set)) = response.provider_replay else {
                panic!("tool round requires full replay");
            };
            let owner = format!("owner-{round}");
            request.messages.push(LlmMessage {
                source_message_id: Some(owner.clone()),
                role: MessageRole::Assistant,
                content: set.public_content.clone(),
            });
            request
                .responses_replay
                .push(set.with_owner_message_id(owner));
            request.messages.push(LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: format!("call-{round}"),
                    content: format!("ws-tool-round-result-{round}"),
                    is_error: false,
                    images: vec![],
                }],
            });
        }
        let requests = state.ws_requests.lock().await;
        assert_eq!(state.connections.load(Ordering::SeqCst), 1);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 0);
        assert_eq!(requests.len(), 3);
        for round in 1..=2 {
            assert_eq!(requests[round].0, requests[0].0);
            assert_eq!(
                requests[round].1["previous_response_id"],
                format!("resp-{round}")
            );
            assert_eq!(
                requests[round].1["input"],
                serde_json::json!([{
                    "type":"function_call_output",
                    "call_id":format!("call-{round}"),
                    "output":format!("ws-tool-round-result-{round}")
                }])
            );
            assert_eq!(requests[round].1["prompt_cache_key"], "integration");
        }
        assert_eq!(
            request.responses_replay[0].output_items[0]["phase"],
            "commentary"
        );
        assert_eq!(
            request.responses_replay[1].output_items[0]["encrypted_content"],
            "opaque"
        );
        assert_eq!(
            request.responses_replay[1].output_items[0]["unexpected"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn websocket_retires_previous_response_on_durable_context_change() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions::default()));
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let call = |context: &str, messages: &[(&str, MessageRole)]| {
            let mut request = request_with(messages);
            request.tool_availability = request
                .tool_availability
                .with_continuation_id(context.to_owned())
                .unwrap();
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
                .unwrap();
            }
        };
        call("original-a", &[("one", MessageRole::User)]).await;
        call(
            "original-a",
            &[
                ("one", MessageRole::User),
                ("answer-1", MessageRole::Assistant),
                ("two", MessageRole::User),
            ],
        )
        .await;
        {
            let requests = state.ws_requests.lock().await;
            assert_eq!(requests[1].1["previous_response_id"], "resp-1");
        }
        // A settled excursion to another provider produces a new A context,
        // even though returning A sees the same public prefix plus a user turn.
        call(
            "returning-a",
            &[
                ("one", MessageRole::User),
                ("answer-1", MessageRole::Assistant),
                ("two", MessageRole::User),
                ("answer-2", MessageRole::Assistant),
                ("three", MessageRole::User),
            ],
        )
        .await;
        call(
            "returning-a",
            &[
                ("one", MessageRole::User),
                ("answer-1", MessageRole::Assistant),
                ("two", MessageRole::User),
                ("answer-2", MessageRole::Assistant),
                ("three", MessageRole::User),
                ("answer-3", MessageRole::Assistant),
                ("four", MessageRole::User),
            ],
        )
        .await;
        let requests = state.ws_requests.lock().await;
        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
        assert_ne!(requests[1].0, requests[2].0);
        assert_eq!(requests[2].0, requests[3].0);
        assert!(requests[2].1.get("previous_response_id").is_none());
        assert_eq!(requests[2].1["input"].as_array().unwrap().len(), 7);
        assert_eq!(requests[3].1["previous_response_id"], "resp-3");
        assert_eq!(requests[3].1["input"].as_array().unwrap().len(), 1);
        for (_, request) in requests.iter() {
            assert_eq!(request["prompt_cache_key"], "integration");
            assert!(request.get("continuation_id").is_none());
        }
        assert_eq!(
            sessions.lock().await.by_cache_key.len(),
            1,
            "incarnation replacement must not grow the pool per context"
        );
    }

    #[tokio::test]
    async fn websocket_reuses_connection_continues_resets_and_falls_back_safely() {
        // also covers HTTP fallback path: the mock HTTP SSE stream emits control + text + terminal.
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
            while let Some(chunk) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("public replay channel closes before the fixture deadline")
            {
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
        // quota/control frames must not become first-generation events.
        // also exercise the plain HTTP/SSE path, which has no WebSocket-specific rate-limit frame.
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
        let text_response = complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &headers,
            &BTreeMap::new(),
            &request_with(&[("healthy", MessageRole::User)]),
            &tx,
            true,
            Some(&sessions),
        )
        .await
        .unwrap();
        assert_eq!(text_response.stream_telemetry.generation_event_count, 0);
        assert_eq!(text_response.stream_telemetry.visible_text_event_count, 0);

        let http_only = complete_streaming(
            &codex_spec(),
            "secret",
            Some(&url),
            &headers,
            &BTreeMap::new(),
            &request_with(&[("plain-http", MessageRole::User)]),
            &tx,
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(http_only.text(), "http-answer");
        assert_eq!(http_only.stream_telemetry.provider_event_count, 3);
        assert_eq!(http_only.stream_telemetry.generation_event_count, 1);
        assert_eq!(http_only.stream_telemetry.visible_text_event_count, 1);
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
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn per_frame_timeout_does_not_bound_a_72_minute_logical_attempt() {
        let logical_dispatch = tokio::time::Instant::now();
        let request = request_with(&[("incident", MessageRole::User)]);
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel(1);
        let mut websocket = ResponsesStreamAccumulator::new(Instant::now(), &request);
        let reasoning = serde_json::json!({
            "type":"response.reasoning_summary_text.delta",
            "delta":"internal reasoning"
        })
        .to_string();

        // 285 frames arrive every 15 seconds. The exact production loop wraps
        // each `socket.next()` independently, so every frame satisfies the
        // 30-second guard while the logical attempt reaches 71.25 minutes.
        for _ in 0..285 {
            let guarded_frame = tokio::spawn(async {
                tokio::time::timeout(CODEX_WS_FRAME_TIMEOUT, async {
                    // test-timing-allow: paused Tokio time reproduces recurring frames inside the idle guard
                    tokio::time::sleep(Duration::from_secs(15)).await;
                })
                .await
            });
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_secs(15)).await;
            guarded_frame
                .await
                .unwrap()
                .expect("each frame arrives within the per-frame timeout");
            websocket
                .process_event(
                    "response.reasoning_summary_text.delta",
                    &reasoning,
                    &chunk_tx,
                )
                .await
                .unwrap();
            assert!(!websocket.done);
        }
        tokio::time::advance(Duration::from_secs(8)).await;
        assert_eq!(logical_dispatch.elapsed(), Duration::from_secs(4_283));
        assert_eq!(
            websocket.telemetry.snapshot(false).generation_event_count,
            285
        );
        assert!(
            chunk_rx.try_recv().is_err(),
            "reasoning produces no visible text"
        );

        // A transport failure can then replace the stream accumulator during
        // HTTP/SSE fallback. The logical request clock continues, but the final
        // stream snapshot describes only this fallback segment.
        let mut http = ResponsesStreamAccumulator::new(Instant::now(), &request);
        for _ in 0..282 {
            tokio::time::advance(Duration::from_millis(150)).await;
            http.process_event(
                "response.reasoning_summary_text.delta",
                &reasoning,
                &chunk_tx,
            )
            .await
            .unwrap();
        }
        for _ in 0..5 {
            tokio::time::advance(Duration::from_millis(150)).await;
            http.process_event(
                "response.created",
                &serde_json::json!({"type":"response.created"}).to_string(),
                &chunk_tx,
            )
            .await
            .unwrap();
        }
        tokio::time::advance(Duration::from_millis(3_550)).await;
        let terminal = serde_json::json!({
            "type":"response.completed",
            "response":{
                "id":"response-after-fallback",
                "usage":{"input_tokens":10,"output_tokens":1},
                "output":[{"type":"reasoning","id":"reasoning-1","summary":[]}]
            }
        })
        .to_string();
        http.process_event("response.completed", &terminal, &chunk_tx)
            .await
            .unwrap();

        let final_stream = http.telemetry.snapshot(true);
        assert_eq!(logical_dispatch.elapsed(), Duration::from_millis(4_329_600));
        assert_eq!(final_stream.provider_event_count, 288);
        assert_eq!(final_stream.generation_event_count, 282);
        assert_eq!(final_stream.visible_text_event_count, 0);
        assert!(final_stream.completed);
    }

    async fn wait_for_mock_phase<F, Fut>(phase: &str, mut ready: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if ready().await {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for mock phase: {phase}"
            );
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn real_websocket_reconnect_and_http_fallback_share_the_original_deadline() {
        let (url, state) = mock_server().await;
        let sessions = Arc::new(Mutex::new(CodexWsSessions {
            frame_timeout: Duration::from_secs(30),
            ..CodexWsSessions::default()
        }));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut request = request_with(&[(
            "connection-limit shared-deadline-fallback",
            MessageRole::User,
        )]);
        let capture = crate::LlmAttemptCapture::new();
        request.telemetry = Some(crate::LlmRequestTelemetry {
            conversation_id: "deadline".to_string(),
            root_conversation_id: "deadline".to_string(),
            request_id: "deadline".to_string(),
            retry_attempt: 1,
            attempt_capture: capture.clone(),
        });
        capture.begin(
            request.telemetry.as_ref().unwrap(),
            "openai",
            "gpt-test",
            crate::LlmTransport::Websocket,
        );
        let task = tokio::spawn(async move {
            Box::pin(crate::service::enforce_attempt_deadline(
                crate::service::LlmAttemptDeadline::new(Duration::from_secs(10)),
                &request,
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
                ),
            ))
            .await
        });

        wait_for_mock_phase("first WebSocket request", || async {
            state.ws_requests.lock().await.len() == 1
                && state.shared_delay_started.load(Ordering::SeqCst)
        })
        .await;
        assert_eq!(state.connections.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(8)).await;
        state.shared_delay_release.notify_one();
        wait_for_mock_phase("fresh WebSocket request", || async {
            state.connections.load(Ordering::SeqCst) == 2
                && state.ws_requests.lock().await.len() == 2
        })
        .await;
        wait_for_mock_phase("HTTP/SSE fallback request", || async {
            state.http_requests.load(Ordering::SeqCst) == 1
        })
        .await;

        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
        assert_eq!(state.ws_requests.lock().await.len(), 2);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 1);
        assert!(
            rx.try_recv().is_err(),
            "fresh WebSocket must close before public output"
        );
        assert!(!task.is_finished());

        tokio::time::advance(Duration::from_secs(2)).await;
        let error = task
            .await
            .unwrap()
            .expect_err("original deadline wins during HTTP/SSE fallback");
        assert_eq!(error.kind, crate::LlmErrorKind::TimedOut);
        assert_eq!(state.connections.load(Ordering::SeqCst), 2);
        assert_eq!(state.ws_requests.lock().await.len(), 2);
        assert_eq!(state.http_requests.load(Ordering::SeqCst), 1);

        let metrics = capture.finalized().expect("timeout metric");
        assert_eq!(metrics.outcome, crate::LlmAttemptOutcome::TimedOut);
        assert_eq!(metrics.transport, crate::LlmTransport::HttpSse);
        assert_eq!(metrics.stream.provider_event_count, 0);
        assert_eq!(metrics.stream.generation_event_count, 0);
        assert!(!metrics.stream.completed);
        assert_eq!(metrics.total_duration_ms, 10_000);
        assert_eq!(
            capture.finalize_cancelled(),
            Some(metrics),
            "timeout is the single immutable terminal metric"
        );
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
            provider_replay: None,
            responses_replay: vec![],
            tool_availability: phoenix_core::domain::tool_availability::ToolAvailability::all(
                vec![],
            ),
            max_tokens: None,
            effective_effort: phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
            service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
            telemetry: None,
            cache_key: PromptCacheKey::stable("test"),
        }
    }

    fn restricted_request(callable: &[&str]) -> LlmRequest {
        let mut request = empty_request();
        request.tool_availability = phoenix_core::domain::tool_availability::ToolAvailability::new(
            ["bash", "propose_plan"]
                .into_iter()
                .map(|name| super::super::types::ToolDefinition {
                    name: name.into(),
                    description: name.into(),
                    input_schema: serde_json::json!({"type":"object"}),
                    defer_loading: false,
                })
                .collect(),
            callable.iter().map(|name| (*name).to_string()).collect(),
        )
        .unwrap();
        request
    }

    #[test]
    fn tool_policy_uses_distinct_native_shapes_and_advisory_on_unknown_routes() {
        let request = restricted_request(&["bash"]);
        let responses = serde_json::to_value(translate_to_responses_request(
            "gpt-6", &request, false, true,
        ))
        .unwrap();
        assert_eq!(
            responses["tool_choice"],
            serde_json::json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"bash"}]})
        );
        assert_eq!(responses["tools"].as_array().unwrap().len(), 2);
        assert_eq!(responses["store"], false);
        assert_eq!(
            responses["include"],
            serde_json::json!(["reasoning.encrypted_content"])
        );
        let chat = serde_json::to_value(translate_to_chat_request_with_route(
            "gpt-6", &request, true,
        ))
        .unwrap();
        assert_eq!(
            chat["tool_choice"],
            serde_json::json!({"type":"allowed_tools","allowed_tools":{"mode":"auto","tools":[{"type":"function","function":{"name":"bash"}}]}})
        );
        let unknown = serde_json::to_value(translate_to_responses_request(
            "proxy", &request, false, false,
        ))
        .unwrap();
        assert_eq!(unknown["tool_choice"], "auto");
        assert!(unknown["instructions"]
            .as_str()
            .unwrap()
            .contains("propose_plan"));
        let unknown_chat = serde_json::to_value(translate_to_chat_request_with_route(
            "proxy", &request, false,
        ))
        .unwrap();
        assert_eq!(unknown_chat["tool_choice"], "auto");
        assert!(unknown_chat["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("propose_plan"));
        let none = restricted_request(&[]);
        assert_eq!(
            serde_json::to_value(responses_tool_choice(&none, true)).unwrap(),
            "none"
        );
        assert_eq!(
            serde_json::to_value(chat_tool_choice(&none, true)).unwrap(),
            "none"
        );
    }

    fn reasoning_response(id: &str, call: &str) -> serde_json::Value {
        serde_json::json!({"id":id,"model":"gpt-6","status":"completed","usage":{"input_tokens":1,"output_tokens":2},"output":[
            {"type":"reasoning","id":format!("reasoning-{id}"),"summary":[],"encrypted_content":"opaque","unexpected":null},
            {"type":"message","id":format!("message-{id}"),"role":"assistant","status":"completed","content":[{"type":"output_text","text":"Checking","annotations":[]}]},
            {"type":"function_call","id":format!("item-{call}"),"call_id":call,"name":"bash","arguments":"{}","status":"completed"}
        ]})
    }

    #[test]
    fn commentary_without_reasoning_then_reasoning_round_replays_full_envelopes() {
        let mut request = restricted_request(&["bash"]);
        let mut expected = Vec::new();
        for (id, call) in [("r1", "c1"), ("r2", "c2")] {
            let mut wire = reasoning_response(id, call);
            if id == "r1" {
                wire["output"].as_array_mut().unwrap().remove(0);
                wire["output"][0]["phase"] = serde_json::json!("commentary");
            }
            let response =
                normalize_responses_api_response(serde_json::from_value(wire.clone()).unwrap())
                    .unwrap();
            assert_eq!(
                response.content.len(),
                2,
                "private reasoning never becomes public content"
            );
            let Some(ProviderReplayUpdate::Responses(set)) = response.provider_replay else {
                panic!("missing private continuation");
            };
            let set = set.with_owner_message_id(id.into());
            assert_eq!(set.output_items, wire["output"].as_array().unwrap().clone());
            expected.extend(set.output_items.clone());
            request.messages.push(super::super::types::LlmMessage {
                source_message_id: Some(id.into()),
                role: MessageRole::Assistant,
                content: set.public_content.clone(),
            });
            request.responses_replay.push(set);
            request.messages.push(super::super::types::LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: call.into(),
                    content: "unavailable".into(),
                    is_error: true,
                    images: vec![],
                }],
            });
            expected.push(serde_json::json!({"type":"function_call_output","call_id":call,"output":"Error: unavailable"}));
        }
        validate_responses_replay(&request, "gpt-6").unwrap();
        let translated = serde_json::to_value(translate_to_responses_request(
            "gpt-6", &request, false, true,
        ))
        .unwrap();
        assert_eq!(translated["input"], serde_json::Value::Array(expected));
        request.messages[0].content.clear();
        assert!(validate_responses_replay(&request, "gpt-6").is_err());
    }

    #[test]
    fn tool_round_without_reasoning_appends_its_full_output_then_terminal_clears() {
        let mut wire = reasoning_response("r1", "c1");
        wire["output"].as_array_mut().unwrap().remove(0);
        let response =
            normalize_responses_api_response(serde_json::from_value(wire.clone()).unwrap())
                .unwrap();
        assert!(!response.end_turn);
        let Some(ProviderReplayUpdate::Responses(set)) = response.provider_replay else {
            panic!("tool continuation must retain original output envelopes");
        };
        assert_eq!(set.output_items, wire["output"].as_array().unwrap().clone());
        wire["output"].as_array_mut().unwrap().pop();
        let response =
            normalize_responses_api_response(serde_json::from_value(wire).unwrap()).unwrap();
        assert!(response.end_turn);
        assert!(matches!(
            response.provider_replay,
            Some(ProviderReplayUpdate::Clear)
        ));
    }

    async fn collect_completed_output(wire: &serde_json::Value) -> ResponsesStreamAccumulator {
        let mut accumulator = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        for index in [2, 0, 1] {
            let event = serde_json::json!({"type":"response.output_item.done","output_index":index,"item":wire["output"][index]});
            accumulator
                .process_event("response.output_item.done", &event.to_string(), &tx)
                .await
                .unwrap();
        }
        accumulator
    }

    async fn finish_with_terminal_output(
        mut accumulator: ResponsesStreamAccumulator,
        wire: &serde_json::Value,
        terminal_output: serde_json::Value,
    ) -> LlmResponse {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut terminal = wire.clone();
        terminal["output"] = terminal_output;
        let event = serde_json::json!({"type":"response.completed","response":terminal});
        accumulator
            .process_event("response.completed", &event.to_string(), &tx)
            .await
            .unwrap();
        accumulator.into_response().unwrap()
    }

    fn assert_exact_completed_replay(response: &LlmResponse, wire: &serde_json::Value) {
        assert!(!response.end_turn);
        assert_eq!(response.content.len(), 2);
        let Some(ProviderReplayUpdate::Responses(set)) = &response.provider_replay else {
            panic!("completed function call and reasoning must remain replayable");
        };
        assert_eq!(set.output_items, wire["output"].as_array().unwrap().clone());
    }

    #[tokio::test]
    async fn complete_terminal_superset_recovers_tool_round_from_completed_commentary() {
        let mut wire = reasoning_response("r1", "c1");
        wire["output"].as_array_mut().unwrap().swap(0, 1);
        wire["output"][0]["phase"] = serde_json::json!("commentary");
        let mut accumulator = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let event = serde_json::json!({
            "type":"response.output_item.done",
            "output_index":0,
            "item":wire["output"][0]
        });
        accumulator
            .process_event("response.output_item.done", &event.to_string(), &tx)
            .await
            .unwrap();
        let response =
            finish_with_terminal_output(accumulator, &wire, wire["output"].clone()).await;
        assert_exact_completed_replay(&response, &wire);
    }

    #[tokio::test]
    async fn empty_terminal_output_preserves_completed_text_tool_and_reasoning() {
        let wire = reasoning_response("r1", "c1");
        let accumulator = collect_completed_output(&wire).await;
        let response = finish_with_terminal_output(accumulator, &wire, serde_json::json!([])).await;
        assert_exact_completed_replay(&response, &wire);
    }

    #[tokio::test]
    async fn partial_terminal_output_preserves_completed_envelopes_at_original_ordinals() {
        let wire = reasoning_response("r1", "c1");
        for terminal in [
            serde_json::json!([wire["output"][2]]),
            serde_json::json!([wire["output"][1]]),
            serde_json::json!([{"type":"function_call","id":"item-c1","call_id":"c1"}]),
        ] {
            let accumulator = collect_completed_output(&wire).await;
            let response = finish_with_terminal_output(accumulator, &wire, terminal).await;
            assert_exact_completed_replay(&response, &wire);
        }
    }

    #[tokio::test]
    async fn partial_terminal_enrichment_adds_encryption_without_erasing_other_items() {
        let wire = reasoning_response("r1", "c1");
        let mut collected = wire.clone();
        collected["output"][0]
            .as_object_mut()
            .unwrap()
            .remove("encrypted_content");
        let accumulator = collect_completed_output(&collected).await;
        let response =
            finish_with_terminal_output(accumulator, &wire, serde_json::json!([wire["output"][0]]))
                .await;
        assert_exact_completed_replay(&response, &wire);
    }

    #[tokio::test]
    async fn streaming_replay_retains_terminal_encrypted_reasoning_and_output_order() {
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let wire = reasoning_response("r1", "c1");
        for index in [2, 0, 1] {
            let mut item = wire["output"][index].clone();
            if index == 0 {
                item.as_object_mut().unwrap().remove("encrypted_content");
            }
            let event = serde_json::json!({"type":"response.output_item.done","output_index":index,"item":item});
            acc.process_event("response.output_item.done", &event.to_string(), &tx)
                .await
                .unwrap();
        }
        let complete = serde_json::json!({"type":"response.completed","response":wire});
        acc.process_event("response.completed", &complete.to_string(), &tx)
            .await
            .unwrap();
        let response = acc.into_response().unwrap();
        let Some(ProviderReplayUpdate::Responses(set)) = response.provider_replay else {
            panic!("missing private continuation");
        };
        assert_eq!(set.output_items, wire["output"].as_array().unwrap().clone());
        assert_eq!(response.content.len(), 2);
    }

    #[test]
    fn explicit_effort_serializes_on_platform_and_native_default_omits_reasoning() {
        let mut request = empty_request();
        request.max_tokens = Some(16_384);

        let native = serde_json::to_value(translate_to_responses_request(
            "gpt-5.6-sol",
            &request,
            false,
            true,
        ))
        .unwrap();
        assert!(native.get("reasoning").is_none());
        assert_eq!(native["max_output_tokens"], 16_384);

        request.effective_effort =
            phoenix_core::domain::llm_types::EffectiveEffort::native_known(ModelEffort::Medium);
        let native_known = serde_json::to_value(translate_to_responses_request(
            "gpt-5.6-sol",
            &request,
            false,
            true,
        ))
        .unwrap();
        assert!(native_known.get("reasoning").is_none());

        request.effective_effort =
            phoenix_core::domain::llm_types::EffectiveEffort::explicit(ModelEffort::Max);
        let explicit = serde_json::to_value(translate_to_responses_request(
            "gpt-5.6-sol",
            &request,
            false,
            true,
        ))
        .unwrap();
        assert_eq!(explicit["reasoning"]["effort"], "max");
        assert_eq!(explicit["max_output_tokens"], 16_384);
    }

    #[test]
    fn astra_fast_tier_serializes_on_direct_and_codex_routes() {
        let mut request = empty_request();
        request.service_tier = phoenix_core::domain::llm_types::EffectiveServiceTier::Fast;

        let direct = serde_json::to_value(translate_to_responses_request(
            "gpt-6-astra",
            &request,
            false,
            true,
        ))
        .unwrap();
        let codex = serde_json::to_value(translate_to_responses_request(
            "gpt-6-astra",
            &request,
            true,
            true,
        ))
        .unwrap();

        assert_eq!(direct["service_tier"], "priority");
        assert_eq!(codex["service_tier"], "priority");
    }

    #[test]
    fn gpt6_sol_luna_fast_tier_serializes_on_direct_and_codex_routes() {
        let mut request = empty_request();
        request.service_tier = phoenix_core::domain::llm_types::EffectiveServiceTier::Fast;
        for model in ["gpt-6-sol", "gpt-6-luna"] {
            let direct =
                serde_json::to_value(translate_to_responses_request(model, &request, false, true))
                    .unwrap();
            let codex =
                serde_json::to_value(translate_to_responses_request(model, &request, true, false))
                    .unwrap();
            assert_eq!(direct["service_tier"], "priority");
            assert_eq!(codex["service_tier"], "priority");
        }
    }

    #[test]
    fn gpt_61_sol_uses_supported_responses_routes_and_effort() {
        let mut request = empty_request();
        request.service_tier = phoenix_core::domain::llm_types::EffectiveServiceTier::Fast;
        request.effective_effort =
            phoenix_core::domain::llm_types::EffectiveEffort::explicit(ModelEffort::Max);
        let direct = serde_json::to_value(translate_to_backend_request(
            "gpt-6.1-sol",
            &request,
            false,
            true,
        ))
        .unwrap();
        assert_eq!(direct["model"], "gpt-6.1-sol");
        assert_eq!(direct["reasoning"]["effort"], "max");
        assert_eq!(direct["service_tier"], "priority");
        assert!(direct.get("prompt_cache_options").is_some());
        assert!(matches!(
            translate_to_backend_request("gpt-6.1-sol", &request, true, false),
            ResponsesBackendRequest::CodexLite(_)
        ));
        assert!(supports_responses_lite("gpt-6.1-sol"));
        let custom = serde_json::to_value(translate_to_backend_request(
            "gpt-6.1-sol",
            &request,
            false,
            false,
        ))
        .unwrap();
        assert!(custom.get("service_tier").is_none());
        assert!(custom.get("prompt_cache_options").is_none());
    }

    #[test]
    fn future_gpt6_model_does_not_inherit_known_model_capabilities() {
        let mut request = empty_request();
        request.service_tier = phoenix_core::domain::llm_types::EffectiveServiceTier::Fast;
        let direct = serde_json::to_value(translate_to_responses_request(
            "gpt-6-future",
            &request,
            false,
            true,
        ))
        .unwrap();
        assert!(direct.get("service_tier").is_none());
        assert!(!supports_responses_lite("gpt-6-future"));
        assert!(!supports_explicit_prompt_cache("gpt-6-future"));
    }

    #[test]
    fn gpt6_sol_luna_omit_fast_and_explicit_cache_on_custom_routes() {
        let mut request = empty_request();
        request.service_tier = phoenix_core::domain::llm_types::EffectiveServiceTier::Fast;
        for model in ["gpt-6-sol", "gpt-6-luna"] {
            let custom = serde_json::to_value(translate_to_responses_request(
                model, &request, false, false,
            ))
            .unwrap();
            assert!(custom.get("service_tier").is_none());
            assert!(custom.get("prompt_cache_options").is_none());
        }
    }

    #[test]
    fn astra_explicit_cache_controls_are_omitted_on_custom_routes() {
        let request = empty_request();

        let custom = serde_json::to_value(translate_to_responses_request(
            "gpt-6-astra",
            &request,
            false,
            false,
        ))
        .unwrap();

        assert!(custom.get("prompt_cache_options").is_none());
    }

    #[test]
    fn astra_fast_tier_is_supported_on_explicit_canonical_route() {
        assert!(is_official_responses_route(Some(
            "https://api.openai.com/v1/responses"
        )));
    }

    #[test]
    fn astra_fast_tier_is_omitted_on_custom_responses_routes() {
        let mut request = empty_request();
        request.service_tier = phoenix_core::domain::llm_types::EffectiveServiceTier::Fast;

        let custom = serde_json::to_value(translate_to_responses_request(
            "gpt-6-astra",
            &request,
            false,
            false,
        ))
        .unwrap();

        assert!(custom.get("service_tier").is_none());
    }

    #[test]
    fn astra_uses_responses_lite_only_on_the_codex_route() {
        let request = empty_request();

        let codex = translate_to_backend_request("gpt-6-astra", &request, true, true);
        let platform = translate_to_backend_request("gpt-6-astra", &request, false, true);

        assert!(matches!(codex, ResponsesBackendRequest::CodexLite(_)));
        assert!(matches!(platform, ResponsesBackendRequest::Platform(_)));
    }

    #[test]
    fn gpt6_sol_luna_use_responses_lite_only_on_codex_route() {
        let request = empty_request();
        for model in ["gpt-6-sol", "gpt-6-luna"] {
            assert!(matches!(
                translate_to_backend_request(model, &request, true, false),
                ResponsesBackendRequest::CodexLite(_)
            ));
            assert!(matches!(
                translate_to_backend_request(model, &request, false, true),
                ResponsesBackendRequest::Platform(_)
            ));
        }
    }

    #[test]
    fn codex_lite_composes_effort_with_reasoning_context() {
        let mut request = empty_request();
        request.effective_effort =
            phoenix_core::domain::llm_types::EffectiveEffort::explicit(ModelEffort::High);
        let translated = translate_to_backend_request("gpt-5.6-sol", &request, true, true);
        let json = serde_json::to_value(translated).unwrap();

        assert_eq!(json["reasoning"]["context"], "all_turns");
        assert_eq!(json["reasoning"]["effort"], "high");
    }

    #[test]
    fn present_output_details_may_omit_reasoning_tokens() {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "output_tokens_details": {}
            }
        }))
        .unwrap();
        assert_eq!(
            response
                .usage
                .output_tokens_details
                .unwrap()
                .reasoning_tokens,
            None
        );
    }

    #[test]
    fn reasoning_tokens_are_preserved_as_output_subset() {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "status": "completed",
            "output": [{"type":"message","content":[{"type":"output_text","text":"ok"}]}],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 25,
                "output_tokens_details": {"reasoning_tokens": 20}
            }
        }))
        .unwrap();

        let normalized = normalize_responses_api_response(response).unwrap();
        assert_eq!(normalized.usage.output_tokens, 25);
        assert_eq!(normalized.usage.reasoning_tokens, Some(20));
    }

    #[test]
    fn missing_reasoning_tokens_stays_absent() {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "status": "completed",
            "output": [{"type":"message","content":[{"type":"output_text","text":"ok"}]}],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 25
            }
        }))
        .unwrap();

        let normalized = normalize_responses_api_response(response).unwrap();
        assert_eq!(normalized.usage.reasoning_tokens, None);
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
            source_message_id: None,
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

        let translated = translate_to_responses_request("gpt-5.5", &req, false, true);
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
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text("first question")],
            },
            LlmMessage {
                source_message_id: None,
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text("prior answer")],
            },
            LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text("follow-up question")],
            },
        ];

        let translated = translate_to_responses_request("gpt-5.6-sol", &req, false, true);
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
                    .is_some_and(|part| part["type"] == "input_text")
        });
        assert!(
            earlier_user_marked,
            "explicit cache pass should still mark the earlier user message"
        );

        // Positively assert the replayed assistant turn's wire shape so the
        // assistant checks above cannot pass vacuously: it must be a message
        // item with role "assistant" and plain-string content.
        let assistant = input
            .iter()
            .find(|item| item["role"] == "assistant")
            .expect("replayed assistant turn present in input");
        assert_eq!(assistant["type"], "message");
        assert_eq!(assistant["content"], "prior answer");
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
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("history {i}"))],
            })
            .collect();
        req.messages.push(LlmMessage {
            source_message_id: None,
            role: MessageRole::User,
            content: vec![ContentBlock::text("prepare continuation handoff")],
        });

        let translated = translate_to_responses_request("gpt-5.5", &req, true, true);
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
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("history {i}"))],
            })
            .collect();
        req.messages.push(LlmMessage {
            source_message_id: None,
            role: MessageRole::User,
            content: vec![ContentBlock::text("prepare continuation handoff")],
        });

        let translated = translate_to_backend_request("gpt-5.6-sol", &req, true, true);
        let ResponsesBackendRequest::CodexLite(translated) = translated else {
            panic!("GPT-5.6 Codex must use Responses Lite");
        };
        assert_eq!(translated.input.len(), history_cap + 1 + 2);
        assert_eq!(translated.input.len(), limits.max_input_items().unwrap());
    }

    #[tokio::test]
    async fn test_request_tags_omitted_when_none() {
        let req = translate_to_responses_request("gpt-5.5", &empty_request(), false, true);
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
        fn usage_limit_reached_threads_explicit_credits_depletion_type() {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-codex-rate-limit-reached-type",
                HeaderValue::from_static("workspace_member_credits_depleted"),
            );
            let body = r#"{"error":{"type":"usage_limit_reached","plan_type":"team"}}"#;
            let err = parse_codex_error(429, &headers, body).expect("parsed");
            assert_eq!(
                err.quota.as_ref().unwrap().rate_limit_reached_type,
                Some(crate::rate_limit::RateLimitReachedType::WorkspaceMemberCreditsDepleted)
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
            assert_eq!(quota.limit_id.as_deref(), Some("codex-other"));
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
        let mut req = translate_to_responses_request("gpt-5.5", &empty_request(), false, true);
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
            classify_responses_error("invalid_prompt", "x").kind,
            LlmErrorKind::PromptRejected
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
    fn access_program_rejection_allows_manual_recovery_on_http_and_websocket() {
        let message = "The access_programs parameter is not enabled for this organization.";
        let error =
            serde_json::json!({"message": message, "type": "invalid_request_error", "code": null});
        let http = responses_http_error(400, &serde_json::json!({"error": error}).to_string());
        let websocket = parse_wrapped_codex_websocket_error(&serde_json::json!({
            "type": "error", "status": 400, "error": error
        }))
        .expect("provider rejection");

        for rejection in [http, websocket] {
            assert_eq!(rejection.kind, super::super::LlmErrorKind::InvalidRequest);
            assert!(rejection.message.contains(message));
            assert!(!rejection.kind.is_auto_retryable());
            assert!(rejection.kind.is_user_resumable());
        }
    }

    #[test]
    fn responses_http_error_routes_provider_code_through_classifier() {
        use super::super::LlmErrorKind;

        let prompt_rejected = responses_http_error(
            400,
            r#"{"error":{"message":"prompt rejected by policy","type":"invalid_request_error","code":"invalid_prompt"}}"#,
        );
        assert_eq!(prompt_rejected.kind, LlmErrorKind::PromptRejected);
        assert!(prompt_rejected.kind.is_user_resumable());

        let generic_invalid = responses_http_error(
            400,
            r#"{"error":{"message":"unsupported input","type":"invalid_request_error","code":"invalid_request_error"}}"#,
        );
        assert_eq!(generic_invalid.kind, LlmErrorKind::InvalidRequest);
        assert!(generic_invalid.kind.is_user_resumable());

        let unknown_client_code = responses_http_error(
            404,
            r#"{"error":{"message":"model does not exist","type":"invalid_request_error","code":"model_not_found"}}"#,
        );
        assert_eq!(unknown_client_code.kind, LlmErrorKind::InvalidRequest);
        assert!(!unknown_client_code.kind.is_auto_retryable());
    }

    #[tokio::test]
    async fn process_event_returns_err_on_top_level_error() {
        use super::super::LlmErrorKind;
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
        let data = r#"{"type":"error","code":"rate_limit_exceeded","message":"slow down"}"#;
        let err = acc.process_event("error", data, &tx).await.unwrap_err();
        assert_eq!(err.kind, LlmErrorKind::RateLimit);
    }

    // --- streaming SSE accumulator robustness ---

    #[tokio::test]
    async fn process_event_malformed_sse_data_is_invalid_response_not_panic() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
        acc.process_event("", r#"{"type":"response.in_progress"}"#, &tx)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn process_event_empty_dispatch_type_is_ignored_and_logged_once() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
    async fn reasoning_text_delta_records_generation_event() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());

        acc.process_event(
            "response.reasoning_text.delta",
            r#"{"type":"response.reasoning_text.delta","delta":"reasoning"}"#,
            &tx,
        )
        .await
        .unwrap();

        let telemetry = acc.telemetry.snapshot(false);
        assert_eq!(telemetry.generation_event_count, 1);
        assert_eq!(telemetry.visible_text_event_count, 0);
        assert!(telemetry.dispatch_to_first_generation_event_ms.is_some());
    }

    #[tokio::test]
    async fn refusal_delta_records_generated_visible_text() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());

        acc.process_event(
            "response.refusal.delta",
            r#"{"type":"response.refusal.delta","delta":"I can't help with that."}"#,
            &tx,
        )
        .await
        .unwrap();

        let super::super::TokenChunk::Text(text) =
            rx.try_recv().expect("refusal delta should be enqueued")
        else {
            panic!("expected refusal delta as text chunk");
        };
        assert_eq!(text, "I can't help with that.");
        let telemetry = acc.telemetry.snapshot(false);
        assert_eq!(telemetry.generation_event_count, 1);
        assert_eq!(telemetry.visible_text_event_count, 1);
        assert!(telemetry.dispatch_to_first_generation_event_ms.is_some());
    }

    #[tokio::test]
    async fn process_event_assembles_message_text_from_output_item_done() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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

    #[tokio::test]
    async fn streamed_reasoning_tokens_are_preserved_when_reported() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
        let data = r#"{
            "type":"response.completed",
            "response":{
                "usage":{
                    "input_tokens":10,
                    "output_tokens":25,
                    "output_tokens_details":{"reasoning_tokens":20}
                },
                "output":[{"type":"message","role":"assistant","content":[
                    {"type":"output_text","text":"answer"}
                ]}]
            }
        }"#;

        acc.process_event("response.completed", data, &tx)
            .await
            .expect("handler should preserve terminal usage");
        let response = acc.into_response().expect("stream should normalize");

        assert_eq!(response.usage.reasoning_tokens, Some(20));
    }

    /// `OpenAI`'s cached-read and cache-write details are both subsets of
    /// `input_tokens`; normalization splits both without changing context usage.
    #[tokio::test]
    async fn responses_api_cache_details_are_threaded_without_double_counting() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
            id: String::new(),
            model: String::new(),
            status: "completed".to_string(),
            output: acc.output_items.into_values().collect(),
            usage: ResponsesApiUsage {
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: acc.cached_tokens,
                    cache_write_tokens: acc.cache_write_tokens,
                },
                output_tokens_details: None,
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
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("stable-{i}"))],
            });
        }
        request.messages.push(LlmMessage {
            source_message_id: None,
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
            true,
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
                serde_json::to_value(translate_to_responses_request(model, &request, codex, true))
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
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("stable-{i}"))],
            });
        }

        let wire = serde_json::to_value(translate_to_responses_request(
            "gpt-5.6", &request, false, true,
        ))
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

    /// In an alternating conversation, assistant turns are skipped for cache
    /// marking but must not consume the read-marker budget: the full limit of
    /// input-role markers should still be placed even when assistant turns sit
    /// among the newest messages. Regression guard against filtering after the
    /// `take`, which would halve effective cache reads on long histories.
    #[tokio::test]
    async fn explicit_cache_skipped_assistants_do_not_consume_marker_budget() {
        use crate::types::{ContentBlock, LlmMessage, MessageRole};

        let mut request = empty_request();
        // 60 user+assistant pairs -> 60 markable user turns available (more than
        // the 50 limit), each preceded/followed by a skipped assistant turn.
        for i in 0..60 {
            request.messages.push(LlmMessage {
                source_message_id: None,
                role: MessageRole::User,
                content: vec![ContentBlock::text(format!("q-{i}"))],
            });
            request.messages.push(LlmMessage {
                source_message_id: None,
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text(format!("a-{i}"))],
            });
        }
        request.messages.push(LlmMessage {
            source_message_id: None,
            role: MessageRole::User,
            content: vec![ContentBlock::text("latest")],
        });

        let wire = serde_json::to_value(translate_to_responses_request(
            "gpt-5.6", &request, false, true,
        ))
        .unwrap();
        assert_eq!(
            wire.to_string().matches("prompt_cache_breakpoint").count(),
            50,
            "skipped assistant turns must not consume the 50-marker budget"
        );
        // Every marker must sit on a user message — an assistant message never
        // gains an input_text part.
        for item in wire["input"].as_array().unwrap() {
            if item.to_string().contains("prompt_cache_breakpoint") {
                assert_eq!(item["role"], "user");
            }
        }
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
            id: String::new(),
            model: String::new(),
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
            id: String::new(),
            model: String::new(),
            status: "completed".to_string(),
            output: vec![],
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens: 42,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
                output_tokens_details: None,
            },
        })
        .expect_err("empty content with billed output tokens must fail");
        assert_eq!(err.kind, crate::LlmErrorKind::ServerError);
        assert!(
            err.kind.is_auto_retryable(),
            "a lost-message response must be retryable so the executor retries"
        );
    }

    #[tokio::test]
    async fn completed_reasoning_only_item_is_valid_quiet_turn() {
        let response = normalize_responses_api_response(ResponsesApiResponse {
            id: "resp-reasoning-only".to_string(),
            model: "gpt-test".to_string(),
            status: "completed".to_string(),
            output: vec![ResponsesApiOutput(serde_json::json!({
                "type": "reasoning",
                "id": "reasoning-1",
                "summary": [{"type": "summary_text", "text": "bounded fixture"}],
                "encrypted_content": "fixture-ciphertext",
                "status": "completed"
            }))],
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens: 18,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
                output_tokens_details: Some(ResponsesApiOutputTokensDetails {
                    reasoning_tokens: Some(16),
                }),
            },
        })
        .expect("a completed reasoning-only response is a valid quiet turn");

        assert!(response.content.is_empty());
        assert!(response.end_turn);
        assert_eq!(response.usage.output_tokens, 18);
        assert_eq!(response.usage.reasoning_tokens, Some(16));
        assert_eq!(response.provider_replay, Some(ProviderReplayUpdate::Clear));
    }

    #[tokio::test]
    async fn completed_reasoning_with_empty_message_companion_is_valid_quiet_turn() {
        let output = vec![
            serde_json::json!({
                "type": "reasoning",
                "id": "reasoning-1",
                "summary": [],
                "status": "completed"
            }),
            serde_json::json!({
                "type": "message",
                "id": "message-1",
                "status": "completed",
                "role": "assistant",
                "content": []
            }),
        ];

        let response =
            normalize_responses_api_response(reasoning_only_response(output, 50, Some(44)))
                .expect("a completed empty message companion carries no public output");

        assert!(response.content.is_empty());
        assert!(response.end_turn);
        assert_eq!(response.usage.output_tokens, 50);
        assert_eq!(response.usage.reasoning_tokens, Some(44));
        assert_eq!(response.provider_replay, Some(ProviderReplayUpdate::Clear));
    }

    #[tokio::test]
    async fn malformed_empty_message_companions_remain_retryable_errors() {
        for message in [
            serde_json::json!({
                "type": "message", "id": "message-1", "status": "incomplete",
                "role": "assistant", "content": []
            }),
            serde_json::json!({
                "type": "message", "id": "message-1", "status": "completed",
                "role": "user", "content": []
            }),
            serde_json::json!({
                "type": "message", "id": "", "status": "completed",
                "role": "assistant", "content": []
            }),
            serde_json::json!({
                "type": "message", "id": "message-1", "status": "completed",
                "role": "assistant", "content": [{"type": "unknown"}]
            }),
        ] {
            let output = vec![
                serde_json::json!({
                    "type": "reasoning", "id": "reasoning-1", "summary": [],
                    "status": "completed"
                }),
                message,
            ];
            let error =
                normalize_responses_api_response(reasoning_only_response(output, 50, Some(44)))
                    .expect_err("only a structurally completed empty assistant message is inert");
            assert_eq!(error.kind, crate::LlmErrorKind::ServerError);
            assert!(error.kind.is_auto_retryable());
        }
    }

    #[tokio::test]
    async fn invalid_reasoning_usage_boundaries_remain_retryable_errors() {
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "reasoning-1",
            "summary": []
        });
        for reasoning_tokens in [None, Some(0), Some(19)] {
            let err = normalize_responses_api_response(reasoning_only_response(
                vec![item.clone()],
                18,
                reasoning_tokens,
            ))
            .expect_err("reasoning usage must be a positive subset of output usage");

            assert_eq!(err.kind, crate::LlmErrorKind::ServerError);
            assert!(err.kind.is_auto_retryable());
        }
    }

    fn reasoning_only_response(
        output: Vec<serde_json::Value>,
        output_tokens: u32,
        reasoning_tokens: Option<u32>,
    ) -> ResponsesApiResponse {
        ResponsesApiResponse {
            id: "resp-reasoning-fixture".to_string(),
            model: "gpt-test".to_string(),
            status: "completed".to_string(),
            output: output.into_iter().map(ResponsesApiOutput).collect(),
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
                output_tokens_details: reasoning_tokens.map(|reasoning_tokens| {
                    ResponsesApiOutputTokensDetails {
                        reasoning_tokens: Some(reasoning_tokens),
                    }
                }),
            },
        }
    }

    #[tokio::test]
    async fn malformed_or_incomplete_reasoning_items_remain_retryable_errors() {
        for item in [
            serde_json::json!({"type": "reasoning", "id": "", "summary": []}),
            serde_json::json!({
                "type": "reasoning",
                "id": "reasoning-1",
                "summary": [],
                "status": "incomplete"
            }),
            serde_json::json!({
                "type": "reasoning",
                "id": "reasoning-1",
                "summary": [{"type": "unknown", "text": "fixture"}]
            }),
        ] {
            let err =
                normalize_responses_api_response(reasoning_only_response(vec![item], 18, Some(16)))
                    .expect_err("malformed reasoning must not prove a quiet completion");
            assert_eq!(err.kind, crate::LlmErrorKind::ServerError);
        }
    }

    #[tokio::test]
    async fn reasoning_item_without_positive_usage_is_not_an_unbilled_empty_turn() {
        for item in [
            serde_json::json!({"type": "reasoning", "id": "", "summary": []}),
            serde_json::json!({
                "type": "reasoning",
                "id": "reasoning-1",
                "summary": [],
                "status": "incomplete"
            }),
        ] {
            let err =
                normalize_responses_api_response(reasoning_only_response(vec![item], 0, None))
                    .expect_err("a reasoning item requires positive reasoning usage");
            assert_eq!(err.kind, crate::LlmErrorKind::ServerError);
            assert!(err.kind.is_auto_retryable());
        }
    }

    #[tokio::test]
    async fn reasoning_usage_without_items_or_with_mixed_output_remains_retryable() {
        let empty = normalize_responses_api_response(reasoning_only_response(vec![], 18, Some(16)))
            .expect_err("usage without a reasoning item is not terminal evidence");
        assert_eq!(empty.kind, crate::LlmErrorKind::ServerError);

        let mixed = normalize_responses_api_response(reasoning_only_response(
            vec![
                serde_json::json!({"type": "reasoning", "id": "r1", "summary": []}),
                serde_json::json!({"type": "message", "id": "m1", "content": []}),
            ],
            18,
            Some(16),
        ))
        .expect_err("a lost message beside reasoning is not a quiet completion");
        assert_eq!(mixed.kind, crate::LlmErrorKind::ServerError);
    }

    #[tokio::test]
    async fn observed_non_reasoning_output_without_completed_message_remains_retryable() {
        let err = normalize_responses_api_response_with_evidence(
            reasoning_only_response(
                vec![serde_json::json!({
                    "type": "reasoning",
                    "id": "reasoning-1",
                    "summary": []
                })],
                18,
                Some(16),
            ),
            true,
        )
        .expect_err("observed visible output cannot disappear at the terminal boundary");
        assert_eq!(err.kind, crate::LlmErrorKind::ServerError);
        assert!(err.kind.is_auto_retryable());
    }

    #[tokio::test]
    async fn observed_non_reasoning_output_loss_is_retryable_on_websocket_and_sse() {
        let request = empty_request();
        let terminal = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp-lost-visible-output",
                "status": "completed",
                "output": [
                    {"type": "reasoning", "id": "reasoning-1", "summary": []},
                    {
                        "type": "message", "id": "message-1", "status": "completed",
                        "role": "assistant", "content": []
                    }
                ],
                "usage": {
                    "input_tokens": 1000,
                    "output_tokens": 18,
                    "output_tokens_details": {"reasoning_tokens": 16}
                }
            }
        })
        .to_string();
        let finalized_events = [
            serde_json::json!({
                "type": "response.output_text.done",
                "text": "final answer"
            }),
            serde_json::json!({
                "type": "response.refusal.done",
                "refusal": "final refusal"
            }),
            serde_json::json!({
                "type": "response.content_part.done",
                "part": {"type": "output_text", "text": "final content part"}
            }),
            serde_json::json!({
                "type": "response.content_part.added",
                "part": {"type": "refusal", "refusal": "added refusal part"}
            }),
            serde_json::json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "added message text"}]
                }
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "delta": "{}"
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.done",
                "arguments": "{}"
            }),
            serde_json::json!({
                "type": "response.output_item.added",
                "item": {"type": "function_call", "name": "get_weather"}
            }),
        ];
        let (chunk_tx, _chunk_rx) = tokio::sync::mpsc::channel(8);

        for finalized in finalized_events {
            let event_type = finalized["type"].as_str().unwrap();
            let finalized = finalized.to_string();

            let mut websocket = ResponsesStreamAccumulator::new(Instant::now(), &request);
            websocket
                .process_event(event_type, &finalized, &chunk_tx)
                .await
                .unwrap();
            websocket
                .process_event("response.completed", &terminal, &chunk_tx)
                .await
                .unwrap();
            let websocket_error = finalize_websocket_response(websocket, "gpt-test")
                .expect_err("observed non-reasoning output cannot disappear on WebSocket");
            let CodexWsError::Backend(websocket_error) = websocket_error else {
                panic!("non-reasoning output loss must remain a provider error");
            };
            assert_eq!(websocket_error.kind, crate::LlmErrorKind::ServerError);
            assert!(websocket_error.kind.is_auto_retryable());

            let mut sse = ResponsesStreamAccumulator::new(Instant::now(), &request);
            sse.process_event(event_type, &finalized, &chunk_tx)
                .await
                .unwrap();
            sse.process_event("response.completed", &terminal, &chunk_tx)
                .await
                .unwrap();
            let sse_error = finalize_responses_stream(sse, "gpt-test")
                .expect_err("observed non-reasoning output cannot disappear on SSE");
            assert_eq!(sse_error.kind, crate::LlmErrorKind::ServerError);
            assert!(sse_error.kind.is_auto_retryable());
        }
    }

    #[tokio::test]
    async fn quiet_reasoning_terminals_have_websocket_sse_finalizer_parity() {
        let request = empty_request();
        let cases = [
            (
                serde_json::json!([
                    {"type": "reasoning", "id": "reasoning-only", "summary": []}
                ]),
                18,
                16,
            ),
            (
                serde_json::json!([
                    {"type": "reasoning", "id": "reasoning-with-message", "summary": []},
                    {
                        "type": "message", "id": "message-1", "status": "completed",
                        "role": "assistant", "content": []
                    }
                ]),
                50,
                44,
            ),
        ];
        let (chunk_tx, _chunk_rx) = tokio::sync::mpsc::channel(8);

        for (output, output_tokens, reasoning_tokens) in cases {
            let terminal = serde_json::json!({
                "type": "response.completed",
                "response": {
                    "id": "resp-quiet-reasoning-parity",
                    "status": "completed",
                    "output": output,
                    "usage": {
                        "input_tokens": 1000,
                        "output_tokens": output_tokens,
                        "output_tokens_details": {"reasoning_tokens": reasoning_tokens}
                    }
                }
            })
            .to_string();

            let mut websocket = ResponsesStreamAccumulator::new(Instant::now(), &request);
            websocket
                .process_event("response.completed", &terminal, &chunk_tx)
                .await
                .unwrap();
            let websocket = finalize_websocket_response(websocket, "gpt-test").unwrap();

            let mut sse = ResponsesStreamAccumulator::new(Instant::now(), &request);
            sse.process_event("response.completed", &terminal, &chunk_tx)
                .await
                .unwrap();
            let sse = finalize_responses_stream(sse, "gpt-test").unwrap();

            for response in [&websocket, &sse] {
                assert!(response.content.is_empty());
                assert!(response.end_turn);
                assert_eq!(response.usage.output_tokens, output_tokens);
                assert_eq!(response.usage.reasoning_tokens, Some(reasoning_tokens));
                assert_eq!(response.provider_replay, Some(ProviderReplayUpdate::Clear));
            }
        }
    }

    #[test]
    fn responses_sse_preterminal_eof_is_retryable_network_error() {
        let request = empty_request();
        let acc = ResponsesStreamAccumulator::new(Instant::now(), &request);

        let err = finalize_responses_stream(acc, "gpt-test")
            .expect_err("pre-terminal EOF must not fabricate completed status");

        assert_eq!(err.kind, crate::LlmErrorKind::Network);
        assert!(err.kind.is_auto_retryable());
    }

    /// A `refusal` message part is the model's actual reply — it declined.
    /// It must surface as non-empty text content so the billed-but-empty
    /// guard does not mistake a final answer for a lost message and retry.
    #[tokio::test]
    async fn responses_api_refusal_message_surfaces_as_text_not_retried() {
        let resp = normalize_responses_api_response(ResponsesApiResponse {
            id: String::new(), model: String::new(),
            status: "completed".to_string(),
            output: vec![ResponsesApiOutput(serde_json::json!({"type":"message", "content":[{"type":"refusal", "refusal":"I can't help with that."}]}))],
            usage: ResponsesApiUsage {
                input_tokens: 1000,
                output_tokens: 7,
                input_tokens_details: ResponsesApiInputTokensDetails {
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
                output_tokens_details: None,
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

    #[tokio::test]
    async fn process_event_terminal_output_preserves_items_without_duplication() {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ResponsesStreamAccumulator::new(Instant::now(), &empty_request());
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
            "terminal output must not duplicate completed items"
        );
    }

    #[test]
    fn chat_streaming_request_requests_usage_chunk() {
        let request = empty_request();
        let mut wire = translate_to_chat_request("compatible-chat-model", &request);
        wire.stream = Some(true);
        wire.stream_options = Some(ChatStreamOptions {
            include_usage: true,
        });
        let json = serde_json::to_value(wire).unwrap();
        assert_eq!(json["stream"], true);
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[test]
    fn chat_request_serializes_only_explicit_reasoning_effort() {
        let native = serde_json::to_value(translate_to_chat_request(
            "compatible-chat-model",
            &empty_request(),
        ))
        .expect("serialize native chat request");
        assert!(native.get("reasoning_effort").is_none());

        let mut request = empty_request();
        request.effective_effort =
            phoenix_core::domain::llm_types::EffectiveEffort::explicit(ModelEffort::High);
        let explicit =
            serde_json::to_value(translate_to_chat_request("compatible-chat-model", &request))
                .expect("serialize explicit chat request");
        assert_eq!(explicit["reasoning_effort"], "high");
    }

    #[test]
    fn chat_normalization_preserves_tool_id_and_cached_usage() {
        let response = ChatCompletionsResponse {
            choices: vec![ChatChoice {
                message: ChatResponseMessage {
                    reasoning_content: Some("private reasoning".to_string()),
                    content: None,
                    refusal: None,
                    tool_calls: Some(vec![ChatToolCall {
                        id: "call-7".to_string(),
                        r#type: "function".to_string(),
                        function: ChatFunctionCall {
                            name: "read_file".to_string(),
                            arguments: r#"{"path":"README.md"}"#.to_string(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: Some(ChatUsage {
                prompt_tokens: 1_000,
                completion_tokens: 50,
                prompt_tokens_details: ChatPromptTokensDetails { cached_tokens: 800 },
                completion_tokens_details: ChatCompletionTokensDetails::default(),
            }),
        };
        let normalized = normalize_chat_response(response, "compatible-chat-model").unwrap();
        assert!(!normalized.end_turn);
        assert_eq!(normalized.usage.input_tokens, 200);
        assert_eq!(normalized.usage.cache_read_tokens, 800);
        assert!(matches!(
            &normalized.content[0],
            ContentBlock::ToolUse { id, name, input }
                if id == "call-7" && name == "read_file" && input["path"] == "README.md"
        ));
    }

    #[test]
    fn chat_normalization_preserves_literal_angle_bracket_and_refusal() {
        let literal = normalize_chat_response(
            ChatCompletionsResponse {
                choices: vec![ChatChoice {
                    message: ChatResponseMessage {
                        reasoning_content: None,
                        content: Some("<".to_string()),
                        refusal: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            },
            "compatible-chat-model",
        )
        .unwrap();
        assert!(matches!(
            &literal.content[0],
            ContentBlock::Text { text } if text == "<"
        ));

        let refusal = normalize_chat_response(
            ChatCompletionsResponse {
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
            },
            "compatible-chat-model",
        )
        .unwrap();
        assert!(matches!(
            &refusal.content[0],
            ContentBlock::Text { text } if text == "I can't help with that."
        ));
    }

    #[test]
    fn chat_errors_preserve_numeric_status_and_overload_semantics() {
        for code in [serde_json::json!(401), serde_json::json!("401")] {
            let numeric = classify_chat_stream_error(Some(&code), "credential rejected");
            assert_eq!(numeric.kind, crate::LlmErrorKind::Auth);
        }

        for (status, code) in [(429, "slow_down"), (503, "server_is_overloaded")] {
            let body = serde_json::json!({
                "error": {"message": "at capacity", "code": code}
            })
            .to_string();
            let overload = openai_http_error(status, &status.to_string(), &body);
            assert_eq!(overload.kind, crate::LlmErrorKind::ServerOverloaded);
        }
    }

    #[test]
    fn chat_http_errors_without_codes_remain_terminal_client_errors() {
        for status in [400, 404] {
            for body in [
                serde_json::json!({
                    "error": {"message": "invalid model", "type": "invalid_request_error"}
                }),
                serde_json::json!({
                    "error": {
                        "message": "invalid model",
                        "type": "invalid_request_error",
                        "code": null
                    }
                }),
            ] {
                let error = openai_http_error(status, &status.to_string(), &body.to_string());
                assert_eq!(error.kind, crate::LlmErrorKind::InvalidRequest);
                assert!(!error.kind.is_auto_retryable());
            }
        }
    }

    #[tokio::test]
    async fn chat_stream_surfaces_inline_errors_and_requires_terminal_event() {
        let request = empty_request();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut error_acc = ChatStreamAccumulator::new(Instant::now(), &request);
        let error = error_acc
            .process_event(
                r#"{"error":{"message":"context length exceeded","code":"context_length_exceeded"}}"#,
                &tx,
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, crate::LlmErrorKind::ContextWindowExceeded);

        let mut incomplete = ChatStreamAccumulator::new(Instant::now(), &request);
        incomplete
            .process_event(r#"{"choices":[{"delta":{"content":"partial"}}]}"#, &tx)
            .await
            .unwrap();
        let error = incomplete.into_response().unwrap_err();
        assert_eq!(error.kind, crate::LlmErrorKind::InvalidResponse);
    }

    #[test]
    fn chat_normalization_rejects_malformed_tools_and_usage() {
        let malformed_tool = ChatCompletionsResponse {
            choices: vec![ChatChoice {
                message: ChatResponseMessage {
                    reasoning_content: None,
                    content: None,
                    refusal: None,
                    tool_calls: Some(vec![ChatToolCall {
                        id: "call-1".to_string(),
                        r#type: "function".to_string(),
                        function: ChatFunctionCall {
                            name: "bash".to_string(),
                            arguments: "{".to_string(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: None,
        };
        assert_eq!(
            normalize_chat_response(malformed_tool, "compatible-chat-model")
                .unwrap_err()
                .kind,
            crate::LlmErrorKind::InvalidResponse
        );

        let invalid_usage = ChatCompletionsResponse {
            choices: vec![ChatChoice {
                message: ChatResponseMessage {
                    reasoning_content: None,
                    content: Some("ok".to_string()),
                    refusal: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(ChatUsage {
                prompt_tokens: 10,
                completion_tokens: 1,
                prompt_tokens_details: ChatPromptTokensDetails { cached_tokens: 11 },
                completion_tokens_details: ChatCompletionTokensDetails::default(),
            }),
        };
        assert_eq!(
            normalize_chat_response(invalid_usage, "compatible-chat-model")
                .unwrap_err()
                .kind,
            crate::LlmErrorKind::InvalidResponse
        );
    }

    #[tokio::test]
    async fn chat_stream_rejects_multiple_choices_and_invalid_tool_indices() {
        let request = empty_request();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        for event in [
            r#"{"choices":[{"delta":{"content":"a"}},{"delta":{"content":"b"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1000000,"id":"call","function":{"name":"bash","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"call","function":{"name":"bash","arguments":"{}"}}]}}]}"#,
        ] {
            let mut acc = ChatStreamAccumulator::new(Instant::now(), &request);
            assert_eq!(
                acc.process_event(event, &tx).await.unwrap_err().kind,
                crate::LlmErrorKind::InvalidResponse
            );
        }
    }

    #[tokio::test]
    async fn chat_stream_rejects_choice_changes_and_mutated_tool_identity() {
        let request = empty_request();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut choices = ChatStreamAccumulator::new(Instant::now(), &request);
        choices
            .process_event(r#"{"choices":[{"index":0,"delta":{"content":"a"}}]}"#, &tx)
            .await
            .unwrap();
        assert_eq!(
            choices
                .process_event(r#"{"choices":[{"index":1,"delta":{"content":"b"}}]}"#, &tx,)
                .await
                .unwrap_err()
                .kind,
            crate::LlmErrorKind::InvalidResponse
        );

        let mut tool = ChatStreamAccumulator::new(Instant::now(), &request);
        tool.process_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","type":"function","function":{"name":"bash","arguments":""}}]}}]}"#,
            &tx,
        )
        .await
        .unwrap();
        assert_eq!(
            tool.process_event(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-b","function":{"name":"read_file","arguments":"{}"}}]}}]}"#,
                &tx,
            )
            .await
            .unwrap_err()
            .kind,
            crate::LlmErrorKind::InvalidResponse
        );
    }

    #[tokio::test]
    async fn chat_stream_rejects_mixed_content_and_refusal() {
        let request = empty_request();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ChatStreamAccumulator::new(Instant::now(), &request);
        acc.process_event(r#"{"choices":[{"delta":{"content":"text"}}]}"#, &tx)
            .await
            .unwrap();
        assert_eq!(
            acc.process_event(r#"{"choices":[{"delta":{"refusal":"no"}}]}"#, &tx)
                .await
                .unwrap_err()
                .kind,
            crate::LlmErrorKind::InvalidResponse
        );
    }

    #[tokio::test]
    async fn chat_stream_rejects_incomplete_tool_call() {
        let request = empty_request();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ChatStreamAccumulator::new(Instant::now(), &request);
        acc.process_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            &tx,
        )
        .await
        .unwrap();
        assert_eq!(
            acc.into_response().unwrap_err().kind,
            crate::LlmErrorKind::InvalidResponse
        );
    }

    #[tokio::test]
    async fn chat_stream_preserves_literal_angle_bracket_and_refusal_deltas() {
        let request = empty_request();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut literal = ChatStreamAccumulator::new(Instant::now(), &request);
        literal
            .process_event(
                r#"{"choices":[{"delta":{"content":"<"},"finish_reason":"stop"}]}"#,
                &tx,
            )
            .await
            .unwrap();
        assert!(matches!(rx.try_recv(), Ok(crate::TokenChunk::Text(text)) if text == "<"));
        assert!(matches!(
            &literal.into_response().unwrap().content[0],
            ContentBlock::Text { text } if text == "<"
        ));

        let mut refusal = ChatStreamAccumulator::new(Instant::now(), &request);
        refusal
            .process_event(
                r#"{"choices":[{"delta":{"refusal":"declined"},"finish_reason":"stop"}]}"#,
                &tx,
            )
            .await
            .unwrap();
        assert!(matches!(rx.try_recv(), Ok(crate::TokenChunk::Text(text)) if text == "declined"));
        assert!(matches!(
            &refusal.into_response().unwrap().content[0],
            ContentBlock::Text { text } if text == "declined"
        ));
    }

    #[tokio::test]
    async fn chat_stream_accumulates_tool_fragments_usage_and_telemetry() {
        let request = empty_request();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut acc = ChatStreamAccumulator::new(Instant::now(), &request);
        acc.process_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-9","function":{"name":"bash","arguments":""}}]}}]}"#,
            &tx,
        )
        .await
        .unwrap();
        acc.process_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"pwd\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
            &tx,
        )
        .await
        .unwrap();
        let response = acc.into_response().unwrap();
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 2);
        assert_eq!(response.stream_telemetry.generation_event_count, 2);
        assert!(response.stream_telemetry.completed);
        assert!(matches!(
            &response.content[0],
            ContentBlock::ToolUse { id, name, input }
                if id == "call-9" && name == "bash" && input["cmd"] == "pwd"
        ));
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub fn translate_to_responses_request(
        api_name: &str,
        request: &crate::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, false, true)
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
            true,
        ))
        .expect("request serializes")
    }

    pub fn translate_to_responses_request_codex(
        api_name: &str,
        request: &crate::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request, true, true)
    }
}
