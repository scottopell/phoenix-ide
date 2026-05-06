//! Unified LLM service implementation

use super::models::{ApiFormat, ModelSpec};
use super::types::{LlmRequest, LlmResponse};
use super::{
    anthropic, openai, CodexCredential, LlmAuth, LlmError, LlmService, TokenChunk,
    CODEX_BACKEND_URL,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Empty placeholder used when no tags should be forwarded — keeps the
/// provider-call signatures uniform without allocating per request.
fn empty_tags() -> &'static BTreeMap<String, String> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    EMPTY.get_or_init(BTreeMap::new)
}

/// Unified service implementation that dispatches by API format
pub struct LlmServiceImpl {
    pub spec: ModelSpec,
    /// LLM auth: credential source + header style.
    pub auth: LlmAuth,
    pub gateway: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub openai_base_url: Option<String>,
    pub custom_headers: Vec<(String, String)>,
    /// Free-form metadata pairs injected as a top-level `tags` object on
    /// every outbound request. Phoenix doesn't interpret these — they're a
    /// pass-through channel for whatever proxy the request is routed
    /// through. Only attached when `gateway` is set; direct provider APIs
    /// reject unknown top-level fields.
    pub request_tags: BTreeMap<String, String>,
    /// When true, `OpenAI` Responses requests target the `ChatGPT` backend
    /// (`chatgpt.com/backend-api/codex`) and the request body is adjusted:
    /// `store: false` is set and a default `instructions` value is injected
    /// when the caller did not provide one.
    pub use_codex_backend: bool,
    /// Concrete `CodexCredential` reference used to source the
    /// `chatgpt-account-id` header per request — re-read each call so a
    /// `codex login` against a different account during the session reaches
    /// the wire instead of being pinned at registry build time.
    pub codex_credential: Option<Arc<CodexCredential>>,
}

impl LlmServiceImpl {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        spec: ModelSpec,
        auth: LlmAuth,
        gateway: Option<String>,
        anthropic_base_url: Option<String>,
        openai_base_url: Option<String>,
        custom_headers: Vec<(String, String)>,
        request_tags: BTreeMap<String, String>,
    ) -> Self {
        Self {
            spec,
            auth,
            gateway,
            anthropic_base_url,
            openai_base_url,
            custom_headers,
            request_tags,
            use_codex_backend: false,
            codex_credential: None,
        }
    }

    /// Build a service that routes `OpenAI` Responses calls through the `ChatGPT`
    /// backend (codex bridge). The base URL is forced to `CODEX_BACKEND_URL`
    /// regardless of any `OPENAI_BASE_URL` / `LLM_GATEWAY` setting; gateway and
    /// `Anthropic` URL fields are ignored on this path.
    pub fn new_with_codex_backend(
        spec: ModelSpec,
        auth: LlmAuth,
        custom_headers: Vec<(String, String)>,
        codex_credential: Arc<CodexCredential>,
    ) -> Self {
        Self {
            spec,
            auth,
            gateway: None,
            anthropic_base_url: None,
            openai_base_url: Some(CODEX_BACKEND_URL.to_string()),
            custom_headers,
            // No gateway in front of the codex bridge — tags would be sent
            // directly to chatgpt.com which rejects unknown body fields.
            request_tags: BTreeMap::new(),
            use_codex_backend: true,
            codex_credential: Some(codex_credential),
        }
    }

    /// Returns the tags map to attach on the wire for this request. Empty
    /// unless a gateway is configured — direct-to-provider calls go out
    /// untagged so unknown-field rejection can't break us.
    fn effective_request_tags(&self) -> &BTreeMap<String, String> {
        if self.gateway.is_some() {
            &self.request_tags
        } else {
            empty_tags()
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
    /// When the codex bridge is in use, the live `chatgpt-account-id` is read
    /// from the credential at every request so a mid-session account switch
    /// (re-running `codex login`) reaches the wire.
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
        if let Some(ref cred) = self.codex_credential {
            if let Some(account_id) = cred.account_id() {
                if !headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("chatgpt-account-id"))
                {
                    headers.push(("chatgpt-account-id".to_string(), account_id));
                }
            }
            // OpenAI-Beta is required by the ChatGPT-backend Responses
            // endpoint for the experimental Responses surface; Codex CLI
            // and Pi both send it. `originator` is OpenAI's telemetry-
            // attribution channel so traffic from Phoenix is identifiable
            // alongside Codex CLI and Pi traffic.
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("openai-beta"))
            {
                headers.push((
                    "OpenAI-Beta".to_string(),
                    "responses=experimental".to_string(),
                ));
            }
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("originator"))
            {
                headers.push(("originator".to_string(), "phoenix-ide".to_string()));
            }
        }
        headers
    }

    async fn complete_inner(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                let resolved = self.resolve_auth().await?;
                // Build headers AFTER resolve so any per-request state the
                // credential refresh updates (notably the codex account_id
                // pulled from auth.json) is reflected in this request's
                // headers, not the previous request's snapshot.
                let headers = self.headers_for_provider();
                anthropic::complete(
                    &self.spec,
                    &resolved,
                    self.gateway.as_deref(),
                    self.anthropic_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(),
                    request,
                )
                .await
            }
            ApiFormat::OpenAIResponses => {
                let key = self.auth.resolve().await?.credential;
                let headers = self.headers_for_provider();
                openai::complete(
                    &self.spec,
                    &key,
                    self.gateway.as_deref(),
                    self.openai_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(),
                    request,
                    self.use_codex_backend,
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
        match self.spec.api_format {
            ApiFormat::Anthropic => {
                let resolved = self.resolve_auth().await?;
                let headers = self.headers_for_provider();
                anthropic::complete_streaming(
                    &self.spec,
                    &resolved,
                    self.gateway.as_deref(),
                    self.anthropic_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(),
                    request,
                    chunk_tx,
                )
                .await
            }
            ApiFormat::OpenAIResponses => {
                let key = self.auth.resolve().await?.credential;
                let headers = self.headers_for_provider();
                openai::complete_streaming(
                    &self.spec,
                    &key,
                    self.gateway.as_deref(),
                    self.openai_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(),
                    request,
                    chunk_tx,
                    self.use_codex_backend,
                )
                .await
            }
        }
    }

    /// Resolve auth credential for this request.
    async fn resolve_auth(&self) -> Result<super::ResolvedAuth, super::LlmError> {
        self.auth.resolve().await
    }
}
