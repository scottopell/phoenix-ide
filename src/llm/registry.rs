//! Model registry for managing available LLM providers

#![allow(dead_code)] // new_empty() used in tests

use super::{all_models, LlmService, LoggingService, Provider};
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for LLM providers
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub fireworks_api_key: Option<String>,
    /// exe.dev gateway URL (e.g., `http://169.254.169.254/gateway/llm`)
    pub gateway: Option<String>,
    /// Default model ID
    pub default_model: Option<String>,
}

impl LlmConfig {
    pub fn from_env() -> Self {
        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            fireworks_api_key: std::env::var("FIREWORKS_API_KEY").ok(),
            gateway: std::env::var("LLM_GATEWAY").ok(),
            default_model: std::env::var("DEFAULT_MODEL").ok(),
        }
    }
}

/// Registry of available LLM models
pub struct ModelRegistry {
    services: HashMap<String, Arc<dyn LlmService>>,
    default_model: String,
}

impl ModelRegistry {
    /// Create an empty registry for testing purposes
    pub fn new_empty() -> Self {
        Self {
            services: HashMap::new(),
            default_model: "test-model".to_string(),
        }
    }

    pub fn new(config: &LlmConfig) -> Self {
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();

        // Try to create each model from the centralized definitions
        for model_def in all_models() {
            if let Some(service) = Self::try_create_model(model_def, config) {
                services.insert(model_def.id.to_string(), service);
            }
        }

        // Determine default model
        let default_model = config
            .default_model
            .clone()
            .or_else(|| {
                // Try claude-4.5-sonnet first (our preferred default)
                if services.contains_key("claude-4.5-sonnet") {
                    Some("claude-4.5-sonnet".to_string())
                } else {
                    // Fall back to first available model
                    services.keys().next().cloned()
                }
            })
            .unwrap_or_else(|| "claude-4.5-sonnet".to_string());

        Self {
            services,
            default_model,
        }
    }

    /// Try to create a model service, validating prerequisites
    fn try_create_model(
        model_def: &super::ModelDef,
        config: &LlmConfig,
    ) -> Option<Arc<dyn LlmService>> {
        // In gateway mode, use "implicit" as the API key
        // The gateway will handle the actual authentication
        let api_key = if config.gateway.is_some() {
            "implicit".to_string()
        } else {
            // Direct mode: require actual API key
            match model_def.provider {
                Provider::Anthropic => config.anthropic_api_key.as_ref()?,
                Provider::OpenAI => config.openai_api_key.as_ref()?,
                Provider::Fireworks => config.fireworks_api_key.as_ref()?,
            }
            .clone()
        };

        // In direct mode, don't allow empty keys
        if config.gateway.is_none() && api_key.is_empty() {
            return None;
        }

        // Try to create the service using the factory
        match (model_def.factory)(&api_key, config.gateway.as_deref()) {
            Ok(service) => {
                // Wrap with logging
                Some(Arc::new(LoggingService::new(service)))
            }
            Err(_) => None,
        }
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

    /// List all available model IDs
    pub fn available_models(&self) -> Vec<String> {
        let mut models: Vec<_> = self.services.keys().cloned().collect();
        models.sort();
        models
    }

    /// Get detailed information about available models
    pub fn available_model_info(&self) -> Vec<crate::api::ModelInfo> {
        let mut model_infos = Vec::new();

        // Get info for each registered model
        for model_def in super::all_models() {
            if self.services.contains_key(model_def.id) {
                model_infos.push(crate::api::ModelInfo {
                    id: model_def.id.to_string(),
                    provider: model_def.provider.display_name().to_string(),
                    description: model_def.description.to_string(),
                    context_window: model_def.context_window,
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
    /// Prefers: claude-4.5-haiku > gpt-4o-mini > any available model
    pub fn get_cheap_model(&self) -> Option<Arc<dyn LlmService>> {
        // Priority order for cheap models
        const CHEAP_MODELS: &[&str] = &["claude-4.5-haiku", "gpt-4o-mini", "gpt-5-mini"];

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
                "Expected claude model, got {}",
                model_id
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
        assert!(registry.get("claude-4.5-sonnet").is_some());
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
        assert!(models.contains(&"claude-4.5-opus".to_string()));
    }

    #[test]
    fn test_default_model_selection() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        // Should default to claude-4.5-sonnet
        assert_eq!(registry.default_model_id(), "claude-4.5-sonnet");
    }

    #[test]
    fn test_custom_default_model() {
        let config = LlmConfig {
            anthropic_api_key: Some("test-key".to_string()),
            default_model: Some("claude-4.5-opus".to_string()),
            ..Default::default()
        };
        let registry = ModelRegistry::new(&config);

        assert_eq!(registry.default_model_id(), "claude-4.5-opus");
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
        let opus = model_infos.iter().find(|m| m.id == "claude-4.5-opus");
        assert!(opus.is_some());
        let opus = opus.unwrap();
        assert_eq!(opus.provider, "Anthropic");
        assert!(opus.description.contains("most capable"));
        assert_eq!(opus.context_window, 200_000);
    }
}
