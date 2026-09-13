//! Unified LLM service implementation

use super::codex_credential::AccountBoundCodexCredential;
use super::models::{ApiFormat, ModelSpec};
use super::types::{LlmRequest, LlmResponse};
use super::{anthropic, openai, LlmAuth, LlmError, LlmService, TokenChunk, CODEX_BACKEND_URL};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

const DEFAULT_LLM_ATTEMPT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Total lifetime allowed for one provider attempt. The service captures one
/// absolute instant at dispatch; transport retries and fallback cannot renew it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmAttemptDeadline(std::time::Duration);

impl LlmAttemptDeadline {
    #[must_use]
    pub const fn new(duration: std::time::Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> std::time::Duration {
        self.0
    }
}

impl Default for LlmAttemptDeadline {
    fn default() -> Self {
        Self(DEFAULT_LLM_ATTEMPT_DEADLINE)
    }
}

pub(crate) async fn enforce_attempt_deadline<T>(
    policy: LlmAttemptDeadline,
    request: &LlmRequest,
    operation: impl std::future::Future<Output = Result<T, LlmError>>,
) -> Result<T, LlmError> {
    let started_at = tokio::time::Instant::now();
    let deadline_at = started_at + policy.duration();
    if let Ok(result) = tokio::time::timeout_at(deadline_at, operation).await {
        result
    } else {
        if let Some(telemetry) = &request.telemetry {
            let _ = telemetry
                .attempt_capture
                .finalize_timed_out(started_at.elapsed());
        }
        Err(LlmError::timed_out(format!(
            "LLM provider attempt exceeded its {}s total deadline",
            policy.duration().as_secs()
        )))
    }
}

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
    pub anthropic_base_url: Option<String>,
    pub openai_responses_base_url: Option<String>,
    pub openai_chat_completions_base_url: Option<String>,
    pub custom_headers: Vec<(String, String)>,
    /// Free-form metadata pairs injected as a top-level `tags` object on
    /// every outbound request. Phoenix doesn't interpret these — they're a
    /// pass-through channel for whatever proxy the request is routed
    /// through. Attached only when the request is going to a non-default
    /// endpoint (`*_base_url`); direct provider APIs reject unknown top-level
    /// fields. See `effective_request_tags`.
    pub request_tags: BTreeMap<String, String>,
    /// When true, `OpenAI` Responses requests target the `ChatGPT` backend
    /// (`chatgpt.com/backend-api/codex`) and the request body is adjusted:
    /// `store: false` is set and a default `instructions` value is injected
    /// when the caller did not provide one.
    pub use_codex_backend: bool,
    /// Account-bound Codex credential used for both the bearer token and the
    /// `chatgpt-account-id` header. A file switch to another account fails
    /// closed until registry reload publishes that account's catalog.
    pub(crate) codex_credential: Option<Arc<AccountBoundCodexCredential>>,
    /// WebSocket continuation is shared by all calls through this service and
    /// isolated by the caller's prompt-cache cohort.
    attempt_deadline: LlmAttemptDeadline,
    pub(crate) codex_ws_sessions: Arc<Mutex<openai::CodexWsSessions>>,
}

