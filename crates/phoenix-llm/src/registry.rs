//! Model registry for managing available LLM providers

use super::{
    all_models, codex_credential, discover_models, merge_model_specs, parse_external_models,
    CodexCredential, DiscoveredModels, DiscoveryConfig, LlmService, LlmServiceImpl, LoggingService,
    ModelBackend, ModelInfo, ModelSource,
};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
    /// `x-api-key: <credential>` (standard API keys)
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
    ///
    /// # Errors
    /// Returns an auth [`super::LlmError`] when the credential source yields
    /// no credential (missing API key, unavailable helper, or an in-progress
    /// recovery the caller should wait on).
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
    /// Default model ID
    pub default_model: Option<String>,
    /// Interactive credential helper. Implements `CredentialSource` for LLM auth
    /// and streams interactive output (OIDC flows) to the UI panel.
    pub credential_helper: Option<Arc<crate::CredentialHelper>>,
    /// Direct URL override for the Anthropic endpoint.
    pub anthropic_base_url: Option<String>,
    /// Direct URL override for the `OpenAI` endpoint.
    pub openai_base_url: Option<String>,
    /// Extra headers to inject on every LLM request (newline-separated "key: value").
    /// Parsed from `LLM_CUSTOM_HEADERS` env var. A `provider` header is auto-injected
    /// based on which provider is being called.
    pub custom_headers: Vec<(String, String)>,
    /// Free-form metadata pairs forwarded as a top-level `tags` object on
    /// every outbound LLM request routed through a base URL override. Parsed from
    /// `LLM_REQUEST_TAGS` env var as comma-separated `key=value` pairs.
    /// Phoenix doesn't interpret these — they're a pass-through channel for
    /// whatever proxy sits in front of the model.
    pub request_tags: std::collections::BTreeMap<String, String>,
    /// How credential helper output should be sent in HTTP headers.
    /// Parsed from `LLM_AUTH_HEADER` env var at startup.
    pub auth_style: AuthStyle,
    /// Additional model specs loaded from `PHOENIX_LLM_MODELS` inline JSON.
    /// These are additive only; duplicate IDs are ignored when merged with
    /// built-ins.
    pub external_models: Vec<super::ModelSpec>,
    /// User has signalled intent to use the `ChatGPT` bridge. True when
    /// Phoenix's own login file (`~/.phoenix-ide/codex-auth.json`) exists at
    /// startup OR when `OPENAI_USE_CODEX_AUTH=1` is set (piggyback mode).
    /// When true, `OpenAI` models route through the bridge; when true but
    /// `codex_credential` is `None` (load failed), `OpenAI` models are
    /// unavailable rather than silently falling back to `OPENAI_API_KEY`.
    pub use_codex_auth: bool,
    /// When populated, `OpenAI` models are routed through the
    /// `ChatGPT` backend (`https://chatgpt.com/backend-api/codex`) using
    /// `OAuth` tokens borrowed from the local `Codex` CLI's `~/.codex/auth.json`.
    /// `Anthropic` and `Mock` providers are unaffected.
    pub codex_credential: Option<Arc<CodexCredential>>,
    /// Filesystem path the loaded `codex_credential` was constructed from
    /// (Phoenix's own auth file or Codex CLI's, depending on which won the
    /// startup-time priority dance — see `codex_credential::resolve_active_auth_path`).
    /// `None` whenever `codex_credential` is `None`. Surfaced so the login
    /// preflight can answer "do you need to restart after signing in?": if
    /// the loaded path equals the path the in-app login writes to, the mtime
    /// watch picks up new tokens; otherwise a restart is required.
    pub codex_credential_path: Option<std::path::PathBuf>,
    /// Filesystem environment this config resolved its paths from. Retained so
    /// a runtime credential reload ([`ModelRegistry::reload_codex_credential`])
    /// re-resolves the active auth path against the same authority instead of
    /// re-reading `$HOME`.
    pub runtime_env: Arc<PhoenixRuntimeEnvironment>,
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
            .field("default_model", &self.default_model)
            .field("credential_helper", &self.credential_helper.is_some())
            .field("anthropic_base_url", &self.anthropic_base_url)
            .field("openai_base_url", &self.openai_base_url)
            .field("custom_headers", &self.custom_headers)
            .field("request_tags", &self.request_tags)
            .field("auth_style", &self.auth_style)
            .field("external_models", &self.external_models.len())
            .field("use_codex_auth", &self.use_codex_auth)
            .field("codex_credential", &self.codex_credential.is_some())
            .field("codex_credential_path", &self.codex_credential_path)
            .field("runtime_env", &self.runtime_env)
            .finish()
    }
}

