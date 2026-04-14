//! LLM provider abstraction
//!
//! Provides a common interface for interacting with various LLM providers.
//!
//! # Gateway Contract
//!
//! Phoenix supports an optional LLM gateway (`LLM_GATEWAY` env var) that proxies
//! requests to upstream providers. The primary supported gateway is the exe.dev
//! built-in gateway at `http://169.254.169.254/gateway/llm`.
//!
//! ## URL construction
//!
//! Given `LLM_GATEWAY=http://host/gateway/llm`, Phoenix constructs request URLs as:
//!
//! | Provider    | URL                                                      |
//! |-------------|----------------------------------------------------------|
//! | `Anthropic` | `{gateway}/anthropic/v1/messages`                        |
//! | `OpenAI`    | `{gateway}/openai/v1/responses`                          |
//!
//! The first path segment after the gateway base is an **origin alias** that the
//! gateway uses to route to the correct upstream provider. Known aliases:
//! `anthropic`, `openai`.
//!
//! Note: the exe.dev gateway also supports an alternative path convention
//! `{gateway}/_/gateway/{provider}/...` (used by the Shelley Go agent). Phoenix
//! uses the shorter form. Both resolve to the same upstream.
//!
//! ## Authentication
//!
//! When a gateway is configured, Phoenix sends `x-api-key: implicit` (Anthropic)
//! or `Authorization: Bearer implicit` (`OpenAI`). The gateway handles
//! real API key injection.
//!
//! ## Discovery
//!
//! On startup, Phoenix probes `{gateway}/_proxy/status` for reachability, then
//! queries `{gateway}/{provider}/v1/models` for each provider to discover
//! available models. See [`discovery`] module.
//!
//! ## Streaming
//!
//! All streaming requests use `Transfer-Encoding: chunked` with
//! `Content-Type: text/event-stream`. The gateway proxies SSE events from the
//! upstream provider. Phoenix parses these with [`sse::SseParser`] which handles
//! chunk-boundary splits, bare `\r` line endings, and multi-line `data:` fields.
//!
//! ### Known issue: intermittent SSE corruption
//!
//! The exe.dev gateway has a known intermittent bug where SSE events are
//! corrupted during long streams (~500+ chunks). Symptoms: two SSE events
//! smashed together mid-JSON, or bytes dropped from event boundaries.
//! Likely cause: chunked transfer-encoding reassembly in the gateway proxy.
//! See task 594 for tracking. The `SseParser` includes diagnostic dump
//! capability (`diagnostic_dump()`) to capture raw bytes when parse failures
//! occur, aiding diagnosis.

mod anthropic;
pub mod credential_helper;
mod discovery;
mod error;
mod mock;
mod models;
mod openai;
#[cfg(test)]
mod proptests;
mod registry;
mod service;
pub(crate) mod sse;
mod types;

pub use credential_helper::{CredentialHelper, CredentialStatus};
pub use discovery::{discover_models, probe_gateway, DiscoveryConfig};
pub use error::{LlmError, LlmErrorKind};
pub use models::{all_models, ModelSpec, Provider};
#[allow(unused_imports)]
// CredentialSource + ResolvedAuth + AuthStyle: public API for downstream consumers
pub use registry::{
    AuthStyle, CredentialSource, GatewayStatus, LlmAuth, LlmConfig, ModelRegistry, ResolvedAuth,
};
pub use service::LlmServiceImpl;
pub use types::*;

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Chunks emitted during streaming. Only text deltas are forwarded to the UI;
/// tool input fragments are accumulated internally by the provider.
#[derive(Debug, Clone)]
pub enum TokenChunk {
    Text(String),
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
                    retryable = e.kind.is_retryable(),
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
                    retryable = e.kind.is_retryable(),
                    "LLM streaming request failed"
                );
            }
        }

        result
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}
