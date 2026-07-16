//! LLM provider abstraction
//!
//! Provides a common interface for interacting with various LLM providers.
//!
//! Phoenix supports direct provider APIs and provider-compatible endpoints via
//! `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL`. A configured base URL is used as-is;
//! Phoenix does not append provider paths.
//!
//! ## Backend contract
//!
//! Each model has a [`ModelBackend`] that couples the route/auth family with the
//! wire protocol. `ModelBackend::Anthropic` uses Anthropic Messages-compatible
//! requests. `ModelBackend::OpenAIResponses` uses `OpenAI` Responses-compatible
//! requests. Externally configured models use the same backend values, so the
//! config says what Phoenix should speak rather than who hosts the model.
//!
//! ## Authentication
//!
//! Direct Anthropic calls use `x-api-key`. Helper-issued credentials may choose
//! an alternate header style for provider-compatible endpoints. `OpenAI`
//! Responses calls use bearer auth. The ChatGPT/Codex bridge only applies to
//! built-in `OpenAI` Responses models.
//!
//! ## Discovery
//!
//! With `LLM_API_KEY_HELPER` and base URL overrides, Phoenix derives `/v1/models`
//! URLs from those base URLs and filters the configured model set to discovered
//! IDs when possible. Discovery is opportunistic: if listing is unavailable,
//! Phoenix falls back to the configured model set.
//!
//! ## Streaming
//!
//! All streaming requests use `Transfer-Encoding: chunked` with
//! `Content-Type: text/event-stream`. Phoenix parses SSE events with
//! chunk-boundary splits, bare `\r` line endings, and multi-line `data:` fields.
//!

mod anthropic;
pub mod codex_credential;
pub mod codex_login;
pub mod credential_helper;
mod discovery;
mod error;
mod headers;
mod mock;
mod models;
mod openai;
#[cfg(test)]
mod proptests;
pub mod rate_limit;
mod registry;
mod service;
pub(crate) mod sse;

pub use codex_credential::{CodexCredential, CODEX_BACKEND_URL, CODEX_BRIDGE_CONTEXT_WINDOW};
pub use credential_helper::{CredentialHelper, CredentialStatus};
pub use discovery::{discover_models, DiscoveredModels, DiscoveryConfig};
pub use error::{LlmAttemptReason, LlmError, LlmErrorKind};
// AutoRetryPolicy / UserResumePolicy live in phoenix-core
// (phoenix_core::domain::retry_policy) and are not re-exported here: nothing
// imports them via a `phoenix_llm::` path. Their only consumer is the persisted
// error-kind schema, which references the domain crate directly.
// Re-exported types: QuotaDetails is consumed by `LlmOutcome::UsageLimitReached`
// and the executor mapper. CreditsSnapshot / RateLimitWindow live behind it,
// accessed via the `rate_limit` submodule.
pub use models::{
    all_models, merge_model_specs, parse_external_models, ModelBackend, ModelInfo, ModelSource,
    ModelSpec,
};
#[allow(unused_imports)]
pub use rate_limit::{CreditsSnapshot, QuotaDetails, RateLimitWindow};
#[allow(unused_imports)]
// CredentialSource + ResolvedAuth + AuthStyle: public API for downstream consumers
pub use registry::{AuthStyle, CredentialSource, LlmAuth, LlmConfig, ModelRegistry, ResolvedAuth};
pub use service::LlmServiceImpl;
// `types` (ContentBlock, Usage, ImageSource, …) live in phoenix-core. Alias
// the module back as `types` and glob-re-export so both `phoenix_llm::types::X`
// and `phoenix_llm::X` paths resolve for downstream consumers.
pub use phoenix_core::domain::llm_types::{self as types, *};

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::Instrument;

/// Chunks emitted during streaming. Only text deltas are forwarded to the UI;
/// tool input fragments are accumulated internally by the provider.
#[derive(Debug, Clone)]
pub enum TokenChunk {
    Text(String),
    /// Mid-stream quota snapshot from the codex backend's
    /// `codex.rate_limits` SSE event. Emitted on every turn (not only
    /// pre-429), so the UI can show usage trends before the user hits the
    /// terminal `UsageLimitReached` state.
    RateLimitSnapshot(QuotaDetails),
}

/// Provider request-shape limits that apply to continuation summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationRequestLimits {
    /// The route has no known request-shape limit beyond its token window.
    TokenWindowOnly,
    /// The route accepts at most `total` provider input items. `prefix_items`
    /// are provider-owned items inserted after history planning and are
    /// therefore subtracted structurally rather than by caller convention.
    MaxInputItems {
        total: std::num::NonZeroUsize,
        prefix_items: usize,
    },
}

impl ContinuationRequestLimits {
    /// Conservative Codex bridge bound. The backend's exact ceiling is not
    /// documented; this reserves one item beyond 900 history messages for the
    /// appended continuation prompt.
    #[must_use]
    pub const fn codex_bridge() -> Self {
        Self::MaxInputItems {
            total: std::num::NonZeroUsize::MIN.saturating_add(900),
            prefix_items: 0,
        }
    }

    /// GPT-5.6 Responses Lite prepends additional-tools and developer-
    /// instructions items after continuation history has been planned.
    #[must_use]
    pub const fn codex_responses_lite() -> Self {
        Self::MaxInputItems {
            total: std::num::NonZeroUsize::MIN.saturating_add(900),
            prefix_items: 2,
        }
    }

    #[must_use]
    pub const fn max_input_items(self) -> Option<usize> {
        match self {
            Self::TokenWindowOnly => None,
            Self::MaxInputItems { total, .. } => Some(total.get()),
        }
    }

