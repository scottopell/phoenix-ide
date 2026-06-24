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
//! Direct Anthropic calls use `x-api-key` unless `LLM_AUTH_HEADER=bearer` is set
//! for helper-issued tokens. `OpenAI` Responses calls use bearer auth. The
//! ChatGPT/Codex bridge only applies to built-in `OpenAI` Responses models.
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
pub use discovery::{discover_models, DiscoveryConfig};
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
use tokio::sync::broadcast;

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
        chunk_tx: &broadcast::Sender<TokenChunk>,
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
}

/// Logging wrapper for LLM services
pub struct LoggingService {
    inner: Arc<dyn LlmService>,
    model_id: String,
}

impl LoggingService {
    pub fn new(inner: Arc<dyn LlmService>) -> Self {
        let model_id = inner.model_id().to_string();
        Self { inner, model_id }
    }
}

#[async_trait]
impl LlmService for LoggingService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let start = std::time::Instant::now();
        let result = self.inner.complete(request).await;
        let duration = start.elapsed();

        match &result {
            Ok(response) => {
                tracing::info!(
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM request completed"
                );
            }
            Err(e) => {
                tracing::error!(
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    error = %e.message,
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
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let start = std::time::Instant::now();
        let result = self.inner.complete_streaming(request, chunk_tx).await;
        let duration = start.elapsed();

        match &result {
            Ok(response) => {
                tracing::info!(
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM streaming request completed"
                );
            }
            Err(e) => {
                tracing::error!(
                    model = %self.model_id,
                    duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    error = %e.message,
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
}
