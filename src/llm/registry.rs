//! Model registry for managing available LLM providers

#![allow(dead_code)] // new_empty() used in tests

use super::{
    all_models, codex_credential, discover_models, probe_gateway, CodexCredential, DiscoveryConfig,
    LlmService, LlmServiceImpl, LoggingService, Provider,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Gateway reachability status determined at startup
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayStatus {
    /// No gateway configured; direct API key mode
    NotConfigured,
    /// Gateway configured and responded successfully during startup probe
    Healthy,
    /// Gateway configured but unreachable or returned an error during startup probe
    Unreachable,
}

/// A credential source that produces a string on demand.
/// Implementations range from static strings to cached command execution.
#[async_trait::async_trait]
pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    /// Fetch the current credential if available. Returns immediately (non-blocking).
    /// Returns `None` if the credential is not yet available (helper still running,
    /// no credential configured, etc.).
    async fn get(&self) -> Option<String>;
    /// Whether a recovery mechanism is actively running to obtain the credential.
    /// When `get()` returns `None` and this returns `true`, the caller should wait
    /// rather than treat it as a terminal failure.
    async fn is_recovering(&self) -> bool {
        false
    }
    /// Invalidate any cached value (e.g. after a 401).
    /// Returns `true` if there was a cached value to invalidate (i.e. a retry is worthwhile).
    async fn invalidate(&self) -> bool;
    /// Optional source-specific hint to surface on auth failures, used by
    /// `LlmAuth::resolve()` to enrich the generic "credential unavailable"
    /// message with actionable recovery guidance (e.g. "run `codex login`").
    /// Returns `None` to fall back to the generic message. Default-impl `None`
    /// keeps existing implementations unchanged.
    async fn last_error_hint(&self) -> Option<String> {
        None
    }
}

/// A static credential string that never changes.
pub struct StaticCredential(String);

impl StaticCredential {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Debug for StaticCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticCredential")
            .field("value", &"[redacted]")
            .finish()
    }
}

#[async_trait::async_trait]
impl CredentialSource for StaticCredential {
    async fn get(&self) -> Option<String> {
        Some(self.0.clone())
    }
    async fn invalidate(&self) -> bool {
        false // Static credentials can't be invalidated — retry won't help
    }
}

/// How an LLM credential should be sent in HTTP headers.
#[derive(Debug, Clone, Copy)]
pub enum AuthStyle {
    /// `x-api-key: <credential>` (standard API keys and gateway implicit auth)
    ApiKey,
    /// `Authorization: Bearer <credential>`.
    /// Used for service-to-service auth (e.g. Datadog AI Gateway with ddtool JWT).
    PlainBearer,
}

/// LLM authentication: a credential source paired with a header style.
pub struct LlmAuth {
    source: Arc<dyn CredentialSource>,
    style: AuthStyle,
}

impl LlmAuth {
    pub fn new(source: Arc<dyn CredentialSource>, style: AuthStyle) -> Self {
        Self { source, style }
    }

    /// Resolve the credential for use in request headers.
    pub async fn resolve(&self) -> Result<ResolvedAuth, super::LlmError> {
        if let Some(credential) = self.source.get().await {
            return Ok(ResolvedAuth {
                credential,
                style: self.style,
            });
        }
        let recovering = self.source.is_recovering().await;
        // Prefer the source's own hint (e.g. "run `codex login`") over the
        // generic message; fall back to the recovery / generic text.
        let message = if let Some(hint) = self.source.last_error_hint().await {
            hint
        } else if recovering {
            "Waiting for authentication — complete the sign-in flow to continue".to_string()
        } else {
            "Credential unavailable — check API key or LLM_API_KEY_HELPER".to_string()
        };
        let mut err = super::LlmError::auth(message);
        err.recovery_in_progress = recovering;
        Err(err)
    }

    /// Invalidate any cached credential (e.g. after a 401).
    pub async fn invalidate(&self) -> bool {
        self.source.invalidate().await
    }
}

impl std::fmt::Debug for LlmAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmAuth")
            .field("style", &self.style)
            .field("source", &"[redacted]")
            .finish()
    }
}

