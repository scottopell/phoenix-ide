//! LLM provider abstraction
//!
//! Provides a common interface for interacting with various LLM providers.

mod anthropic;
mod error;
mod models;
mod registry;
mod types;

pub use anthropic::AnthropicService;
pub use error::{LlmError, LlmErrorKind};
pub use models::{Provider, ModelDef, all_models};
pub use registry::{LlmConfig, ModelRegistry};
pub use types::*;

use async_trait::async_trait;
use std::sync::Arc;

/// Common interface for LLM providers
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Make a completion request
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Get the model ID
    fn model_id(&self) -> &str;

    /// Get the context window size in tokens
    #[allow(dead_code)] // For future context management
    fn context_window(&self) -> usize;

    /// Get max image dimension (for resizing before send)
    #[allow(dead_code)] // For future image resizing
    fn max_image_dimension(&self) -> Option<u32>;
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

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn context_window(&self) -> usize {
        self.inner.context_window()
    }

    fn max_image_dimension(&self) -> Option<u32> {
        self.inner.max_image_dimension()
    }
}