impl Clone for LlmConfig {
    fn clone(&self) -> Self {
        Self {
            anthropic_api_key: self.anthropic_api_key.clone(),
            openai_api_key: self.openai_api_key.clone(),
            default_model: self.default_model.clone(),
            credential_helper: self.credential_helper.as_ref().map(Arc::clone),
            anthropic_base_url: self.anthropic_base_url.clone(),
            openai_base_url: self.openai_base_url.clone(),
            custom_headers: self.custom_headers.clone(),
            request_tags: self.request_tags.clone(),
            auth_style: self.auth_style,
            external_models: self.external_models.clone(),
            use_codex_auth: self.use_codex_auth,
            codex_credential: self.codex_credential.as_ref().map(Arc::clone),
            codex_credential_path: self.codex_credential_path.clone(),
            runtime_env: self.runtime_env.clone(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            anthropic_api_key: None,
            openai_api_key: None,
            default_model: None,
            credential_helper: None,
            anthropic_base_url: None,
            openai_base_url: None,
            custom_headers: Vec::new(),
            request_tags: std::collections::BTreeMap::new(),
            auth_style: AuthStyle::ApiKey,
            external_models: Vec::new(),
            use_codex_auth: false,
            codex_credential: None,
            codex_credential_path: None,
            runtime_env: Arc::new(PhoenixRuntimeEnvironment::detect()),
        }
    }
}

impl LlmConfig {
    pub fn from_env(runtime_env: Arc<PhoenixRuntimeEnvironment>) -> Self {
        let credential_helper = std::env::var("LLM_API_KEY_HELPER")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|command| {
                let ttl_ms = std::env::var("LLM_API_KEY_HELPER_TTL_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(2 * 60 * 60 * 1000); // default 2 hours
                crate::CredentialHelper::new(command, Duration::from_millis(ttl_ms))
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

        let request_tags = std::env::var("LLM_REQUEST_TAGS")
            .ok()
            .as_deref()
            .map(parse_request_tags)
            .unwrap_or_default();

        let external_models = std::env::var("PHOENIX_LLM_MODELS")
            .ok()
            .filter(|raw| !raw.trim().is_empty())
            .map(|raw| match parse_external_models(&raw) {
                Ok(models) => {
                    tracing::info!(count = models.len(), "loaded PHOENIX_LLM_MODELS additions");
                    models
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "invalid PHOENIX_LLM_MODELS; ignoring externally configured LLM models"
                    );
                    Vec::new()
                }
            })
            .unwrap_or_default();

        // Resolve which file (if any) holds ChatGPT credentials at startup.
        // Phoenix's own ~/.phoenix-ide/codex-auth.json wins; OPENAI_USE_CODEX_AUTH=1
        // opts into reading Codex CLI's ~/.codex/auth.json instead. See
        // [`codex_credential::resolve_active_auth_path`].
        let active_auth_path = codex_credential::resolve_active_auth_path(&runtime_env);
        let (codex_credential, codex_credential_path) = match active_auth_path.as_ref() {
            Some(path) => match CodexCredential::load(path.clone()) {
                Ok((cred, account_id)) => {
                    tracing::info!(
                        path = %path.display(),
                        account_id = account_id.as_deref().unwrap_or("<none>"),
                        "ChatGPT bridge active — routing OpenAI models via ChatGPT backend"
                    );
                    (Some(cred), Some(path.clone()))
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(),
                        "ChatGPT auth file present but failed to load; OpenAI models unavailable");
                    (None, None)
                }
            },
            None => (None, None),
        };
        // `use_codex_auth` is the bridge-intent flag (see field docs). True
        // whenever the user has done something that signals "I want OpenAI
        // models routed through ChatGPT" — either logging in via Phoenix or
        // setting the piggyback env-var. The credential's actual presence is
        // checked separately at call sites.
        let use_codex_auth = active_auth_path.is_some()
            || std::env::var("OPENAI_USE_CODEX_AUTH")
                .ok()
                .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"));

        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            default_model: std::env::var("DEFAULT_MODEL").ok(),
            credential_helper,
            anthropic_base_url,
            openai_base_url,
            custom_headers,
            request_tags,
            auth_style: if std::env::var("LLM_AUTH_HEADER")
                .ok()
                .is_some_and(|v| v.eq_ignore_ascii_case("bearer"))
            {
                AuthStyle::PlainBearer
            } else {
                AuthStyle::ApiKey
            },
            external_models,
            use_codex_auth,
            codex_credential,
            codex_credential_path,
            runtime_env,
        }
    }
}

/// Parse the `LLM_REQUEST_TAGS` env-var format: comma-separated `key=value`
/// pairs. Whitespace around keys/values is trimmed. Empty pairs and pairs
/// without `=` are skipped. Empty keys are skipped (a value with no key has
/// nothing useful to forward).
fn parse_request_tags(raw: &str) -> std::collections::BTreeMap<String, String> {
    raw.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let (k, v) = pair.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.trim().to_string()))
        })
        .collect()
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

/// Registry of available LLM models.
///
/// Most state is frozen at construction. The Codex/ChatGPT bridge bits are
/// interior-mutable (RwLock-protected) so [`Self::reload_codex_credential`]
/// can swap the `OpenAI` bridge services in atomically after an in-app login —
/// no Phoenix restart needed (task 13005). Reads of the bridged services go
/// through the same `services` map readers already use; the lock gates a
/// per-OpenAI-model rebuild on the write side only.
pub struct ModelRegistry {
    services: std::sync::RwLock<HashMap<String, Arc<dyn LlmService>>>,
    specs: std::sync::RwLock<HashMap<String, super::ModelSpec>>,
    default_model: String,
    /// Whether the Codex/ChatGPT credential was loaded into the registry at
    /// process startup. **Frozen** at construction time and **not updated**
    /// when [`Self::reload_codex_credential`] runs — this is "was the bridge
    /// active when the process booted?", deliberately distinct from the
    /// current state. Diagnostic only; the preflight handler computes
    /// `restart_required_after_login` from the *current* loaded path
    /// (via [`Self::current_codex_loaded_path`]) instead.
    #[allow(dead_code)]
    pub codex_bridge_loaded_at_startup: bool,
    /// Path the **currently-loaded** credential was constructed from. `None`
    /// when no credential is active. Updated by `reload_codex_credential`
    /// in lockstep with the `OpenAI` bridge services.
    current_codex_loaded_path: std::sync::RwLock<Option<std::path::PathBuf>>,
    /// Config template kept for rebuilding bridge services on reload. The
    /// `codex_credential` / `codex_credential_path` fields are ignored on
    /// reload — we always re-resolve those from the filesystem.
    config: Arc<LlmConfig>,
}