impl Clone for LlmAuth {
    fn clone(&self) -> Self {
        Self {
            source: Arc::clone(&self.source),
            style: self.style,
        }
    }
}

/// Credential resolved for use in HTTP headers.
pub struct ResolvedAuth {
    pub credential: String,
    pub style: AuthStyle,
}

/// Configuration for LLM providers
pub struct LlmConfig {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    /// exe.dev gateway URL (e.g., `http://127.0.0.1:8462`)
    pub gateway: Option<String>,
    /// Default model ID
    pub default_model: Option<String>,
    /// Interactive credential helper. Implements `CredentialSource` for LLM auth
    /// and streams interactive output (OIDC flows) to the UI panel.
    pub credential_helper: Option<Arc<crate::llm::CredentialHelper>>,
    /// Direct URL override for the Anthropic endpoint (overrides gateway routing).
    pub anthropic_base_url: Option<String>,
    /// Direct URL override for the `OpenAI` endpoint (overrides gateway routing).
    pub openai_base_url: Option<String>,
    /// Extra headers to inject on every LLM request (newline-separated "key: value").
    /// Parsed from `LLM_CUSTOM_HEADERS` env var. A `provider` header is auto-injected
    /// based on which provider is being called.
    pub custom_headers: Vec<(String, String)>,
    /// How credential helper output should be sent in HTTP headers.
    /// Parsed from `LLM_AUTH_HEADER` env var at startup.
    pub auth_style: AuthStyle,
    /// Experimental: gates the codex auth bridge. Parsed from
    /// `OPENAI_USE_CODEX_AUTH=1`. Tracked separately from `codex_credential`
    /// so a credential-load failure (file missing, wrong mode) is
    /// distinguishable from the feature being off — when this is `true`,
    /// `OpenAI` models must NOT silently fall through to the platform
    /// API-key path.
    pub use_codex_auth: bool,
    /// Experimental: when populated, `OpenAI` models are routed through the
    /// `ChatGPT` backend (`https://chatgpt.com/backend-api/codex`) using
    /// `OAuth` tokens borrowed from the local `Codex` CLI's `~/.codex/auth.json`.
    /// `Anthropic` and `Mock` providers are unaffected. The credential is
    /// loaded once at registry build; if loading fails, this is `None` and
    /// (when `use_codex_auth` is set) `OpenAI` models are not registered.
    pub codex_credential: Option<Arc<CodexCredential>>,
}

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field(
                "anthropic_api_key",
                &self.anthropic_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "openai_api_key",
                &self.openai_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field("gateway", &self.gateway)
            .field("default_model", &self.default_model)
            .field("credential_helper", &self.credential_helper.is_some())
            .field("anthropic_base_url", &self.anthropic_base_url)
            .field("openai_base_url", &self.openai_base_url)
            .field("custom_headers", &self.custom_headers)
            .field("auth_style", &self.auth_style)
            .field("use_codex_auth", &self.use_codex_auth)
            .field("codex_credential", &self.codex_credential.is_some())
            .finish()
    }
}

impl Clone for LlmConfig {
    fn clone(&self) -> Self {
        Self {
            anthropic_api_key: self.anthropic_api_key.clone(),
            openai_api_key: self.openai_api_key.clone(),
            gateway: self.gateway.clone(),
            default_model: self.default_model.clone(),
            credential_helper: self.credential_helper.as_ref().map(Arc::clone),
            anthropic_base_url: self.anthropic_base_url.clone(),
            openai_base_url: self.openai_base_url.clone(),
            custom_headers: self.custom_headers.clone(),
            auth_style: self.auth_style,
            use_codex_auth: self.use_codex_auth,
            codex_credential: self.codex_credential.as_ref().map(Arc::clone),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            anthropic_api_key: None,
            openai_api_key: None,
            gateway: None,
            default_model: None,
            credential_helper: None,
            anthropic_base_url: None,
            openai_base_url: None,
            custom_headers: Vec::new(),
            auth_style: AuthStyle::ApiKey,
            use_codex_auth: false,
            codex_credential: None,
        }
    }
}

