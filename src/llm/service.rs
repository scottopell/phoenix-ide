//! Unified LLM service implementation

use super::models::{ApiFormat, ModelSpec};
use super::types::{LlmRequest, LlmResponse};
use super::{anthropic, openai, LlmError, LlmService};
use async_trait::async_trait;

/// Unified service implementation that dispatches by API format
pub struct LlmServiceImpl {
    pub spec: ModelSpec,
    pub api_key: String,
    pub gateway: Option<String>,
}

impl LlmServiceImpl {
    pub fn new(spec: ModelSpec, api_key: String, gateway: Option<String>) -> Self {
        Self {
            spec,
            api_key,
            gateway,
        }
    }
}

#[async_trait]
impl LlmService for LlmServiceImpl {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                anthropic::complete(
                    &self.spec,
                    &self.api_key,
                    self.gateway.as_deref(),
                    request,
                )
                .await
            }
            ApiFormat::OpenAIChat => {
                openai::complete(
                    &self.spec,
                    &self.api_key,
                    self.gateway.as_deref(),
                    request,
                )
                .await
            }
        }
    }

    fn model_id(&self) -> &str {
        &self.spec.id
    }
}
