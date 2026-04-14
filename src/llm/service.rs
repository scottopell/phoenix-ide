//! Unified LLM service implementation

use super::models::{ApiFormat, ModelSpec};
use super::types::{LlmRequest, LlmResponse};
use super::{anthropic, openai, LlmAuth, LlmError, LlmService, TokenChunk};
use async_trait::async_trait;
use tokio::sync::broadcast;

/// Unified service implementation that dispatches by API format
pub struct LlmServiceImpl {
    pub spec: ModelSpec,
    /// LLM auth: credential source + header style.
    pub auth: LlmAuth,
    pub gateway: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub openai_base_url: Option<String>,
    pub custom_headers: Vec<(String, String)>,
}

impl LlmServiceImpl {
    pub fn new(
        spec: ModelSpec,
        auth: LlmAuth,
        gateway: Option<String>,
        anthropic_base_url: Option<String>,
        openai_base_url: Option<String>,
        custom_headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            spec,
            auth,
            gateway,
            anthropic_base_url,
            openai_base_url,
            custom_headers,
        }
    }
}

#[async_trait]
impl LlmService for LlmServiceImpl {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let result = self.complete_inner(request).await;

        // On auth failure: invalidate credential cache and retry once (only if
        // the credential source actually had something cached to invalidate —
        // static keys can't be refreshed, so retrying would be pointless).
        if let Err(ref e) = result {
            if e.kind == super::LlmErrorKind::Auth && self.auth.invalidate().await {
                tracing::warn!(
                    model = %self.spec.id,
                    "Auth failure; credential cache invalidated, retrying"
                );
                return self.complete_inner(request).await;
            }
        }

        result
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let result = self.complete_streaming_inner(request, chunk_tx).await;

        // On auth failure: invalidate cached credential so the next request uses
        // fresh ones, but don't retry. Retrying a stream risks sending duplicate
        // tokens through chunk_tx if any were emitted before the error.
        if let Err(ref e) = result {
            if e.kind == super::LlmErrorKind::Auth && self.auth.invalidate().await {
                tracing::warn!(
                    model = %self.spec.id,
                    "Auth failure (streaming); credential cache invalidated (next request will use fresh credentials)"
                );
            }
        }

        result
    }

    fn model_id(&self) -> &str {
        &self.spec.id
    }
}

impl LlmServiceImpl {
    /// Build the custom headers for a request, auto-injecting `provider` based on the model spec.
    fn headers_for_provider(&self) -> Vec<(String, String)> {
        let mut headers = self.custom_headers.clone();
        if !headers.is_empty()
            || self.anthropic_base_url.is_some()
            || self.openai_base_url.is_some()
        {
            // Auto-inject provider header if not already present
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("provider"))
            {
                headers.push((
                    "provider".to_string(),
                    self.spec.provider.header_value().to_string(),
                ));
            }
        }
        headers
    }

    async fn complete_inner(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let headers = self.headers_for_provider();
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                let resolved = self.resolve_auth().await?;
                anthropic::complete(
                    &self.spec,
                    &resolved,
                    self.gateway.as_deref(),
                    self.anthropic_base_url.as_deref(),
                    &headers,
                    request,
                )
                .await
            }
            ApiFormat::OpenAIResponses => {
                let key = self.resolve_openai_key().await?;
                openai::complete(
                    &self.spec,
                    &key,
                    self.gateway.as_deref(),
                    self.openai_base_url.as_deref(),
                    &headers,
                    request,
                )
                .await
            }
        }
    }

    async fn complete_streaming_inner(
        &self,
        request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let headers = self.headers_for_provider();
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                let resolved = self.resolve_auth().await?;
                anthropic::complete_streaming(
                    &self.spec,
                    &resolved,
                    self.gateway.as_deref(),
                    self.anthropic_base_url.as_deref(),
                    &headers,
                    request,
                    chunk_tx,
                )
                .await
            }
            ApiFormat::OpenAIResponses => {
                let key = self.resolve_openai_key().await?;
                openai::complete_streaming(
                    &self.spec,
                    &key,
                    self.gateway.as_deref(),
                    self.openai_base_url.as_deref(),
                    &headers,
                    request,
                    chunk_tx,
                )
                .await
            }
        }
    }

    /// Resolve auth credential for this request.
    async fn resolve_auth(&self) -> Result<super::ResolvedAuth, super::LlmError> {
        self.auth.resolve().await
    }

    /// Resolve a plain credential string for `OpenAI` calls.
    async fn resolve_openai_key(&self) -> Result<String, super::LlmError> {
        Ok(self.auth.resolve().await?.credential)
    }
}