impl LlmConfig {
    pub fn from_env() -> Self {
        let credential_helper = std::env::var("LLM_API_KEY_HELPER")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|command| {
                let ttl_ms = std::env::var("LLM_API_KEY_HELPER_TTL_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(2 * 60 * 60 * 1000); // default 2 hours
                crate::llm::CredentialHelper::new(command, Duration::from_millis(ttl_ms))
            });

        let anthropic_base_url = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty());

        let openai_base_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty());

        // Parse newline-separated "key: value" pairs (supports real newlines and literal \n)
        let custom_headers = std::env::var("LLM_CUSTOM_HEADERS")
            .ok()
            .map(|raw| {
                raw.replace("\\n", "\n")
                    .lines()
                    .filter_map(|line| {
                        let line = line.trim();
                        let (k, v) = line.split_once(':')?;
                        Some((k.trim().to_string(), v.trim().to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let use_codex_auth = std::env::var("OPENAI_USE_CODEX_AUTH")
            .ok()
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"));
        let codex_credential = if use_codex_auth {
            match CodexCredential::load(codex_credential::default_auth_path()) {
                Ok((cred, account_id)) => {
                    tracing::info!(
                        account_id = account_id.as_deref().unwrap_or("<none>"),
                        "OPENAI_USE_CODEX_AUTH enabled — routing OpenAI models via ChatGPT backend"
                    );
                    Some(cred)
                }
                Err(e) => {
                    tracing::warn!(error = %e,
                        "OPENAI_USE_CODEX_AUTH set but codex credential load failed; OpenAI models unavailable");
                    None
                }
            }
        } else {
            None
        };

        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            gateway: std::env::var("LLM_GATEWAY").ok(),
            default_model: std::env::var("DEFAULT_MODEL").ok(),
            credential_helper,
            anthropic_base_url,
            openai_base_url,
            custom_headers,
            auth_style: if std::env::var("LLM_AUTH_HEADER")
                .ok()
                .is_some_and(|v| v.eq_ignore_ascii_case("bearer"))
            {
                AuthStyle::PlainBearer
            } else {
                AuthStyle::ApiKey
            },
            use_codex_auth,
            codex_credential,
        }
    }
}

/// Derive a `/v1/models` URL from a base URL like `/v1/messages` or `/v1/responses`.
/// Replaces the last path segment with `"models"`, stripping any query string first.
fn derive_models_url(base_url: &str) -> Option<String> {
    // Strip query string if present (e.g. "https://host/v1/messages?foo=bar")
    let path = base_url.split('?').next().unwrap_or(base_url);
    let last_slash = path.rfind('/')?;
    // Safety: `last_slash` is from `rfind('/')` on `path`
    #[allow(clippy::string_slice)]
    Some(format!("{}models", &path[..=last_slash]))
}

/// Registry of available LLM models
pub struct ModelRegistry {
    services: HashMap<String, Arc<dyn LlmService>>,
    specs: HashMap<String, super::ModelSpec>,
    default_model: String,
    /// Reachability status of the configured gateway, determined at startup
    pub gateway_status: GatewayStatus,
}

impl ModelRegistry {
    /// Create an empty registry for testing purposes
    pub fn new_empty() -> Self {
        Self {
            services: HashMap::new(),
            specs: HashMap::new(),
            default_model: "test-model".to_string(),
            gateway_status: GatewayStatus::NotConfigured,
        }
    }

    pub fn new(config: &LlmConfig) -> Self {
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        let mut specs: HashMap<String, super::ModelSpec> = HashMap::new();

        // Try to create each model from the centralized definitions
        for spec in all_models() {
            if let Some(service) = Self::try_create_model(&spec, config) {
                services.insert(spec.id.clone(), service);
                specs.insert(spec.id.clone(), spec);
            }
        }

        let default_model = Self::pick_default_model(&services, config);

        Self {
            services,
            specs,
            default_model,
            gateway_status: GatewayStatus::NotConfigured,
        }
    }

    /// Create a registry with a specific gateway status, using hardcoded models only.
    fn new_with_status(config: &LlmConfig, status: GatewayStatus) -> Self {
        let mut reg = Self::new(config);
        reg.gateway_status = status;
        reg
    }

    /// Pick the default model from available services.
    /// Prefers claude-sonnet-4-6 > claude-sonnet-4-5 > any available > hardcoded fallback.
    fn pick_default_model(
        services: &HashMap<String, Arc<dyn LlmService>>,
        config: &LlmConfig,
    ) -> String {
        const PREFERRED: &[&str] = &["claude-sonnet-4-6", "claude-sonnet-4-5"];
        // Honor `DEFAULT_MODEL` only if it actually got registered. A
        // configured default that points at e.g. an OpenAI model when codex
        // auth failed would otherwise pin the registry's default to an
        // unavailable id, breaking every code path that calls `default()`.
        if let Some(ref configured) = config.default_model {
            if services.contains_key(configured) {
                return configured.clone();
            }
            tracing::warn!(
                requested = %configured,
                "DEFAULT_MODEL is configured but not available; falling back to a registered model"
            );
        }
        PREFERRED
            .iter()
            .find(|id| services.contains_key(**id))
            .map(|id| (*id).to_string())
            .or_else(|| services.keys().next().cloned())
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string())
    }

    /// Create registry with model discovery from gateway or `credential_helper`.
    ///
    /// Discovery validates which hardcoded models are available on the gateway.
    /// Unknown/dynamic models from the gateway are silently ignored.
    /// Falls back to hardcoded models if discovery fails.
    pub async fn new_with_discovery(config: &LlmConfig) -> Self {
        // Build discovery config from available settings
        let Some((discovery, is_gateway_mode)) = Self::build_discovery_config(config).await else {
            return Self::new(config);
        };

        // Gateway mode: probe reachability first
        if let (true, Some(ref gw)) = (is_gateway_mode, &config.gateway) {
            tracing::info!("Discovering models from gateway: {}", gw);

            let reachable = probe_gateway(
                gw,
                discovery.auth_token.as_deref(),
                &discovery.custom_headers,
            )
            .await;

            if !reachable {
                tracing::warn!(
                    gateway = %gw,
                    "Gateway unreachable during startup probe; falling back to hardcoded models"
                );
                return Self::new_with_status(config, GatewayStatus::Unreachable);
            }
        } else {
            tracing::info!("Discovering models via credential_helper auth");
        }

        // Try to discover models
        let discovered = discover_models(&discovery).await;

        // If discovery returned no models but we're in gateway mode and the probe succeeded,
        // the gateway is reachable but doesn't expose a model-listing endpoint (e.g. exe.dev
        // gateway only proxies inference). Fall back to hardcoded models with Healthy status.
        if discovered.is_empty() {
            if is_gateway_mode {
                tracing::warn!(
                    "Gateway model discovery returned no models (gateway may not support listing); \
                     using hardcoded model list with Healthy status"
                );
                return Self::new_with_status(config, GatewayStatus::Healthy);
            }
            tracing::warn!("Model discovery returned no models, falling back to hardcoded list");
            return Self::new_with_status(config, GatewayStatus::Unreachable);
        }

        tracing::info!("Discovered {} models from gateway", discovered.len());

        // Register hardcoded models that were confirmed by discovery.
        // The AI gateway returns provider-prefixed IDs (e.g. "anthropic/claude-sonnet-4-6"),
        // so also check for "{provider}/{id}" and "{provider}/{api_name}".
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        let mut specs: HashMap<String, super::ModelSpec> = HashMap::new();

        for spec in all_models() {
            let prefixed_id = format!("{}/{}", spec.provider.header_value(), spec.id);
            let prefixed_api = format!("{}/{}", spec.provider.header_value(), spec.api_name);
            if discovered.contains(&spec.id)
                || discovered.contains(&spec.api_name)
                || discovered.contains(&prefixed_id)
                || discovered.contains(&prefixed_api)
            {
                if let Some(service) = Self::try_create_model(&spec, config) {
                    services.insert(spec.id.clone(), service);
                    specs.insert(spec.id.clone(), spec.clone());
                }
            }
        }

        if services.is_empty() {
            tracing::warn!(
                discovered = discovered.len(),
                "No known models found in gateway discovery; falling back to hardcoded list"
            );
            return Self::new_with_status(config, GatewayStatus::Unreachable);
        }

        tracing::info!("Registered {} models (hardcoded only)", services.len());

        let default_model = Self::pick_default_model(&services, config);

        Self {
            services,
            specs,
            default_model,
            gateway_status: GatewayStatus::Healthy,
        }
    }

    /// Build a `DiscoveryConfig` from the available LLM config settings.
    ///
    /// Returns `Some((config, is_gateway_mode))` when discovery is possible,
    /// or `None` when no gateway or `credential_helper` is configured.
    async fn build_discovery_config(config: &LlmConfig) -> Option<(DiscoveryConfig, bool)> {
        if let Some(ref gw) = config.gateway {
            // Legacy gateway mode — construct URLs from gateway base
            let base = gw.trim_end_matches('/');
            Some((
                DiscoveryConfig {
                    anthropic_models_url: Some(format!("{base}/anthropic/v1/models")),
                    openai_models_url: Some(format!("{base}/openai/v1/models")),
                    auth_token: None, // Gateway handles auth
                    custom_headers: vec![],
                },
                true,
            ))
        } else if let Some(ref helper) = config.credential_helper {
            // Direct auth mode — derive models URLs from base URL overrides
            let auth_token = helper.get().await;
            // Helper not yet authenticated — skip discovery, fall back to hardcoded models
            auth_token.as_ref()?;
            let headers = config.custom_headers.clone();

            Some((
                DiscoveryConfig {
                    anthropic_models_url: config
                        .anthropic_base_url
                        .as_deref()
                        .and_then(derive_models_url),
                    openai_models_url: config
                        .openai_base_url
                        .as_deref()
                        .and_then(derive_models_url),
                    auth_token,
                    custom_headers: headers,
                },
                false,
            ))
        } else {
            None
        }
    }

    /// Try to create a model service, validating prerequisites
    fn try_create_model(
        spec: &super::ModelSpec,
        config: &LlmConfig,
    ) -> Option<Arc<dyn LlmService>> {
        // Mock provider needs no credentials
        if spec.provider == Provider::Mock {
            let service: Arc<dyn LlmService> = Arc::new(super::mock::MockLlmService);
            return Some(Arc::new(LoggingService::new(service)));
        }

        // Codex auth bridge: when the env flag is set, OpenAI models route
        // through the ChatGPT backend with borrowed OAuth tokens. If the
        // credential failed to load (file missing, wrong mode), OpenAI models
        // are skipped entirely — never silently fall through to platform API
        // key auth. Anthropic and Mock are unaffected.
        if config.use_codex_auth && spec.provider == Provider::OpenAI {
            let cred = config.codex_credential.as_ref()?;
            let auth = LlmAuth::new(
                Arc::clone(cred) as Arc<dyn CredentialSource>,
                AuthStyle::PlainBearer,
            );
            let service = Arc::new(LlmServiceImpl::new_with_codex_backend(
                spec.clone(),
                auth,
                config.custom_headers.clone(),
                Arc::clone(cred),
            ));
            return Some(Arc::new(LoggingService::new(service)));
        }

        let auth = if let Some(ref helper) = config.credential_helper {
            // credential_helper takes highest priority — dynamic credential for all providers
            LlmAuth::new(
                Arc::clone(helper) as Arc<dyn CredentialSource>,
                config.auth_style,
            )
        } else if config.gateway.is_some() {
            // Gateway mode: sentinel value; gateway handles real authentication
            LlmAuth::new(
                Arc::new(StaticCredential::new("implicit")),
                AuthStyle::ApiKey,
            )
        } else {
            // Direct mode: require real credentials per provider
            match spec.provider {
                Provider::Anthropic => {
                    let key = config
                        .anthropic_api_key
                        .as_deref()
                        .filter(|k| !k.is_empty())?;
                    LlmAuth::new(Arc::new(StaticCredential::new(key)), AuthStyle::ApiKey)
                }
                Provider::OpenAI => {
                    let key = config.openai_api_key.as_deref().filter(|k| !k.is_empty())?;
                    LlmAuth::new(Arc::new(StaticCredential::new(key)), AuthStyle::ApiKey)
                }
                Provider::Mock => unreachable!("handled above"),
            }
        };

        let service = Arc::new(LlmServiceImpl::new(
            spec.clone(),
            auth,
            config.gateway.clone(),
            config.anthropic_base_url.clone(),
            config.openai_base_url.clone(),
            config.custom_headers.clone(),
        ));
        Some(Arc::new(LoggingService::new(service)))
    }

    /// Get a model by ID
    pub fn get(&self, model_id: &str) -> Option<Arc<dyn LlmService>> {
        self.services.get(model_id).cloned()
    }

    /// Get the default model
    pub fn default(&self) -> Option<Arc<dyn LlmService>> {
        self.get(&self.default_model)
    }

    /// Get the default model ID
    pub fn default_model_id(&self) -> &str {
        &self.default_model
    }

    /// Get the context window size for a model (REQ-BED-022)
    pub fn context_window(&self, model_id: &str) -> usize {
        // Look up in stored specs (includes both hardcoded and dynamic)
        self.specs.get(model_id).map_or(
            crate::state_machine::state::DEFAULT_CONTEXT_WINDOW,
            |spec| spec.context_window,
        )
    }

    /// List all available model IDs
    pub fn available_models(&self) -> Vec<String> {
        let mut models: Vec<_> = self.services.keys().cloned().collect();
        models.sort();
        models
    }

    /// Get detailed information about available models
    pub fn available_model_info(&self) -> Vec<crate::api::ModelInfo> {
        let mut model_infos = Vec::new();

        // Get info for each registered model from stored specs
        for (model_id, spec) in &self.specs {
            if self.services.contains_key(model_id) {
                model_infos.push(crate::api::ModelInfo {
                    id: spec.id.clone(),
                    provider: spec.provider.display_name().to_string(),
                    description: spec.description.clone(),
                    context_window: spec.context_window,
                    recommended: spec.recommended,
                });
            }
        }

        model_infos
    }

    /// Check if any models are available
    pub fn has_models(&self) -> bool {
        !self.services.is_empty()
    }

    /// Build a registry with a single `claude-sonnet-4-6` slot wired to
    /// `service`. Test-only: bypasses `LlmConfig` and credential plumbing so
    /// integration-flavoured tests in non-llm modules (chain Q&A) can drive
    /// the public registry surface against a mock service.
    #[cfg(test)]
    pub fn for_test_with_sonnet(service: Arc<dyn LlmService>) -> Self {
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        services.insert("claude-sonnet-4-6".to_string(), service);
        Self {
            services,
            specs: HashMap::new(),
            default_model: "claude-sonnet-4-6".to_string(),
            gateway_status: GatewayStatus::NotConfigured,
        }
    }

    /// Get a mid-tier "Sonnet-class" model balanced for cost vs accuracy.
    ///
    /// Used by chain Q&A (REQ-CHN-006) where the same model identifier is
    /// pinned across all questions on the same chain so quality and latency
    /// don't drift. Returns the (`model_id`, service) pair so the caller
    /// can persist the identifier into `chain_qa.model`.
    ///
    /// Preference order: claude-sonnet-4-6 → claude-sonnet-4-6-1m →
    /// gpt-5.5 → registry default. Returns None only when the registry has
    /// no models at all.
    pub fn get_mid_tier_model(&self) -> Option<(String, Arc<dyn LlmService>)> {
        const PREFERRED: &[&str] = &["claude-sonnet-4-6", "claude-sonnet-4-6-1m", "gpt-5.5"];
        for id in PREFERRED {
            if let Some(service) = self.get(id) {
                return Some(((*id).to_string(), service));
            }
        }
        self.default().map(|s| (self.default_model.clone(), s))
    }

    /// Get a cheap/fast model for auxiliary tasks like title generation.
    /// Prefers: claude-haiku-4-5 > gpt-5.4-mini > any available model
    pub fn get_cheap_model(&self) -> Option<Arc<dyn LlmService>> {
        // Priority order for cheap models
        const CHEAP_MODELS: &[&str] = &["claude-haiku-4-5", "gpt-5.4-mini"];

        for model_id in CHEAP_MODELS {
            if let Some(service) = self.get(model_id) {
                return Some(service);
            }
        }

        // Fall back to default model if no cheap model available
        self.default()
    }

    /// Get the cheapest available model ID from the same provider family as `parent_model_id`.
    /// Falls back to `parent_model_id` if no cheap model is available for that provider.
    pub fn cheap_model_id_for_provider(&self, parent_model_id: &str) -> String {
        use crate::llm::models::Provider;

        let parent_provider = self.specs.get(parent_model_id).map(|s| s.provider);

        let candidates: &[&str] = match parent_provider {
            Some(Provider::Anthropic) => &["claude-haiku-4-5"],
            Some(Provider::OpenAI) => &["gpt-5.4-mini"],
            Some(Provider::Mock) => return "mock".to_string(),
            None => return parent_model_id.to_string(),
        };

        candidates
            .iter()
            .find(|id| self.services.contains_key(**id))
            .map_or_else(
                || parent_model_id.to_string(),
                std::string::ToString::to_string,
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_api_keys_only_mock() {
        let config = LlmConfig::default();
        let registry = ModelRegistry::new(&config);
        // Mock model is always available (no credentials needed)
        assert_eq!(registry.available_models(), vec!["mock".to_string()]);
    }

    #[test]
    fn test_anthropic_key_only_anthropic_and_mock_models() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        let models = registry.available_models();
        assert!(!models.is_empty());

        // All models should be Anthropic or mock
        for model_id in &models {
            assert!(
                model_id.contains("claude") || model_id == "mock",
                "Expected claude or mock model, got {model_id}"
            );
        }
    }

    #[test]
    fn test_gateway_enables_all_models() {
        // With gateway, all models become available (gateway handles auth)
        let config = LlmConfig {
            gateway: Some("https://example.com".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        // All models should be available since gateway mode uses "implicit" API key
        assert!(!registry.available_models().is_empty());
        // Should have models from multiple providers
        assert!(registry.get("claude-sonnet-4-6").is_some());
        assert!(registry.get("gpt-5.5").is_some());
    }

    #[test]
    fn test_gateway_with_anthropic_key() {
        let config = LlmConfig {
            gateway: Some("https://example.com".to_string()),
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        let models = registry.available_models();
        assert!(!models.is_empty());
        assert!(models.contains(&"claude-opus-4-5".to_string()));
    }

    #[test]
    fn test_default_model_selection() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        // Should default to claude-sonnet-4-6
        assert_eq!(registry.default_model_id(), "claude-sonnet-4-6");
    }

    #[test]
    fn test_custom_default_model() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            default_model: Some("claude-opus-4-5".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        assert_eq!(registry.default_model_id(), "claude-opus-4-5");
    }

    #[tokio::test]
    async fn test_static_credential() {
        let cred = StaticCredential::new("test-key");
        assert_eq!(cred.get().await, Some("test-key".to_string()));
    }

    #[test]
    fn test_credential_helper_enables_all_models() {
        // When credential_helper is set, all models become available
        let config = LlmConfig {
            credential_helper: Some(crate::llm::CredentialHelper::new(
                "echo test-token".to_string(),
                Duration::from_hours(1),
            )),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(!registry.available_models().is_empty());
        assert!(registry.get("claude-sonnet-4-6").is_some());
        assert!(registry.get("gpt-5.5").is_some());
    }

    /// Helper: build a CodexCredential pointing at a freshly-written valid
    /// auth.json file so try_create_model can complete the codex branch.
    fn fake_codex_credential(_dir: &tempfile::TempDir) -> Arc<crate::llm::CodexCredential> {
        let path = _dir.path().join("auth.json");
        std::fs::write(
            &path,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"x","refresh_token":"r","account_id":"acc-1"}}"#,
        )
        .unwrap();
        crate::llm::CodexCredential::load(path).unwrap().0
    }

    /// With OPENAI_USE_CODEX_AUTH on AND a valid credential, OpenAI models
    /// register via the codex branch (no need for OPENAI_API_KEY) and are
    /// distinct from Anthropic registration.
    #[test]
    fn test_codex_auth_registers_openai_models_without_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            use_codex_auth: true,
            codex_credential: Some(fake_codex_credential(&dir)),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(
            registry.get("gpt-5.5").is_some(),
            "OpenAI model should register via codex auth without OPENAI_API_KEY"
        );
        assert!(
            registry.get("claude-sonnet-4-6").is_some(),
            "Anthropic models unaffected by codex auth"
        );
    }

    /// With OPENAI_USE_CODEX_AUTH on but credential load failed, OpenAI
    /// models must NOT silently fall through to OPENAI_API_KEY auth — the
    /// whole point of the env flag's separate tracking.
    #[test]
    fn test_codex_auth_refuses_silent_fallback_when_cred_missing() {
        let config = LlmConfig {
            openai_api_key: Some("a-real-key".to_string()),
            use_codex_auth: true,
            codex_credential: None, // load failed
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(
            registry.get("gpt-5.5").is_none(),
            "OpenAI must not fall through to OPENAI_API_KEY when codex auth is on but cred missing"
        );
    }

    /// With the env flag OFF, codex_credential is irrelevant and the normal
    /// per-provider api-key auth applies. Guard against accidentally
    /// activating the codex branch from the credential alone.
    #[test]
    fn test_codex_branch_is_gated_by_env_flag_not_just_cred_presence() {
        let dir = tempfile::tempdir().unwrap();
        let config = LlmConfig {
            openai_api_key: Some("a-real-key".to_string()),
            use_codex_auth: false, // flag off
            codex_credential: Some(fake_codex_credential(&dir)),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(
            registry.get("gpt-5.5").is_some(),
            "OpenAI should register via OPENAI_API_KEY when use_codex_auth is off"
        );
    }

    /// pick_default_model must not pin to a configured DEFAULT_MODEL that
    /// isn't actually registered (e.g. DEFAULT_MODEL=gpt-5.5 with codex
    /// auth disabled and only an Anthropic key set).
    #[test]
    fn test_default_model_falls_back_when_configured_one_unavailable() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            default_model: Some("gpt-5.5".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        // gpt-5.5 isn't registered (no OpenAI auth), so default must fall
        // back to a model that actually exists.
        assert_ne!(registry.default_model_id(), "gpt-5.5");
        assert!(registry.get(registry.default_model_id()).is_some());
    }

    #[test]
    fn test_model_info_metadata() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        let model_infos = registry.available_model_info();
        assert!(!model_infos.is_empty());

        // Check that all models have proper metadata
        for info in &model_infos {
            assert!(!info.id.is_empty());
            assert!(!info.provider.is_empty());
            assert!(!info.description.is_empty());
            assert!(info.context_window > 0);
        }

        // Check specific model
        let opus = model_infos
            .iter()
            .find(|m| m.id == "claude-opus-4-7")
            .unwrap();
        assert_eq!(opus.provider, "Anthropic");
        assert!(opus.description.contains("most capable"));
        assert_eq!(opus.context_window, 200_000);
    }

    #[test]
    fn test_derive_models_url_from_messages() {
        assert_eq!(
            derive_models_url("https://ai-gateway.us1.ddbuild.io/v1/messages"),
            Some("https://ai-gateway.us1.ddbuild.io/v1/models".to_string())
        );
    }

    #[test]
    fn test_derive_models_url_from_responses() {
        assert_eq!(
            derive_models_url("https://ai-gateway.us1.ddbuild.io/v1/responses"),
            Some("https://ai-gateway.us1.ddbuild.io/v1/models".to_string())
        );
    }

    #[test]
    fn test_derive_models_url_from_anthropic_api() {
        assert_eq!(
            derive_models_url("https://api.anthropic.com/v1/messages"),
            Some("https://api.anthropic.com/v1/models".to_string())
        );
    }

    #[test]
    fn test_derive_models_url_no_slash() {
        // A URL with no slash at all returns None
        assert_eq!(derive_models_url("noslash"), None);
    }

    #[test]
    fn test_derive_models_url_strips_query_string() {
        assert_eq!(
            derive_models_url("https://host/v1/messages?foo=bar"),
            Some("https://host/v1/models".to_string())
        );
    }
}
