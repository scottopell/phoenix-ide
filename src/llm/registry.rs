//! Model registry for managing available LLM providers

use super::{AnthropicService, LlmService, LoggingService};
use super::anthropic::AnthropicModel;
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for LLM providers
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub fireworks_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    /// exe.dev gateway URL (e.g., "https://meteor-rain.exe.xyz")
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
            gemini_api_key: std::env::var("GEMINI_API_KEY").ok(),
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
    pub fn new(config: &LlmConfig) -> Self {
        let mut services: HashMap<String, Arc<dyn LlmService>> = HashMap::new();
        
        if config.gateway.is_some() {
            // Gateway mode: register all models, gateway handles API keys
            Self::register_all_models(&mut services, config);
        } else {
            // Direct mode: register only models with API keys
            if config.anthropic_api_key.is_some() {
                Self::register_anthropic_models(&mut services, config);
            }
            // OpenAI and Fireworks support can be added later
        }
        
        // Determine default model
        let default_model = config.default_model.clone()
            .or_else(|| services.keys().next().cloned())
            .unwrap_or_else(|| "claude-4-sonnet".to_string());
        
        Self { services, default_model }
    }

    fn register_all_models(services: &mut HashMap<String, Arc<dyn LlmService>>, config: &LlmConfig) {
        Self::register_anthropic_models(services, config);
        // Additional providers can be registered here
    }

    fn register_anthropic_models(services: &mut HashMap<String, Arc<dyn LlmService>>, config: &LlmConfig) {
        let key = config.anthropic_api_key.clone().unwrap_or_default();
        let gateway = config.gateway.as_deref();
        
        let models = [
            ("claude-4-opus", AnthropicModel::Claude4Opus),
            ("claude-4-sonnet", AnthropicModel::Claude4Sonnet),
            ("claude-3.5-sonnet", AnthropicModel::Claude35Sonnet),
            ("claude-3.5-haiku", AnthropicModel::Claude35Haiku),
        ];
        
        for (id, model) in models {
            let service = AnthropicService::new(key.clone(), model, gateway);
            let logged = LoggingService::new(Arc::new(service));
            services.insert(id.to_string(), Arc::new(logged));
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

    /// Check if any models are available
    pub fn has_models(&self) -> bool {
        !self.services.is_empty()
    }
}