impl LlmServiceImpl {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        spec: ModelSpec,
        auth: LlmAuth,
        anthropic_base_url: Option<String>,
        openai_responses_base_url: Option<String>,
        openai_chat_completions_base_url: Option<String>,
        custom_headers: Vec<(String, String)>,
        request_tags: BTreeMap<String, String>,
    ) -> Self {
        Self {
            spec,
            auth,
            anthropic_base_url,
            openai_responses_base_url,
            openai_chat_completions_base_url,
            custom_headers,
            request_tags,
            use_codex_backend: false,
            codex_credential: None,
            codex_ws_sessions: Arc::new(Mutex::new(openai::CodexWsSessions::default())),
            attempt_deadline: LlmAttemptDeadline::default(),
        }
    }

    /// Build a service that routes `OpenAI` Responses calls through the `ChatGPT`
    /// backend (codex bridge). The base URL is forced to `CODEX_BACKEND_URL`
    /// regardless of any `OPENAI_BASE_URL` setting; `Anthropic` URL fields are
    /// ignored on this path.
    #[must_use]
    pub(crate) fn new_with_codex_backend(
        spec: ModelSpec,
        auth: LlmAuth,
        custom_headers: Vec<(String, String)>,
        codex_credential: Arc<AccountBoundCodexCredential>,
    ) -> Self {
        Self {
            spec,
            auth,
            anthropic_base_url: None,
            openai_responses_base_url: Some(CODEX_BACKEND_URL.to_string()),
            openai_chat_completions_base_url: None,
            custom_headers,
            // No proxy in front of the codex bridge — tags would be sent
            // directly to chatgpt.com which rejects unknown body fields.
            request_tags: BTreeMap::new(),
            use_codex_backend: true,
            codex_credential: Some(codex_credential),
            codex_ws_sessions: Arc::new(Mutex::new(openai::CodexWsSessions::default())),
            attempt_deadline: LlmAttemptDeadline::default(),
        }
    }

    #[must_use]
    pub fn with_attempt_deadline(mut self, deadline: LlmAttemptDeadline) -> Self {
        self.attempt_deadline = deadline;
        self
    }

    /// Returns the tags map to attach on the wire for this request. Empty
    /// unless the request is routed through a non-default endpoint
    /// (the API-format-specific `*_BASE_URL` override). Direct-to-provider
    /// calls go out untagged so unknown-field rejection can't break us. The
    /// codex bridge sets
    /// `request_tags = BTreeMap::new()` in its constructor, so it stays
    /// untagged even though it does set `openai_responses_base_url`.
    fn effective_request_tags(&self, format_base_url: Option<&str>) -> &BTreeMap<String, String> {
        if format_base_url.is_some() {
            &self.request_tags
        } else {
            empty_tags()
        }
    }
}

#[async_trait]
impl LlmService for LlmServiceImpl {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.begin_provider_attempt(request, self.attempt_transport(false));
        Box::pin(enforce_attempt_deadline(
            self.attempt_deadline,
            request,
            async {
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
            },
        ))
        .await
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &mpsc::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        self.begin_provider_attempt(request, self.attempt_transport(true));
        Box::pin(enforce_attempt_deadline(self.attempt_deadline, request, async {
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
        }))
        .await
    }

    fn model_id(&self) -> &str {
        &self.spec.id
    }

    fn uses_codex_bridge(&self) -> bool {
        self.use_codex_backend
    }

    fn continuation_request_limits(&self) -> super::ContinuationRequestLimits {
        if self.use_codex_backend && openai::supports_responses_lite(&self.spec.api_name) {
            super::ContinuationRequestLimits::codex_responses_lite()
        } else if self.use_codex_backend {
            super::ContinuationRequestLimits::codex_bridge()
        } else {
            super::ContinuationRequestLimits::TokenWindowOnly
        }
    }
}