    #[must_use]
    pub const fn max_history_messages(self, reserved_input_items: usize) -> Option<usize> {
        match self {
            Self::MaxInputItems {
                total,
                prefix_items,
            } => Some(
                total
                    .get()
                    .saturating_sub(prefix_items)
                    .saturating_sub(reserved_input_items),
            ),
            Self::TokenWindowOnly => None,
        }
    }
}

/// Common interface for LLM providers
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Make a non-streaming completion request
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Streaming completion: emits text chunks via `chunk_tx` as they arrive,
    /// then returns the fully assembled `LlmResponse` (identical to `complete()`).
    /// Default implementation calls `complete()` with no streaming.
    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &mpsc::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        // Default: ignore chunk_tx, fall back to non-streaming
        let _ = chunk_tx;
        self.complete(request).await
    }

    /// Get the model ID
    fn model_id(&self) -> &str;

    /// True if this service routes through the ChatGPT-backend codex bridge.
    /// Consumed by [`crate::ModelSpec::context_window_for`] to apply the
    /// bridge's 272K cap regardless of the model's platform-API ceiling.
    /// Default `false` covers Anthropic, mock, and direct `OpenAI`.
    fn uses_codex_bridge(&self) -> bool {
        false
    }

    /// Request-shape limits for tool-less continuation-summary requests.
    fn continuation_request_limits(&self) -> ContinuationRequestLimits {
        ContinuationRequestLimits::TokenWindowOnly
    }
}

/// Logging wrapper for LLM services
pub struct LoggingService {
    inner: Arc<dyn LlmService>,
    model_id: String,
    provider: &'static str,
    transport: &'static str,
}

impl LoggingService {
    pub fn new(
        inner: Arc<dyn LlmService>,
        provider: &'static str,
        transport: &'static str,
    ) -> Self {
        let model_id = inner.model_id().to_string();
        Self {
            inner,
            model_id,
            provider,
            transport,
        }
    }
}

impl LoggingService {
    /// Span wrapping one provider call. `otel.name` overrides the exported
    /// span name so the Datadog resource is the model id (per-model latency
    /// and error aggregation); the tracing-side name stays `llm.request` for
    /// local log filtering. Usage/error fields are `Empty` until the call
    /// resolves.
    fn request_span(&self, request: &LlmRequest, streaming: bool) -> tracing::Span {
        let telemetry = request.telemetry.as_ref();
        let generated_request_id = format!("llm-{}", rand::random::<u64>());
        let request_id = telemetry.map_or(generated_request_id.as_str(), |value| {
            value.request_id.as_str()
        });
        tracing::info_span!(
            target: "phoenix_llm::otel",
            "llm.request",
            otel.kind = "client",
            otel.name = %self.model_id,
            otel.status_code = tracing::field::Empty,
            model = %self.model_id,
            provider = self.provider,
            transport = self.transport,
            streaming,
            conv_id = telemetry.map(|value| value.conversation_id.as_str()),
            root_conv_id = telemetry.map(|value| value.root_conversation_id.as_str()),
            request_id,
            retry_attempt = telemetry.map_or(1, |value| value.retry_attempt),
            input_tokens = tracing::field::Empty,
            output_tokens = tracing::field::Empty,
            cache_read_tokens = tracing::field::Empty,
            cache_creation_tokens = tracing::field::Empty,
            error.kind = tracing::field::Empty,
        )
    }

    fn record_outcome(span: &tracing::Span, result: &Result<LlmResponse, LlmError>) {
        match result {
            Ok(response) => {
                span.record("input_tokens", response.usage.input_tokens);
                span.record("output_tokens", response.usage.output_tokens);
                span.record("cache_read_tokens", response.usage.cache_read_tokens);
                span.record(
                    "cache_creation_tokens",
                    response.usage.cache_creation_tokens,
                );
            }
            Err(e) => {
                span.record("otel.status_code", "ERROR");
                span.record("error.kind", format!("{:?}", e.kind));
            }
        }
    }
}

#[async_trait]
impl LlmService for LoggingService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let span = self.request_span(request, false);
        let start = std::time::Instant::now();
        let result = self.inner.complete(request).instrument(span.clone()).await;
        let duration = start.elapsed();
        Self::record_outcome(&span, &result);

        match &result {
            Ok(response) => {
                tracing::info!(
                    parent: &span,
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM request completed"
                );
            }
            Err(e) => {
                tracing::error!(
                    parent: &span,
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    error_kind = ?e.kind,
                    auto_retryable = e.kind.is_auto_retryable(),
                    user_resumable = e.kind.is_user_resumable(),
                    "LLM request failed"
                );
            }
        }

        result
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &mpsc::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let span = self.request_span(request, true);
        let start = std::time::Instant::now();

        let result = self
            .inner
            .complete_streaming(request, chunk_tx)
            .instrument(span.clone())
            .await;
        let duration = start.elapsed();
        Self::record_outcome(&span, &result);

        match &result {
            Ok(response) => {
                tracing::info!(
                    parent: &span,
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM streaming request completed"
                );
            }
            Err(e) => {
                tracing::error!(
                    parent: &span,
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    error_kind = ?e.kind,
                    auto_retryable = e.kind.is_auto_retryable(),
                    user_resumable = e.kind.is_user_resumable(),
                    "LLM streaming request failed"
                );
            }
        }

        result
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn uses_codex_bridge(&self) -> bool {
        self.inner.uses_codex_bridge()
    }

    fn continuation_request_limits(&self) -> ContinuationRequestLimits {
        self.inner.continuation_request_limits()
    }
}