impl ModelRegistry {
    /// Create an empty registry for testing purposes
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            services: std::sync::RwLock::new(HashMap::new()),
            specs: std::sync::RwLock::new(HashMap::new()),
            default_model: "test-model".to_string(),
            codex_bridge_loaded_at_startup: false,
            current_codex_loaded_path: std::sync::RwLock::new(None),
            config: Arc::new(LlmConfig::default()),
        }
    }

    #[must_use]
    pub fn new(config: &LlmConfig) -> Self {
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        let mut specs: HashMap<String, super::ModelSpec> = HashMap::new();

        // Try to create each model from the centralized definitions plus valid external additions.
        for spec in Self::model_specs(config) {
            if let Some(service) = Self::try_create_model(&spec, config) {
                services.insert(spec.id.clone(), service);
                specs.insert(spec.id.clone(), spec);
            }
        }

        let default_model = Self::pick_default_model(&services, config);

        Self {
            services: std::sync::RwLock::new(services),
            specs: std::sync::RwLock::new(specs),
            default_model,
            codex_bridge_loaded_at_startup: config.codex_credential.is_some(),
            current_codex_loaded_path: std::sync::RwLock::new(config.codex_credential_path.clone()),
            config: Arc::new(config.clone()),
        }
    }

    /// Pick the default model from available services.
    /// Prefers claude-sonnet-4-6 > claude-sonnet-4-5 > any available > hardcoded fallback.
    fn pick_default_model(
        services: &HashMap<String, Arc<dyn LlmService>>,
        config: &LlmConfig,
    ) -> String {
        const PREFERRED: &[&str] = &[
            "claude-sonnet-4-6",
            "claude-sonnet-4-5",
            "gpt-5.5",
            "gpt-5.3-codex",
            "gpt-5.4",
            "gpt-5.4-mini",
            "mock",
        ];
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

    /// Create registry with model discovery using credential-helper auth and base URL overrides.
    ///
    /// Discovery validates which configured models are available at direct model-listing
    /// endpoints derived from `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL`. Falls back
    /// to the configured model list if discovery is unavailable or unhelpful.
    pub async fn new_with_discovery(config: &LlmConfig) -> Self {
        let Some(discovery) = Self::build_discovery_config(config).await else {
            return Self::new(config);
        };

        tracing::info!("Discovering models via credential_helper auth");
        let discovered = discover_models(&discovery).await;

        if discovered.is_empty() {
            tracing::warn!(
                "Model discovery returned no models, falling back to configured model list"
            );
            return Self::new(config);
        }

        tracing::info!("Discovered {} models", discovered.len());

        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        let mut specs: HashMap<String, super::ModelSpec> = HashMap::new();

        for spec in Self::model_specs(config) {
            if Self::spec_matches_discovered_model(&spec, &discovered) {
                if let Some(service) = Self::try_create_model(&spec, config) {
                    services.insert(spec.id.clone(), service);
                    specs.insert(spec.id.clone(), spec);
                }
            }
        }

        if services.is_empty() {
            tracing::warn!(
                discovered = discovered.len(),
                "No configured known models found in discovery; falling back to configured model list"
            );
            return Self::new(config);
        }

        tracing::info!("Registered {} discovered configured models", services.len());

        let default_model = Self::pick_default_model(&services, config);

        Self {
            services: std::sync::RwLock::new(services),
            specs: std::sync::RwLock::new(specs),
            default_model,
            codex_bridge_loaded_at_startup: config.codex_credential.is_some(),
            current_codex_loaded_path: std::sync::RwLock::new(config.codex_credential_path.clone()),
            config: Arc::new(config.clone()),
        }
    }

    /// Return built-in model specs plus valid external additions from config.
    fn model_specs(config: &LlmConfig) -> Vec<super::ModelSpec> {
        merge_model_specs(all_models(), &config.external_models)
    }

    fn spec_matches_discovered_model(
        spec: &super::ModelSpec,
        discovered: &DiscoveredModels,
    ) -> bool {
        let ids = discovered.ids_for_backend(spec.backend);
        let prefixed_id = format!("{}/{}", spec.backend.header_value(), spec.id);
        let prefixed_api = format!("{}/{}", spec.backend.header_value(), spec.api_name);
        ids.contains(&spec.id)
            || ids.contains(&spec.api_name)
            || ids.contains(&prefixed_id)
            || ids.contains(&prefixed_api)
    }

    /// Build a `DiscoveryConfig` from credential-helper auth and base URL overrides.
    ///
    /// Returns `None` when no helper credential is ready or no model-listing URL can
    /// be derived from the configured base URLs.
    async fn build_discovery_config(config: &LlmConfig) -> Option<DiscoveryConfig> {
        let helper = config.credential_helper.as_ref()?;
        let auth_token = helper.get().await;
        auth_token.as_ref()?;

        let discovery = DiscoveryConfig {
            anthropic_models_url: config
                .anthropic_base_url
                .as_deref()
                .and_then(derive_models_url),
            openai_models_url: config
                .openai_base_url
                .as_deref()
                .and_then(derive_models_url),
            auth_token,
            custom_headers: config.custom_headers.clone(),
        };

        if discovery.anthropic_models_url.is_none() && discovery.openai_models_url.is_none() {
            None
        } else {
            Some(discovery)
        }
    }

    /// Try to create a model service, validating prerequisites
    fn try_create_model(
        spec: &super::ModelSpec,
        config: &LlmConfig,
    ) -> Option<Arc<dyn LlmService>> {
        // Mock provider: opt-in only via PHOENIX_ENABLE_MOCK_MODEL=1
        if spec.backend == ModelBackend::Mock {
            let enabled = std::env::var("PHOENIX_ENABLE_MOCK_MODEL")
                .map(|v| v == "1")
                .unwrap_or(false);
            if !enabled {
                return None;
            }
            let service: Arc<dyn LlmService> = Arc::new(super::mock::MockLlmService);
            return Some(Arc::new(LoggingService::new(service)));
        }

        // ChatGPT bridge: when the user has signalled intent to use the
        // bridge — either by logging in via Phoenix's `/codex/login` flow
        // (which writes ~/.phoenix-ide/codex-auth.json) or by setting
        // OPENAI_USE_CODEX_AUTH=1 to piggyback Codex CLI's file — OpenAI
        // models route through the ChatGPT backend. If intent was signalled
        // but the credential failed to load, OpenAI models are unavailable
        // rather than silently falling through to OPENAI_API_KEY (which
        // would bill the wrong account).
        if config.use_codex_auth
            && spec.backend == ModelBackend::OpenAIResponses
            && spec.source == ModelSource::BuiltIn
        {
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

        Self::try_create_model_with_standard_auth(spec, config)
    }

    fn try_create_model_with_standard_auth(
        spec: &super::ModelSpec,
        config: &LlmConfig,
    ) -> Option<Arc<dyn LlmService>> {
        let auth = if let Some(ref helper) = config.credential_helper {
            // credential_helper takes highest priority — dynamic credential for all providers
            LlmAuth::new(
                Arc::clone(helper) as Arc<dyn CredentialSource>,
                config.auth_style,
            )
        } else {
            // Direct mode: require real credentials per backend
            match spec.backend {
                ModelBackend::Anthropic => {
                    let key = config
                        .anthropic_api_key
                        .as_deref()
                        .filter(|k| !k.is_empty())?;
                    LlmAuth::new(Arc::new(StaticCredential::new(key)), AuthStyle::ApiKey)
                }
                ModelBackend::OpenAIResponses => {
                    let key = config.openai_api_key.as_deref().filter(|k| !k.is_empty())?;
                    LlmAuth::new(Arc::new(StaticCredential::new(key)), AuthStyle::ApiKey)
                }
                ModelBackend::Mock => unreachable!("handled above"),
            }
        };

        let service = Arc::new(LlmServiceImpl::new(
            spec.clone(),
            auth,
            config.anthropic_base_url.clone(),
            config.openai_base_url.clone(),
            config.custom_headers.clone(),
            config.request_tags.clone(),
        ));
        Some(Arc::new(LoggingService::new(service)))
    }

    /// Get a model by ID
    pub fn get(&self, model_id: &str) -> Option<Arc<dyn LlmService>> {
        self.services
            .read()
            .ok()
            .and_then(|map| map.get(model_id).cloned())
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
        let default = phoenix_core::domain::sm_state::DEFAULT_CONTEXT_WINDOW;
        let specs = self.specs.read().ok();
        let services = self.services.read().ok();
        match (specs.as_deref(), services.as_deref()) {
            (Some(specs), Some(services)) => specs
                .get(model_id)
                .zip(services.get(model_id))
                .map_or(default, |(spec, service)| {
                    spec.context_window_for(service.as_ref())
                }),
            _ => default,
        }
    }

    /// List all available model IDs
    ///
    /// # Panics
    /// Panics if the internal services lock is poisoned.
    pub fn available_models(&self) -> Vec<String> {
        let services = self.services.read().expect("services lock poisoned");
        let mut models: Vec<_> = services.keys().cloned().collect();
        models.sort();
        models
    }

    /// Get detailed information about available models
    ///
    /// # Panics
    /// Panics if the internal services or specs lock is poisoned.
    pub fn available_model_info(&self) -> Vec<ModelInfo> {
        let services = self.services.read().expect("services lock poisoned");
        let specs = self.specs.read().expect("specs lock poisoned");
        let mut model_infos = Vec::new();
        for (model_id, spec) in specs.iter() {
            if let Some(service) = services.get(model_id) {
                model_infos.push(ModelInfo {
                    id: spec.id.clone(),
                    provider: spec.backend.display_name().to_string(),
                    description: spec.description.clone(),
                    context_window: spec.context_window_for(service.as_ref()),
                    recommended: spec.recommended,
                });
            }
        }
        model_infos
    }

    /// Resolve a registered model id to a provider display name.
    ///
    /// Returns `"Unknown"` when the model is not currently registered.
    ///
    /// # Panics
    /// Panics if the internal specs lock is poisoned.
    pub fn provider_display_name(&self, model_id: &str) -> String {
        if let Some(spec) = self
            .specs
            .read()
            .expect("specs lock poisoned")
            .get(model_id)
            .cloned()
        {
            return spec.backend.display_name().to_string();
        }

        Self::model_specs(&self.config)
            .into_iter()
            .find(|spec| spec.id == model_id)
            .map_or_else(
                || "Unknown".to_string(),
                |spec| spec.backend.display_name().to_string(),
            )
    }

    /// Check if any models are available
    pub fn has_models(&self) -> bool {
        self.services
            .read()
            .map(|map| !map.is_empty())
            .unwrap_or(false)
    }

    /// Path the **currently-loaded** Codex/ChatGPT credential was constructed
    /// from, or `None` if no credential is active. Tracks reload state, not
    /// startup state — read by the login preflight to compute
    /// `restart_required_after_login` against where the next in-app login
    /// would write.
    pub fn current_codex_loaded_path(&self) -> Option<std::path::PathBuf> {
        self.current_codex_loaded_path
            .read()
            .ok()
            .and_then(|g| g.clone())
    }

    /// Build a registry with a single `claude-sonnet-4-6` slot wired to
    /// `service`. Test-only: bypasses `LlmConfig` and credential plumbing so
    /// integration-flavoured tests in non-llm modules (chain Q&A) can drive
    /// the public registry surface against a mock service.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test_with_sonnet(service: Arc<dyn LlmService>) -> Self {
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        services.insert("claude-sonnet-4-6".to_string(), service);
        Self {
            services: std::sync::RwLock::new(services),
            specs: std::sync::RwLock::new(HashMap::new()),
            default_model: "claude-sonnet-4-6".to_string(),
            codex_bridge_loaded_at_startup: false,
            current_codex_loaded_path: std::sync::RwLock::new(None),
            config: Arc::new(LlmConfig::default()),
        }
    }

    /// Get a mid-tier "Sonnet-class" model balanced for cost vs accuracy.
    ///
    /// Used by chain Q&A (REQ-CHN-006) where the same model identifier is
    /// pinned across all questions on the same chain so quality and latency
    /// don't drift. Returns the (`model_id`, service) pair so the caller
    /// can persist the identifier into `chain_qa.model`.
    ///
    /// Preference order: claude-sonnet-4-6 → gpt-5.5 → registry default.
    /// Returns None only when the registry has no models at all.
    pub fn get_mid_tier_model(&self) -> Option<(String, Arc<dyn LlmService>)> {
        const PREFERRED: &[&str] = &["claude-sonnet-4-6", "gpt-5.5"];
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
    ///
    /// # Panics
    /// Panics if the internal specs or services lock is poisoned.
    pub fn cheap_model_id_for_provider(&self, parent_model_id: &str) -> String {
        let parent_backend = {
            let specs = self.specs.read().expect("specs lock poisoned");
            specs.get(parent_model_id).map(|s| s.backend)
        };

        let candidates: &[&str] = match parent_backend {
            Some(ModelBackend::Anthropic) => &["claude-haiku-4-5"],
            Some(ModelBackend::OpenAIResponses) => &["gpt-5.4-mini"],
            Some(ModelBackend::Mock) => return "mock".to_string(),
            None => return parent_model_id.to_string(),
        };

        let services = self.services.read().expect("services lock poisoned");
        candidates
            .iter()
            .find(|id| services.contains_key(**id))
            .map_or_else(
                || parent_model_id.to_string(),
                std::string::ToString::to_string,
            )
    }

    /// Re-resolve the active Codex/ChatGPT credential and rebuild the `OpenAI`
    /// bridge services in place. Called from the login completion handlers
    /// (`settle_pkce` / `settle_device`) after a successful in-app login so
    /// the next `OpenAI` request picks up the new account without a Phoenix
    /// restart.
    ///
    /// On reload:
    ///  - If the active path produces a credential, every `OpenAI` model spec
    ///    gets a fresh `LlmServiceImpl::new_with_codex_backend` registered
    ///    under its id (replacing any prior bridge entry or non-bridge
    ///    direct-API-key entry).
    ///  - If the active path is `None` (logout-equivalent: file deleted, env
    ///    flag cleared), `OpenAI` bridge entries are removed. Direct `OpenAI`
    ///    via `OPENAI_API_KEY` is *not* re-registered here — callers that
    ///    need that should restart. Logging out is filed separately.
    ///
    /// Returns the path swap so the caller can log "active credential
    /// changed from X to Y" without re-reading the locks.
    ///
    /// Concurrency: holds write locks on `services` and
    /// `current_codex_loaded_path` for the duration. Concurrent `get()` /
    /// `available_models()` callers either see the prior state or the new
    /// state — never a torn map.
    pub fn reload_codex_credential(&self) -> CodexReloadOutcome {
        self.reload_codex_credential_with(codex_credential::resolve_active_auth_path(
            &self.config.runtime_env,
        ))
    }

    /// Same as [`Self::reload_codex_credential`] but accepts an explicit
    /// path resolution. Used by tests that need to drive reload deterministically
    /// without manipulating process-wide env vars.
    ///
    /// # Panics
    /// Panics if the internal services, specs, or loaded-path lock is poisoned.
    pub fn reload_codex_credential_with(
        &self,
        new_path: Option<std::path::PathBuf>,
    ) -> CodexReloadOutcome {
        let cred_with_account = match new_path.as_ref() {
            Some(path) => match CodexCredential::load(path.clone()) {
                Ok((cred, account_id)) => Some((cred, account_id)),
                Err(e) => {
                    // Load failed for a specified path. Do NOT swap services
                    // or update `current_codex_loaded_path`: pretending we
                    // loaded the file would suppress the UI's
                    // restart-required warning even though the bridge isn't
                    // actually live. Preserve the prior state — if the user
                    // had a working credential before, requests keep going
                    // through it; if not, OpenAI stays unavailable and the
                    // preflight honestly still asks for a restart.
                    tracing::warn!(error = %e, path = %path.display(),
                        "codex_login: reload failed to load credential — preserving prior bridge state");
                    let prior_path = self
                        .current_codex_loaded_path
                        .read()
                        .ok()
                        .and_then(|g| g.clone());
                    return CodexReloadOutcome {
                        previous_path: prior_path.clone(),
                        current_path: prior_path,
                        credential_loaded: false,
                    };
                }
            },
            None => None,
        };

        // Rebuild the OpenAI bridge services off-lock so the write window is
        // short. Build into a separate map; we'll merge under lock.
        let mut new_codex_services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        let mut new_codex_specs: HashMap<String, super::ModelSpec> = HashMap::new();
        if let Some((cred, _)) = cred_with_account.as_ref() {
            for spec in Self::model_specs(&self.config) {
                if spec.backend != ModelBackend::OpenAIResponses
                    || spec.source != ModelSource::BuiltIn
                {
                    continue;
                }
                let auth = LlmAuth::new(
                    Arc::clone(cred) as Arc<dyn CredentialSource>,
                    AuthStyle::PlainBearer,
                );
                let service = Arc::new(LlmServiceImpl::new_with_codex_backend(
                    spec.clone(),
                    auth,
                    self.config.custom_headers.clone(),
                    Arc::clone(cred),
                ));
                new_codex_services.insert(
                    spec.id.clone(),
                    Arc::new(LoggingService::new(service)) as Arc<dyn LlmService>,
                );
                new_codex_specs.insert(spec.id.clone(), spec);
            }
        }

        let previous_path = {
            let mut services = self.services.write().expect("services lock poisoned");
            let mut specs = self.specs.write().expect("specs lock poisoned");
            let mut current_path = self
                .current_codex_loaded_path
                .write()
                .expect("loaded-path lock poisoned");

            // Remove existing OpenAI entries before inserting the new ones,
            // so deregister-on-logout (cred=None) and switch-account both
            // converge on the right state.
            let openai_ids: Vec<String> = Self::model_specs(&self.config)
                .iter()
                .filter(|s| {
                    s.backend == ModelBackend::OpenAIResponses && s.source == ModelSource::BuiltIn
                })
                .map(|s| s.id.clone())
                .collect();
            for id in &openai_ids {
                services.remove(id);
                specs.remove(id);
            }
            for (id, svc) in new_codex_services {
                services.insert(id, svc);
            }
            for (id, spec) in new_codex_specs {
                specs.insert(id, spec);
            }

            let prev = current_path.clone();
            current_path.clone_from(&new_path);
            prev
        };

        let outcome = CodexReloadOutcome {
            previous_path,
            current_path: new_path,
            credential_loaded: cred_with_account.is_some(),
        };
        tracing::info!(
            previous_path = ?outcome.previous_path,
            current_path = ?outcome.current_path,
            credential_loaded = outcome.credential_loaded,
            "codex_login: reloaded ChatGPT bridge credential"
        );
        outcome
    }
}

/// Adapter that presents an `Arc<dyn LlmService>` as the narrow base-crate
/// [`phoenix_core::llm_service::CompletionService`]. A blanket
/// `impl CompletionService for T: LlmService` would violate the orphan rule
/// (both trait and type parameter are foreign to the base crate), and unsizing
/// coercion between two distinct trait objects (`Arc<dyn LlmService>` ->
/// `Arc<dyn CompletionService>`) isn't available — so the selector wraps the
/// service in this local newtype and flattens the rich `LlmError` to a display
/// string at the boundary.
struct AsCompletion(Arc<dyn LlmService>);

#[async_trait::async_trait]
impl phoenix_core::llm_service::CompletionService for AsCompletion {
    async fn complete(&self, request: &super::LlmRequest) -> Result<super::LlmResponse, String> {
        LlmService::complete(self.0.as_ref(), request)
            .await
            .map_err(|e| e.to_string())
    }
}

impl phoenix_core::llm_service::LlmSelector for ModelRegistry {
    fn get(&self, model_id: &str) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
        ModelRegistry::get(self, model_id).map(|svc| {
            Arc::new(AsCompletion(svc)) as Arc<dyn phoenix_core::llm_service::CompletionService>
        })
    }

    fn default_service(&self) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
        ModelRegistry::default(self).map(|svc| {
            Arc::new(AsCompletion(svc)) as Arc<dyn phoenix_core::llm_service::CompletionService>
        })
    }
}