impl LlmServiceImpl {
    /// Build the custom headers for a request, auto-injecting `provider` based on the model spec.
    /// When the Codex bridge is in use, the account ID is pinned to the same
    /// registry generation as its discovered model catalog.
    fn headers_for_provider(&self) -> Vec<(String, String)> {
        let mut headers = self.custom_headers.clone();
        if !headers.is_empty()
            || self.anthropic_base_url.is_some()
            || self.openai_responses_base_url.is_some()
            || self.openai_chat_completions_base_url.is_some()
        {
            // Auto-inject provider header if not already present
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("provider"))
            {
                headers.push((
                    "provider".to_string(),
                    self.spec.provider_header_value().to_string(),
                ));
            }
        }
        if let Some(ref cred) = self.codex_credential {
            headers.retain(|(name, _)| !name.eq_ignore_ascii_case("chatgpt-account-id"));
            if let Some(account_id) = cred.account_id() {
                headers.push(("chatgpt-account-id".to_string(), account_id));
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

    fn attempt_transport(&self, streaming: bool) -> super::LlmTransport {
        if !streaming {
            return super::LlmTransport::HttpJson;
        }
        if self.spec.backend.api_format() == ApiFormat::OpenAIResponses
            && self.use_codex_backend
            && crate::openai::supports_responses_lite(&self.spec.api_name)
        {
            super::LlmTransport::Websocket
        } else {
            super::LlmTransport::HttpSse
        }
    }

    fn begin_provider_attempt(&self, request: &LlmRequest, transport: super::LlmTransport) {
        if let Some(telemetry) = &request.telemetry {
            telemetry.attempt_capture.begin(
                telemetry,
                self.spec.backend.header_value(),
                &self.spec.id,
                transport,
            );
        }
    }

    async fn complete_inner(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        match self.spec.backend.api_format() {
            ApiFormat::Anthropic => {
                let resolved = self.resolve_auth().await?;
                self.begin_provider_attempt(request, super::LlmTransport::HttpJson);
                // Build headers after auth resolution so refresh state is
                // reflected in this request's headers.
                let headers = self.headers_for_provider();
                anthropic::complete(
                    &self.spec,
                    &resolved,
                    self.anthropic_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(self.anthropic_base_url.as_deref()),
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
                    self.openai_responses_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(self.openai_responses_base_url.as_deref()),
                    request,
                    self.use_codex_backend,
                )
                .await
            }
            ApiFormat::OpenAIChatCompletions => {
                let key = self.auth.resolve().await?.credential;
                let headers = self.headers_for_provider();
                openai::complete_chat(
                    &self.spec,
                    &key,
                    self.openai_chat_completions_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(self.openai_chat_completions_base_url.as_deref()),
                    request,
                )
                .await
            }
        }
    }

    async fn complete_streaming_inner(
        &self,
        request: &LlmRequest,
        chunk_tx: &mpsc::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        match self.spec.backend.api_format() {
            ApiFormat::Anthropic => {
                let resolved = self.resolve_auth().await?;
                let headers = self.headers_for_provider();
                anthropic::complete_streaming(
                    &self.spec,
                    &resolved,
                    self.anthropic_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(self.anthropic_base_url.as_deref()),
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
                    self.openai_responses_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(self.openai_responses_base_url.as_deref()),
                    request,
                    chunk_tx,
                    self.use_codex_backend,
                    Some(&self.codex_ws_sessions),
                )
                .await
            }
            ApiFormat::OpenAIChatCompletions => {
                let key = self.auth.resolve().await?.credential;
                let headers = self.headers_for_provider();
                openai::complete_streaming_chat(
                    &self.spec,
                    &key,
                    self.openai_chat_completions_base_url.as_deref(),
                    &headers,
                    self.effective_request_tags(self.openai_chat_completions_base_url.as_deref()),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::all_models;
    use crate::registry::{AuthStyle, CredentialSource, StaticCredential};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    #[derive(Debug)]
    struct MissingCredential;

    #[async_trait::async_trait]
    impl crate::registry::CredentialSource for MissingCredential {
        async fn get(&self) -> Option<String> {
            None
        }

        async fn invalidate(&self) -> bool {
            false
        }
    }

    #[derive(Debug)]
    struct DelayedCredential;

    #[async_trait::async_trait]
    impl crate::registry::CredentialSource for DelayedCredential {
        async fn get(&self) -> Option<String> {
            std::future::pending().await
        }

        async fn invalidate(&self) -> bool {
            false
        }
    }

    fn request_with_capture() -> (LlmRequest, crate::LlmAttemptCapture) {
        let attempt_capture = crate::LlmAttemptCapture::new();
        let request = LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            effective_effort: phoenix_core::domain::llm_types::EffectiveEffort::native_unknown(),
            service_tier: phoenix_core::domain::llm_types::EffectiveServiceTier::Standard,
            telemetry: Some(crate::LlmRequestTelemetry {
                conversation_id: "conversation".to_string(),
                root_conversation_id: "root".to_string(),
                request_id: "request".to_string(),
                retry_attempt: 1,
                attempt_capture: attempt_capture.clone(),
            }),
            cache_key: crate::PromptCacheKey::stable("test"),
        };
        (request, attempt_capture)
    }

    fn make_service(
        anthropic_base_url: Option<&str>,
        openai_base_url: Option<&str>,
        tags: BTreeMap<String, String>,
    ) -> LlmServiceImpl {
        let spec = all_models()
            .into_iter()
            .find(|s| s.id == "claude-sonnet-5")
            .expect("claude-sonnet-5 must be in the model registry");
        let auth = LlmAuth::new(Arc::new(StaticCredential::new("k")), AuthStyle::ApiKey);
        LlmServiceImpl::new(
            spec,
            auth,
            anthropic_base_url.map(String::from),
            openai_base_url.map(String::from),
            None,
            vec![],
            tags,
        )
    }

    fn one_tag() -> BTreeMap<String, String> {
        let mut t = BTreeMap::new();
        t.insert("disable_data_logging".to_string(), "true".to_string());
        t
    }

    fn begin_test_attempt(capture: &crate::LlmAttemptCapture, request: &LlmRequest) {
        capture.begin(
            request.telemetry.as_ref().expect("test telemetry"),
            "openai",
            "gpt-test",
            crate::LlmTransport::Websocket,
        );
    }

    #[tokio::test(start_paused = true)]
    async fn active_non_visible_progress_cannot_extend_absolute_deadline() {
        let (request, capture) = request_with_capture();
        begin_test_attempt(&capture, &request);
        let progress_capture = capture.clone();
        let operation = async move {
            loop {
                // test-timing-allow: paused Tokio time is the deadline behavior under test
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                progress_capture.publish_progress(crate::ProviderStreamTelemetry {
                    dispatch_to_first_provider_event_ms: Some(1_000),
                    dispatch_to_first_generation_event_ms: Some(1_000),
                    dispatch_to_first_visible_text_ms: None,
                    provider_event_count: 1,
                    generation_event_count: 1,
                    visible_text_event_count: 0,
                    max_provider_gap_ms: Some(1_000),
                    max_generation_gap_ms: Some(1_000),
                    output_kind: crate::StreamTelemetryOutputKind::Reasoning,
                    completed: false,
                });
            }
            #[allow(unreachable_code)]
            Ok::<(), LlmError>(())
        };
        let task = tokio::spawn({
            let request = request.clone();
            async move {
                enforce_attempt_deadline(
                    LlmAttemptDeadline::new(std::time::Duration::from_secs(10)),
                    &request,
                    operation,
                )
                .await
            }
        });

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(9)).await;
        assert!(!task.is_finished());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        let error = task.await.unwrap().expect_err("deadline must win");
        assert_eq!(error.kind, crate::LlmErrorKind::TimedOut);
        let metrics = capture.finalized().expect("timeout metric");
        assert_eq!(metrics.outcome, crate::LlmAttemptOutcome::TimedOut);
        assert_eq!(metrics.total_duration_ms, 10_000);
        assert_eq!(metrics.stream.visible_text_event_count, 0);
        assert!(!metrics.stream.completed);
    }

    #[tokio::test(start_paused = true)]
    async fn fallback_phase_receives_only_remaining_absolute_budget() {
        let (request, capture) = request_with_capture();
        begin_test_attempt(&capture, &request);
        let reached_fallback = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fallback_flag = reached_fallback.clone();
        let operation = async move {
            // test-timing-allow: paused Tokio time models the pre-fallback phase
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            fallback_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            // test-timing-allow: paused Tokio time proves fallback cannot renew the deadline
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            Ok::<(), LlmError>(())
        };
        let task = tokio::spawn({
            let request = request.clone();
            async move {
                enforce_attempt_deadline(
                    LlmAttemptDeadline::new(std::time::Duration::from_secs(10)),
                    &request,
                    operation,
                )
                .await
            }
        });

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(8)).await;
        tokio::task::yield_now().await;
        assert!(reached_fallback.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!task.is_finished());
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            task.await.unwrap().expect_err("shared deadline").kind,
            crate::LlmErrorKind::TimedOut
        );
        assert_eq!(
            capture
                .finalized()
                .expect("timeout metric")
                .total_duration_ms,
            10_000
        );
    }

    #[tokio::test(start_paused = true)]
    async fn success_before_deadline_remains_the_single_terminal_metric() {
        let (request, capture) = request_with_capture();
        begin_test_attempt(&capture, &request);
        let result = enforce_attempt_deadline(
            LlmAttemptDeadline::new(std::time::Duration::from_secs(10)),
            &request,
            async {
                // test-timing-allow: paused Tokio time places success before the deadline
                tokio::time::sleep(std::time::Duration::from_secs(9)).await;
                Ok::<_, LlmError>(())
            },
        )
        .await;
        assert!(result.is_ok());
        let success = capture
            .finalize(crate::LlmAttemptFinalization {
                stream: Some(crate::ProviderStreamTelemetry::non_streaming()),
                outcome: crate::LlmAttemptOutcome::Success,
            })
            .expect("started attempt");
        assert_eq!(success.outcome, crate::LlmAttemptOutcome::Success);
        assert_eq!(capture.finalize_cancelled(), Some(success));
    }

    #[test]
    fn unsupported_codex_model_attempt_transport_is_http_sse() {
        let mut spec = all_models()
            .into_iter()
            .find(|model| model.id == "gpt-5.5")
            .expect("gpt-5.5 must be in the model registry");
        spec.backend = crate::ModelBackend::OpenAIResponses;
        spec.api_name = "gpt-5.5".to_string();
        let mut service = LlmServiceImpl::new(
            spec,
            LlmAuth::new(Arc::new(MissingCredential), AuthStyle::PlainBearer),
            None,
            None,
            None,
            vec![],
            BTreeMap::new(),
        );
        service.use_codex_backend = true;

        assert_eq!(
            service.attempt_transport(true),
            crate::LlmTransport::HttpSse
        );
    }

    #[tokio::test]
    async fn unsupported_codex_auth_failure_records_http_sse_transport() {
        let mut spec = all_models()
            .into_iter()
            .find(|model| model.id == "gpt-5.5")
            .expect("gpt-5.5 must be in the model registry");
        spec.backend = crate::ModelBackend::OpenAIResponses;
        spec.api_name = "gpt-5.5".to_string();
        let mut service = LlmServiceImpl::new(
            spec,
            LlmAuth::new(Arc::new(MissingCredential), AuthStyle::PlainBearer),
            None,
            None,
            None,
            vec![],
            BTreeMap::new(),
        );
        service.use_codex_backend = true;
        let service: Arc<dyn LlmService> = Arc::new(service);
        let service = crate::LoggingService::new(service, "openai", crate::LlmTransport::HttpSse);
        let (request, capture) = request_with_capture();
        let (chunk_tx, _chunk_rx) = mpsc::channel(1);

        let error = service
            .complete_streaming(&request, &chunk_tx)
            .await
            .expect_err("missing credential should fail before adapter dispatch");

        assert_eq!(error.kind, crate::LlmErrorKind::Auth);
        assert_eq!(
            capture.finalized().expect("attempt finalized").transport,
            crate::LlmTransport::HttpSse
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_codex_deadline_before_adapter_records_http_sse_transport() {
        let mut spec = all_models()
            .into_iter()
            .find(|model| model.id == "gpt-5.5")
            .expect("gpt-5.5 must be in the model registry");
        spec.backend = crate::ModelBackend::OpenAIResponses;
        spec.api_name = "gpt-5.5".to_string();
        let mut service = LlmServiceImpl::new(
            spec,
            LlmAuth::new(Arc::new(DelayedCredential), AuthStyle::PlainBearer),
            None,
            None,
            None,
            vec![],
            BTreeMap::new(),
        )
        .with_attempt_deadline(LlmAttemptDeadline::new(std::time::Duration::from_millis(
            100,
        )));
        service.use_codex_backend = true;
        let service: Arc<dyn LlmService> = Arc::new(service);
        let service = crate::LoggingService::new(service, "openai", crate::LlmTransport::HttpSse);
        let (request, capture) = request_with_capture();
        let (chunk_tx, _chunk_rx) = mpsc::channel(1);

        let error = service
            .complete_streaming(&request, &chunk_tx)
            .await
            .expect_err("deadline wins before credential or adapter work");

        assert_eq!(error.kind, crate::LlmErrorKind::TimedOut);
        assert_eq!(
            capture.finalized().expect("attempt finalized").transport,
            crate::LlmTransport::HttpSse
        );
    }

    #[tokio::test]
    async fn local_auth_failure_finalizes_the_service_dispatched_attempt() {
        let spec = all_models()
            .into_iter()
            .find(|model| model.id == "claude-sonnet-5")
            .expect("claude-sonnet-5 must be in the model registry");
        let service: Arc<dyn LlmService> = Arc::new(LlmServiceImpl::new(
            spec,
            LlmAuth::new(Arc::new(MissingCredential), AuthStyle::ApiKey),
            None,
            None,
            None,
            vec![],
            BTreeMap::new(),
        ));
        let service =
            crate::LoggingService::new(service, "anthropic", crate::LlmTransport::HttpSse);
        let (request, capture) = request_with_capture();
        let (chunk_tx, _chunk_rx) = mpsc::channel(1);

        let error = service
            .complete_streaming(&request, &chunk_tx)
            .await
            .expect_err("missing local credential should fail");

        assert_eq!(error.kind, crate::LlmErrorKind::Auth);
        assert_eq!(
            capture
                .finalized()
                .expect("service attempt is terminal")
                .outcome,
            crate::LlmAttemptOutcome::AuthError
        );
    }

    #[test]
    fn codex_bound_account_header_replaces_custom_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":4102444800}"#);
        let jwt = format!("{header}.{payload}.");
        std::fs::write(
            &path,
            format!(
                r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{jwt}","refresh_token":"r","account_id":"catalog-account"}}}}"#
            ),
        )
        .unwrap();
        let (credential, account_id) = crate::CodexCredential::load(path).unwrap();
        let bound = Arc::new(AccountBoundCodexCredential::new(credential, account_id));
        let mut spec = all_models()
            .into_iter()
            .find(|spec| spec.id == "gpt-6-astra")
            .unwrap();
        spec.api_name = "gpt-6-astra".to_string();
        let auth = LlmAuth::new(
            Arc::clone(&bound) as Arc<dyn CredentialSource>,
            AuthStyle::PlainBearer,
        );
        let mut service = LlmServiceImpl::new_with_codex_backend(spec, auth, Vec::new(), bound);
        service.custom_headers = vec![(
            "ChatGPT-Account-ID".to_string(),
            "custom-account".to_string(),
        )];

        let headers = service.headers_for_provider();
        assert!(!headers.iter().any(|(_, value)| value == "custom-account"));
        assert!(headers.iter().any(|(name, value)| name
            .eq_ignore_ascii_case("chatgpt-account-id")
            && value == "catalog-account"));
    }

    #[test]
    fn astra_codex_continuation_reserves_responses_lite_prefix() {
        let mut spec = all_models()
            .into_iter()
            .find(|spec| spec.id == "gpt-6-astra")
            .expect("Astra spec");
        spec.api_name = "gpt-6-astra".to_string();
        let auth = LlmAuth::new(Arc::new(StaticCredential::new("k")), AuthStyle::PlainBearer);
        let service = LlmServiceImpl {
            spec,
            auth,
            anthropic_base_url: None,
            openai_responses_base_url: Some(crate::CODEX_BACKEND_URL.to_string()),
            openai_chat_completions_base_url: None,
            custom_headers: Vec::new(),
            request_tags: BTreeMap::new(),
            use_codex_backend: true,
            codex_credential: None,
            codex_ws_sessions: Arc::new(Mutex::new(openai::CodexWsSessions::default())),
        };

        assert_eq!(
            service.continuation_request_limits(),
            crate::ContinuationRequestLimits::codex_responses_lite()
        );
    }

    fn chat_gateway_service_with_api_name(api_name: &str) -> LlmServiceImpl {
        let mut spec = all_models()
            .into_iter()
            .find(|s| s.id == "gpt-5.5")
            .expect("gpt-5.5 must be in the model registry");
        spec.backend = crate::ModelBackend::OpenAIChatCompletions;
        spec.api_name = api_name.to_string();
        let auth = LlmAuth::new(Arc::new(StaticCredential::new("k")), AuthStyle::PlainBearer);
        LlmServiceImpl::new(
            spec,
            auth,
            None,
            None,
            Some("https://gateway.example/v1/chat/completions".to_string()),
            vec![
                ("tenant-id".to_string(), "example".to_string()),
                ("source".to_string(), "test-source".to_string()),
            ],
            BTreeMap::new(),
        )
    }

    #[test]
    fn provider_header_uses_api_name_prefix_for_gateway_models() {
        let svc = chat_gateway_service_with_api_name("gateway-provider/example-org/Code-Model");
        let headers = svc.headers_for_provider();
        assert_eq!(
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("provider"))
                .map(|(_, value)| value.as_str()),
            Some("gateway-provider")
        );
    }

    #[test]
    fn explicit_provider_header_still_wins() {
        let mut svc = chat_gateway_service_with_api_name("gateway-provider/example-org/Code-Model");
        svc.custom_headers
            .push(("provider".to_string(), "custom".to_string()));
        let headers = svc.headers_for_provider();
        assert_eq!(
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("provider"))
                .map(|(_, value)| value.as_str()),
            Some("custom")
        );
    }

    #[test]
    fn openai_format_base_urls_are_isolated() {
        let mut responses_spec = all_models()
            .into_iter()
            .find(|s| s.id == "gpt-5.5")
            .expect("gpt-5.5 must be in the model registry");
        responses_spec.backend = crate::ModelBackend::OpenAIResponses;
        let mut chat_spec = responses_spec.clone();
        chat_spec.backend = crate::ModelBackend::OpenAIChatCompletions;
        let auth = LlmAuth::new(Arc::new(StaticCredential::new("k")), AuthStyle::PlainBearer);

        let responses = LlmServiceImpl::new(
            responses_spec,
            auth.clone(),
            None,
            Some("https://gateway.example/v1/responses".to_string()),
            Some("https://gateway.example/v1/chat/completions".to_string()),
            vec![],
            one_tag(),
        );
        let chat = LlmServiceImpl::new(
            chat_spec,
            auth,
            None,
            Some("https://gateway.example/v1/responses".to_string()),
            Some("https://gateway.example/v1/chat/completions".to_string()),
            vec![],
            one_tag(),
        );

        assert_eq!(
            responses.openai_responses_base_url.as_deref(),
            Some("https://gateway.example/v1/responses")
        );
        assert_eq!(
            chat.openai_chat_completions_base_url.as_deref(),
            Some("https://gateway.example/v1/chat/completions")
        );
        assert_eq!(
            responses
                .effective_request_tags(responses.openai_responses_base_url.as_deref())
                .len(),
            1
        );
        assert_eq!(
            chat.effective_request_tags(chat.openai_chat_completions_base_url.as_deref())
                .len(),
            1
        );
    }

    #[test]
    fn tags_attached_for_anthropic_base_url_only_path() {
        // Helper + ANTHROPIC_BASE_URL means a proxy is in front; tags must reach it.
        let svc = make_service(
            Some("https://proxy.example/anthropic/v1/messages"),
            None,
            one_tag(),
        );
        assert_eq!(
            svc.effective_request_tags(svc.anthropic_base_url.as_deref())
                .len(),
            1
        );
    }

    #[test]
    fn tags_dropped_for_direct_provider_call() {
        // No base_url override -> direct to api.anthropic.com,
        // which 400s on unknown body fields. Drop the tags.
        let svc = make_service(None, None, one_tag());
        assert!(svc.effective_request_tags(None).is_empty());
    }

    #[test]
    fn tags_isolated_per_api_format() {
        // Anthropic via proxy, OpenAI direct: an OpenAI call must not
        // pick up tags just because anthropic_base_url is set.
        let svc = make_service(
            Some("https://proxy.example/anthropic/v1/messages"),
            None,
            one_tag(),
        );
        assert!(
            svc.effective_request_tags(svc.openai_responses_base_url.as_deref())
                .is_empty(),
            "OpenAI call must not inherit Anthropic's base-URL gate"
        );
    }
}
