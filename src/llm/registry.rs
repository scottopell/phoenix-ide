//! Model registry for managing available LLM providers

#![allow(dead_code)] // new_empty() used in tests

use super::{
    all_models, discover_models, probe_gateway, LlmService, LlmServiceImpl, LoggingService,
    Provider,
};
use std::collections::HashMap;
use std::sync::Arc;

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

/// How to authenticate Anthropic API requests.
///
/// Determines which header carries the credential:
/// - `ApiKey`: `x-api-key: <key>` (standard API keys and gateway implicit auth)
/// - `Bearer`: `Authorization: Bearer <token>` (Claude OAuth tokens from `claude login`)
#[derive(Debug, Clone)]
pub enum AnthropicAuth {
    /// Standard API key — sent as `x-api-key` header.
    /// Also used for gateway mode with the sentinel value `"implicit"`.
    ApiKey(String),
    /// OAuth bearer token — sent as `Authorization: Bearer` header.
    /// Sourced from `~/.claude/.credentials.json` written by `claude login`.
    Bearer(String),
}

impl AnthropicAuth {
    /// Extract the credential string regardless of variant.
    /// Used by providers that always take a plain string (e.g. `OpenAI`).
    pub fn as_str(&self) -> &str {
        match self {
            Self::ApiKey(k) | Self::Bearer(k) => k.as_str(),
        }
    }
}

// Private helpers for OAuth credential loading
#[derive(serde::Deserialize)]
struct OAuthCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OAuthToken,
}

#[derive(serde::Deserialize)]
struct OAuthToken {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: String,
}

/// Attempt to load an OAuth access token from `$HOME/.claude/.credentials.json`.
///
/// Returns `None` (silently) if the file doesn't exist, and logs a warning if
/// the file exists but is unreadable or the token has expired.
fn load_claude_oauth_token() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let creds_path = std::path::Path::new(&home)
        .join(".claude")
        .join(".credentials.json");

    let content = std::fs::read_to_string(&creds_path).ok()?; // silently absent

    let creds: OAuthCredentials = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                path = %creds_path.display(),
                error = %e,
                "Failed to parse Claude credentials file; ignoring OAuth auth"
            );
            return None;
        }
    };

    let token = creds.claude_ai_oauth;

    // Check expiry. expiresAt may be RFC3339 or a Unix timestamp in milliseconds.
    let expired = chrono::DateTime::parse_from_rfc3339(&token.expires_at)
        .map(|dt| dt < chrono::Utc::now())
        .or_else(|_| {
            token
                .expires_at
                .parse::<i64>()
                .map(|ms| chrono::Utc::now().timestamp_millis() > ms)
        })
        .unwrap_or_else(|_| {
            tracing::warn!(
                expires_at = %token.expires_at,
                "Could not parse Claude OAuth token expiry; assuming valid"
            );
            false
        });

    if expired {
        tracing::warn!(
            expires_at = %token.expires_at,
            "Claude OAuth token is expired; ignoring (run `claude login` to refresh)"
        );
        return None;
    }

    tracing::info!(expires_at = %token.expires_at, "Loaded Claude OAuth token from ~/.claude/.credentials.json");
    Some(token.access_token)
}

/// Configuration for LLM providers
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub fireworks_api_key: Option<String>,
    /// exe.dev gateway URL (e.g., `http://127.0.0.1:8462`)
    pub gateway: Option<String>,
    /// Default model ID
    pub default_model: Option<String>,
    /// OAuth access token loaded from `~/.claude/.credentials.json`, if present and unexpired.
    /// Takes precedence over `anthropic_api_key` for Anthropic models in direct mode.
    pub anthropic_oauth_token: Option<String>,
}

impl LlmConfig {
    pub fn from_env() -> Self {
        // OAuth token: prefer env var (set by deploy for system service users who
        // cannot read ~/.claude/.credentials.json at runtime), then fall back to
        // reading the file directly (works in dev where the process runs as the user).
        let anthropic_oauth_token = std::env::var("ANTHROPIC_OAUTH_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .or_else(load_claude_oauth_token);

        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            fireworks_api_key: std::env::var("FIREWORKS_API_KEY").ok(),
            gateway: std::env::var("LLM_GATEWAY").ok(),
            default_model: std::env::var("DEFAULT_MODEL").ok(),
            anthropic_oauth_token,
        }
    }
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