/// Result of [`ModelRegistry::reload_codex_credential`]. Surfaced so the
/// caller can log a precise "swapped path X -> Y" line without re-acquiring
/// the registry's internal locks.
#[derive(Debug, Clone)]
pub struct CodexReloadOutcome {
    pub previous_path: Option<std::path::PathBuf>,
    pub current_path: Option<std::path::PathBuf>,
    pub credential_loaded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_no_api_keys_no_models() {
        let config = LlmConfig::default();
        let registry = ModelRegistry::new(&config);
        // Without PHOENIX_ENABLE_MOCK_MODEL=1, no models are available
        assert!(registry.available_models().is_empty());
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
    fn test_parse_request_tags_basic() {
        let tags = parse_request_tags("foo=bar,baz=qux");
        assert_eq!(tags.get("foo"), Some(&"bar".to_string()));
        assert_eq!(tags.get("baz"), Some(&"qux".to_string()));
        assert_eq!(tags.len(), 2);
    }

    #[test]
    fn test_parse_request_tags_whitespace_trimmed() {
        let tags = parse_request_tags("  foo = bar ,  baz=qux  ");
        assert_eq!(tags.get("foo"), Some(&"bar".to_string()));
        assert_eq!(tags.get("baz"), Some(&"qux".to_string()));
    }

    #[test]
    fn test_parse_request_tags_empty_input() {
        assert!(parse_request_tags("").is_empty());
        assert!(parse_request_tags("   ").is_empty());
        assert!(parse_request_tags(",,,").is_empty());
    }

    #[test]
    fn test_parse_request_tags_skips_malformed() {
        // missing '=' -> skipped; empty key -> skipped; empty value -> kept (intentional, "tag=" is a valid clear-flag idiom)
        let tags = parse_request_tags("nokey,=onlyval,foo=,bar=baz");
        assert_eq!(tags.get("foo"), Some(&String::new()));
        assert_eq!(tags.get("bar"), Some(&"baz".to_string()));
        assert_eq!(tags.len(), 2);
    }

    #[test]
    fn test_parse_request_tags_value_with_equals() {
        // split_once on first '=' lets values contain '='
        let tags = parse_request_tags("query=a=b=c");
        assert_eq!(tags.get("query"), Some(&"a=b=c".to_string()));
    }

    fn external_baseten_model() -> super::super::ModelSpec {
        parse_external_models(
            r#"[{"id":"baseten/moonshotai/Kimi-K2.6","backend":"anthropic","description":"Baseten Kimi K2.6 open-weight POC","context_window":262000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
    }

    #[test]
    fn configured_anthropic_model_registers_and_can_be_default() {
        let model = external_baseten_model();
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            default_model: Some(model.id.clone()),
            external_models: vec![model],
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        assert!(registry.get("baseten/moonshotai/Kimi-K2.6").is_some());
        assert_eq!(registry.default_model_id(), "baseten/moonshotai/Kimi-K2.6");
        assert_eq!(
            registry.context_window("baseten/moonshotai/Kimi-K2.6"),
            262_000
        );
    }

    #[test]
    fn configured_model_appears_in_model_info() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            external_models: vec![external_baseten_model()],
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        let info = registry
            .available_model_info()
            .into_iter()
            .find(|model| model.id == "baseten/moonshotai/Kimi-K2.6")
            .expect("external model should be included in /api/models data");

        assert_eq!(info.provider, "Anthropic");
        assert_eq!(info.context_window, 262_000);
        assert!(!info.recommended);
    }

    #[test]
    fn discovery_matcher_allows_configured_model_id_and_backend_prefix() {
        let model = external_baseten_model();
        let discovered = DiscoveredModels {
            anthropic: HashSet::from(["anthropic/baseten/moonshotai/Kimi-K2.6".to_string()]),
            openai_responses: HashSet::new(),
        };

        assert!(ModelRegistry::spec_matches_discovered_model(
            &model,
            &discovered
        ));
    }

    #[test]
    fn discovery_matcher_does_not_cross_backend_boundaries() {
        let model = external_baseten_model();
        let discovered = DiscoveredModels {
            anthropic: HashSet::new(),
            openai_responses: HashSet::from(["baseten/moonshotai/Kimi-K2.6".to_string()]),
        };

        assert!(!ModelRegistry::spec_matches_discovered_model(
            &model,
            &discovered
        ));
    }

    #[test]
    fn provider_display_name_uses_external_model_metadata_even_when_unregistered() {
        let registry = ModelRegistry::new(&LlmConfig {
            external_models: vec![external_baseten_model()],
            ..Default::default()
        });

        assert_eq!(
            registry.provider_display_name("baseten/moonshotai/Kimi-K2.6"),
            "Anthropic"
        );
        assert_eq!(registry.provider_display_name("unknown-model"), "Unknown");
    }

    #[test]
    fn duplicate_configured_model_does_not_override_builtin_registration() {
        let duplicate = parse_external_models(
            r#"[{"id":"claude-sonnet-4-6","api_name":"other-wire-name","backend":"anthropic","description":"Override attempt","context_window":123,"recommended":false,"supports_tool_search":false}]"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            external_models: vec![duplicate],
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        assert_eq!(registry.context_window("claude-sonnet-4-6"), 1_000_000);
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
            default_model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        assert_eq!(registry.default_model_id(), "claude-opus-4-6");
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
            credential_helper: Some(crate::CredentialHelper::new(
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

    /// Helper: build a `CodexCredential` pointing at a freshly-written valid
    /// auth.json file so `try_create_model` can complete the codex branch.
    fn fake_codex_credential(dir: &tempfile::TempDir) -> Arc<crate::CodexCredential> {
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"x","refresh_token":"r","account_id":"acc-1"}}"#,
        )
        .unwrap();
        crate::CodexCredential::load(path).unwrap().0
    }

    fn external_openai_model() -> super::super::ModelSpec {
        parse_external_models(
            r#"[{"id":"openai-compatible/custom","backend":"openai_responses","description":"OpenAI-compatible POC","context_window":128000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
    }

    #[test]
    fn external_openai_model_bypasses_codex_bridge_when_direct_configured() {
        let dir = tempfile::tempdir().unwrap();
        let config = LlmConfig {
            openai_api_key: Some("test-openai-key".to_string()),
            openai_base_url: Some("https://example.test/v1/responses".to_string()),
            use_codex_auth: true,
            codex_credential: Some(fake_codex_credential(&dir)),
            external_models: vec![external_openai_model()],
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        assert!(
            registry
                .get("gpt-5.5")
                .expect("built-in OpenAI model should still use Codex")
                .uses_codex_bridge(),
            "built-in OpenAI models keep existing Codex routing"
        );
        assert!(
            !registry
                .get("openai-compatible/custom")
                .expect("external OpenAI-compatible model should register")
                .uses_codex_bridge(),
            "external OpenAI-compatible models must use explicit endpoint/auth config"
        );
    }

    /// With Codex auth enabled and a valid credential, built-in `OpenAI` models register
    /// via the codex branch (no need for `OPENAI_API_KEY`) and are distinct from
    /// Anthropic registration.
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

    /// With Codex auth enabled but credential load failed, `OpenAI` models must
    /// not silently fall through to `OPENAI_API_KEY` auth.
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
            "OpenAI must not fall through to OPENAI_API_KEY when codex auth is enabled but credentials are absent"
        );
    }

    /// With Codex auth disabled (no bridge intent), `codex_credential` is
    /// ignored and standard `OpenAI` auth via `OPENAI_API_KEY` remains
    /// available. The bridge-intent flag — not mere credential presence —
    /// is what diverts traffic to the `ChatGPT` backend.
    #[test]
    fn test_codex_branch_is_gated_by_intent_flag_not_just_cred_presence() {
        let dir = tempfile::tempdir().unwrap();
        let config = LlmConfig {
            openai_api_key: Some("a-real-key".to_string()),
            use_codex_auth: false,
            codex_credential: Some(fake_codex_credential(&dir)),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(
            registry.get("gpt-5.5").is_some(),
            "OpenAI should register via OPENAI_API_KEY when bridge intent is off"
        );
    }

    /// Task 13005: hot reload after in-app login. A registry that booted
    /// with no Codex credential must register `OpenAI` bridge services after
    /// `reload_codex_credential_with` resolves to a valid auth file — no
    /// Phoenix restart required for the next `OpenAI` request to succeed.
    #[test]
    fn reload_registers_openai_after_first_login() {
        // Boot with no Codex creds.
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(
            registry.get("gpt-5.5").is_none(),
            "no OpenAI bridge before reload"
        );
        assert_eq!(registry.current_codex_loaded_path(), None);

        // Drop a fresh auth file, simulate the login handler's reload call.
        let dir = tempfile::tempdir().unwrap();
        let auth_path = dir.path().join("codex-auth.json");
        std::fs::write(
            &auth_path,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"x","refresh_token":"r","account_id":"acc-1"}}"#,
        )
        .unwrap();

        let outcome = registry.reload_codex_credential_with(Some(auth_path.clone()));
        assert_eq!(outcome.previous_path, None);
        assert_eq!(outcome.current_path.as_deref(), Some(auth_path.as_path()));
        assert!(outcome.credential_loaded);

        assert!(
            registry.get("gpt-5.5").is_some(),
            "reload must register the OpenAI bridge"
        );
        assert!(
            registry.get("claude-sonnet-4-6").is_some(),
            "Anthropic models unaffected by reload"
        );
        assert_eq!(
            registry.current_codex_loaded_path().as_deref(),
            Some(auth_path.as_path())
        );
    }

    /// Account-switch case: registry booted with one auth file, user signs
    /// in to a different account; reload must swap to the new path
    /// atomically, leaving prior bridge services replaced.
    #[test]
    fn reload_swaps_active_auth_path_for_account_switch() {
        let dir1 = tempfile::tempdir().unwrap();
        let path1 = dir1.path().join("piggyback.json");
        std::fs::write(
            &path1,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"a","refresh_token":"r","account_id":"acc-old"}}"#,
        )
        .unwrap();
        let cred1 = crate::CodexCredential::load(path1.clone()).unwrap().0;
        let config = LlmConfig {
            use_codex_auth: true,
            codex_credential: Some(cred1),
            codex_credential_path: Some(path1.clone()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert_eq!(
            registry.current_codex_loaded_path().as_deref(),
            Some(path1.as_path())
        );

        let dir2 = tempfile::tempdir().unwrap();
        let path2 = dir2.path().join("phoenix.json");
        std::fs::write(
            &path2,
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"b","refresh_token":"r","account_id":"acc-new"}}"#,
        )
        .unwrap();

