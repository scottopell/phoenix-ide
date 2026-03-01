//! LLM provider abstraction
//!
//! Provides a common interface for interacting with various LLM providers.

mod anthropic;
mod discovery;
mod error;
mod models;
mod openai;
#[cfg(test)]
mod proptests;
mod registry;
mod service;
mod types;

pub use discovery::discover_models;
pub use error::{LlmError, LlmErrorKind};
pub use models::{all_models, ApiFormat, ModelSpec, Provider};
pub use registry::{LlmConfig, ModelRegistry};
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
                    duration_ms = %duration.as_millis(),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM request completed"
                );
            }
            Err(e) => {
                tracing::error!(
                    model = %self.model_id,
                    duration_ms = %duration.as_millis(),
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
                    duration_ms = %duration.as_millis(),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    "LLM streaming request completed"
                );
            }
            Err(e) => {
                tracing::error!(
                    model = %self.model_id,
                    duration_ms = %duration.as_millis(),
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