        // Determine default model
        let default_model = config
            .default_model
            .clone()
            .or_else(|| {
                if services.contains_key("claude-sonnet-4-6") {
                    Some("claude-sonnet-4-6".to_string())
                } else {
                    services.keys().next().cloned()
                }
            })
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string());

        // In non-discovery (direct key) mode, gateway is not configured
        let gateway_status = if config.gateway.is_some() {
            // new() is called without an async probe; treat as not configured
            // (new_with_discovery should be used when a gateway is present)
            GatewayStatus::NotConfigured
        } else {
            GatewayStatus::NotConfigured
        };

        Self {
            services,
            specs,
            default_model,
            gateway_status,
        }
    }

    /// Create registry with dynamic model discovery from gateway
    ///
    /// When gateway is configured, queries provider endpoints for available models.
    /// Falls back to hardcoded models if discovery fails.
    /// Always probes gateway reachability and records the result in `gateway_status`.
    pub async fn new_with_discovery(config: &LlmConfig) -> Self {
        // If no gateway, use direct-key mode (no probe needed)
        if config.gateway.is_none() {
            return Self::new(config);
        }

        let gateway_url = config.gateway.as_ref().unwrap();
        tracing::info!("Discovering models from gateway: {}", gateway_url);

        // Probe gateway reachability before attempting discovery
        let gateway_reachable = probe_gateway(gateway_url).await;
        let gateway_status = if gateway_reachable {
            GatewayStatus::Healthy
        } else {
            GatewayStatus::Unreachable
        };

        if !gateway_reachable {
            tracing::warn!(
                gateway = %gateway_url,
                "Gateway unreachable during startup probe; falling back to hardcoded models"
            );
            // Fall back to hardcoded models, but carry the Unreachable status so the
            // UI can show a warning banner.  Only use direct API keys if present.
            let mut fallback = Self::new(config);
            fallback.gateway_status = GatewayStatus::Unreachable;
            return fallback;
        }

        // Try to discover models from gateway
        let discovered = discover_models(gateway_url).await;

        if discovered.is_empty() {
            tracing::warn!(
                "Gateway model discovery returned no models, falling back to hardcoded list"
            );
            let mut fallback = Self::new(config);
            // Gateway was reachable (probe passed) but discovery returned nothing —
            // treat as Unreachable so the UI can warn the user.
            fallback.gateway_status = GatewayStatus::Unreachable;
            return fallback;
        }

        tracing::info!("Discovered {} models from gateway", discovered.len());

        // Build services from both hardcoded and discovered models
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        let mut specs: HashMap<String, super::ModelSpec> = HashMap::new();
        let mut registered_ids = std::collections::HashSet::new();

        // First, register hardcoded models that were discovered (preserves metadata).
        // Register both id and api_name in registered_ids so the second pass doesn't
        // double-register under the api_name string with a wrong (default) context window.
        for spec in all_models() {
            if discovered.contains_key(&spec.id) || discovered.contains_key(&spec.api_name) {
                if let Some(service) = Self::try_create_model(&spec, config) {
                    services.insert(spec.id.clone(), service);
                    specs.insert(spec.id.clone(), spec.clone());
                    registered_ids.insert(spec.id.clone());
                    registered_ids.insert(spec.api_name.clone());
                }
            }
        }

        // Guard: if no hardcoded model matched anything in the discovery list, the
        // gateway is returning an unrecognized format (e.g. provider-prefixed IDs,
        // Vertex AI models, etc.).  Fall back to the full hardcoded list rather than
        // poisoning the registry with hundreds of unusable dynamic entries.
        if registered_ids.is_empty() {
            tracing::warn!(
                discovered = discovered.len(),
                "No known models found in gateway discovery; falling back to hardcoded list"
            );
            let mut fallback = Self::new(config);
            // Gateway was reachable but returned an unrecognized model format.
            // Treat as Unreachable so the UI can warn the user.
            fallback.gateway_status = GatewayStatus::Unreachable;
            return fallback;
        }

        // Then, create services for any discovered models not in hardcoded list
        for (model_id, discovered_model) in &discovered {
            if !registered_ids.contains(model_id) {
                let spec = discovered_model.to_model_spec();
                if let Some(service) = Self::try_create_model(&spec, config) {
                    tracing::info!("Dynamically registered model: {}", model_id);
                    services.insert(spec.id.clone(), service);
                    specs.insert(spec.id.clone(), spec);
                }
            }
        }

        tracing::info!(
            "Registered {} models ({} hardcoded, {} dynamic)",
            services.len(),
            registered_ids.len(),
            services.len() - registered_ids.len()
        );

        // Determine default model
        let default_model = config
            .default_model
            .clone()
            .or_else(|| {
                if services.contains_key("claude-sonnet-4-6") {
                    Some("claude-sonnet-4-6".to_string())
                } else if services.contains_key("claude-sonnet-4-5") {
                    Some("claude-sonnet-4-5".to_string())
                } else {
                    services.keys().next().cloned()
                }
            })
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string());

        Self {
            services,
            specs,
            default_model,
            gateway_status,
        }
    }

    /// Try to create a model service, validating prerequisites
    fn try_create_model(
        spec: &super::ModelSpec,
        config: &LlmConfig,
    ) -> Option<Arc<dyn LlmService>> {
        let auth = if config.gateway.is_some() {
            // Gateway mode: sentinel value; gateway handles real authentication
            AnthropicAuth::ApiKey("implicit".to_string())
        } else {
            // Direct mode: require real credentials per provider
            match spec.provider {
                Provider::Anthropic => {
                    // OAuth takes precedence over API key
                    if let Some(token) = config.anthropic_oauth_token.as_ref() {
                        AnthropicAuth::Bearer(token.clone())
                    } else {
                        let key = config
                            .anthropic_api_key
                            .as_deref()
                            .filter(|k| !k.is_empty())?;
                        AnthropicAuth::ApiKey(key.to_string())
                    }
                }
                Provider::OpenAI => {
                    let key = config.openai_api_key.as_deref().filter(|k| !k.is_empty())?;
                    AnthropicAuth::ApiKey(key.to_string())
                }
                Provider::Fireworks => {
                    let key = config
                        .fireworks_api_key
                        .as_deref()
                        .filter(|k| !k.is_empty())?;
                    AnthropicAuth::ApiKey(key.to_string())
                }
            }
        };

        let service = Arc::new(LlmServiceImpl::new(
            spec.clone(),
            auth,
            config.gateway.clone(),
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

    /// Get a cheap/fast model for auxiliary tasks like title generation.
    /// Prefers: claude-haiku-4-5 > gpt-4o-mini > any available model
    pub fn get_cheap_model(&self) -> Option<Arc<dyn LlmService>> {
        // Priority order for cheap models
        const CHEAP_MODELS: &[&str] = &["claude-haiku-4-5", "gpt-4o-mini", "gpt-5-mini"];

        for model_id in CHEAP_MODELS {
            if let Some(service) = self.get(model_id) {
                return Some(service);
            }
        }

        // Fall back to default model if no cheap model available
        self.default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_api_keys_no_models() {
        let config = LlmConfig::default();
        let registry = ModelRegistry::new(&config);
        assert!(registry.available_models().is_empty());
    }

    #[test]
    fn test_anthropic_key_only_anthropic_models() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        let models = registry.available_models();
        assert!(!models.is_empty());

        // All models should be Anthropic models
        for model_id in &models {
            assert!(
                model_id.contains("claude"),
                "Expected claude model, got {model_id}"
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
        assert!(registry.get("gpt-4o").is_some());
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
            .find(|m| m.id == "claude-opus-4-6")
            .unwrap();
        assert_eq!(opus.provider, "Anthropic");
        assert!(opus.description.contains("most capable"));
        assert_eq!(opus.context_window, 200_000);
    }
}