        let outcome = registry.reload_codex_credential_with(Some(path2.clone()));
        assert_eq!(outcome.previous_path.as_deref(), Some(path1.as_path()));
        assert_eq!(outcome.current_path.as_deref(), Some(path2.as_path()));
        assert_eq!(
            registry.current_codex_loaded_path().as_deref(),
            Some(path2.as_path())
        );
        assert!(registry.get("gpt-5.5").is_some());
    }

    /// Reload to None (e.g. file deleted) deregisters the `OpenAI` bridge.
    /// Future logout flow will rely on this contract.
    #[test]
    fn reload_to_none_deregisters_openai_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let config = LlmConfig {
            use_codex_auth: true,
            codex_credential: Some(fake_codex_credential(&dir)),
            codex_credential_path: Some(dir.path().join("auth.json")),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(registry.get("gpt-5.5").is_some());

        let outcome = registry.reload_codex_credential_with(None);
        assert!(!outcome.credential_loaded);
        assert!(
            registry.get("gpt-5.5").is_none(),
            "OpenAI bridge must be removed when reload resolves to None"
        );
    }

    /// Load failure must NOT swap state. The prior bridge is preserved
    /// (so requests keep working) and `current_codex_loaded_path` keeps
    /// reflecting the actually-loaded path — so the preflight's
    /// `restart_required_after_login` predicate stays honest.
    #[test]
    fn reload_load_failure_preserves_prior_state() {
        let dir = tempfile::tempdir().unwrap();
        let initial_path = dir.path().join("auth.json");
        let config = LlmConfig {
            use_codex_auth: true,
            codex_credential: Some(fake_codex_credential(&dir)),
            codex_credential_path: Some(initial_path.clone()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert!(registry.get("gpt-5.5").is_some());
        assert_eq!(
            registry.current_codex_loaded_path().as_deref(),
            Some(initial_path.as_path())
        );

        // Point reload at a path with a malformed auth file. Load must fail.
        let bad_dir = tempfile::tempdir().unwrap();
        let bad_path = bad_dir.path().join("malformed.json");
        std::fs::write(&bad_path, b"{ not even json").unwrap();
        let outcome = registry.reload_codex_credential_with(Some(bad_path));
        assert!(!outcome.credential_loaded);

        // Prior bridge still works; current path still points at the
        // originally-loaded file. A preflight read here would correctly
        // report restart_required iff initial_path doesn't equal the
        // login-write path — independent of this failed attempt.
        assert!(
            registry.get("gpt-5.5").is_some(),
            "load failure must not deregister the working bridge"
        );
        assert_eq!(
            registry.current_codex_loaded_path().as_deref(),
            Some(initial_path.as_path()),
            "load failure must not advance current_codex_loaded_path"
        );
    }

    #[test]
    fn test_default_model_prefers_openai_over_mock() {
        let dir = tempfile::tempdir().unwrap();
        let config = LlmConfig {
            use_codex_auth: true,
            codex_credential: Some(fake_codex_credential(&dir)),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);
        assert_eq!(registry.default_model_id(), "gpt-5.5");
    }

    /// `pick_default_model` must not pin to a configured `DEFAULT_MODEL` that
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
            .find(|m| m.id == "claude-opus-4-8")
            .unwrap();
        assert_eq!(opus.provider, "Anthropic");
        assert!(opus.description.contains("most capable"));
        assert_eq!(opus.context_window, 1_000_000);
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
