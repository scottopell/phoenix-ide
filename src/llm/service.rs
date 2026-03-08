//! Unified LLM service implementation

use super::models::{ApiFormat, ModelSpec};
use super::types::{LlmRequest, LlmResponse};
use super::{anthropic, openai, AnthropicAuth, LlmError, LlmService, TokenChunk};
use async_trait::async_trait;
use tokio::sync::broadcast;

/// Unified service implementation that dispatches by API format
pub struct LlmServiceImpl {
    pub spec: ModelSpec,
    /// Anthropic auth credentials (`ApiKey` or `Bearer` OAuth token).
    /// For OpenAI/Fireworks format, `auth.as_str()` extracts the plain key string.
    pub auth: AnthropicAuth,
    pub gateway: Option<String>,
}

impl LlmServiceImpl {
    pub fn new(spec: ModelSpec, auth: AnthropicAuth, gateway: Option<String>) -> Self {
        Self {
            spec,
            auth,
            gateway,
        }
    }
}

#[async_trait]
impl LlmService for LlmServiceImpl {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                anthropic::complete(&self.spec, &self.auth, self.gateway.as_deref(), request).await
            }
            ApiFormat::OpenAIChat => {
                openai::complete(
                    &self.spec,
                    self.auth.as_str(),
                    self.gateway.as_deref(),
                    request,
                )
                .await
            }
        }
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                anthropic::complete_streaming(
                    &self.spec,
                    &self.auth,
                    self.gateway.as_deref(),
                    request,
                    chunk_tx,
                )
                .await
            }
            ApiFormat::OpenAIChat => {
                openai::complete_streaming(
                    &self.spec,
                    self.auth.as_str(),
                    self.gateway.as_deref(),
                    request,
                    chunk_tx,
                )
                .await
            }
        }
    }

    fn model_id(&self) -> &str {
        &self.spec.id
    }
}
