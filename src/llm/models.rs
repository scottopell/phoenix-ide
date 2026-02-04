//! Centralized model definitions for all LLM providers
//!
//! This module contains all model definitions in a single location,
//! making it easier to add new models and providers.

use super::{AnthropicService, OpenAIService, LlmService};
use super::anthropic::AnthropicModel;
use super::openai::OpenAIModel;
use std::sync::Arc;

/// LLM provider enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Fireworks,
}

impl Provider {
    /// Get the display name for this provider
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAI => "OpenAI",
            Provider::Fireworks => "Fireworks",
        }
    }
    
    /// Get the environment variable name for this provider's API key
    #[allow(dead_code)] // Will be used for error messages
    pub fn api_key_env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            Provider::Fireworks => "FIREWORKS_API_KEY",
        }
    }
}

/// Model definition with metadata
#[derive(Debug, Clone)]
pub struct ModelDef {
    /// User-facing model ID (e.g., "claude-4.5-opus")
    pub id: &'static str,
    /// Provider for this model
    pub provider: Provider,
    /// API name used by the provider (e.g., "claude-opus-4-5-20251101")
    #[allow(dead_code)] // Will be used when we support model updates
    pub api_name: &'static str,
    /// Human-readable description
    pub description: &'static str,
    /// Context window size in tokens
    pub context_window: usize,
    /// Factory function to create the service
    pub factory: fn(&str, Option<&str>) -> Result<Arc<dyn LlmService>, String>,
}

/// Get all available model definitions
pub fn all_models() -> &'static [ModelDef] {
    &[
        // Anthropic models
        ModelDef {
            id: "claude-4.5-opus",
            provider: Provider::Anthropic,
            api_name: "claude-opus-4-5-20251101",
            description: "Claude Opus 4.5 (most capable, slower)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                // Accept any non-empty key (including "implicit" for gateway mode)
                if api_key.is_empty() {
                    return Err("claude-4.5-opus requires ANTHROPIC_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(AnthropicService::new(
                    api_key.to_string(),
                    AnthropicModel::Claude4Opus,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "claude-4.5-sonnet",
            provider: Provider::Anthropic,
            api_name: "claude-sonnet-4-5-20250929",
            description: "Claude Sonnet 4.5 (balanced performance)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                // Accept any non-empty key (including "implicit" for gateway mode)
                if api_key.is_empty() {
                    return Err("claude-4.5-sonnet requires ANTHROPIC_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(AnthropicService::new(
                    api_key.to_string(),
                    AnthropicModel::Claude4Sonnet,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "claude-3.5-sonnet",
            provider: Provider::Anthropic,
            api_name: "claude-sonnet-4-20250514",
            description: "Claude 3.5 Sonnet (legacy)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                // Accept any non-empty key (including "implicit" for gateway mode)
                if api_key.is_empty() {
                    return Err("claude-3.5-sonnet requires ANTHROPIC_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(AnthropicService::new(
                    api_key.to_string(),
                    AnthropicModel::Claude35Sonnet,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "claude-4.5-haiku",
            provider: Provider::Anthropic,
            api_name: "claude-haiku-4-5-20251001",
            description: "Claude Haiku 4.5 (fast, efficient)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                // Accept any non-empty key (including "implicit" for gateway mode)
                if api_key.is_empty() {
                    return Err("claude-4.5-haiku requires ANTHROPIC_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(AnthropicService::new(
                    api_key.to_string(),
                    AnthropicModel::Claude35Haiku,
                    gateway,
                )))
            },
        },
        // Additional providers - These work in gateway mode
        // The gateway handles the actual API communication
        
        // OpenAI models
        ModelDef {
            id: "gpt-4o",
            provider: Provider::OpenAI,
            api_name: "gpt-4o",
            description: "GPT-4o (balanced, multimodal)",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-4o requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT4o,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "gpt-4o-mini",
            provider: Provider::OpenAI,
            api_name: "gpt-4o-mini",
            description: "GPT-4o Mini (fast, efficient)",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-4o-mini requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT4oMini,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "o4-mini",
            provider: Provider::OpenAI,
            api_name: "o4-mini",
            description: "O4-Mini (reasoning model)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("o4-mini requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::O4Mini,
                    gateway,
                )))
            },
        },
        // GPT-5 models (chat endpoint)
        ModelDef {
            id: "gpt-5",
            provider: Provider::OpenAI,
            api_name: "gpt-5",
            description: "GPT-5 (reasoning model)",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-5 requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT5,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "gpt-5-mini",
            provider: Provider::OpenAI,
            api_name: "gpt-5-mini",
            description: "GPT-5 Mini (fast reasoning)",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-5-mini requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT5Mini,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "gpt-5.1",
            provider: Provider::OpenAI,
            api_name: "gpt-5.1",
            description: "GPT-5.1 (latest GPT-5)",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-5.1 requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT51,
                    gateway,
                )))
            },
        },
        // GPT-5 Codex models (responses API)
        ModelDef {
            id: "gpt-5-codex",
            provider: Provider::OpenAI,
            api_name: "gpt-5-codex",
            description: "GPT-5 Codex (code generation)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-5-codex requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT5Codex,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "gpt-5.1-codex",
            provider: Provider::OpenAI,
            api_name: "gpt-5.1-codex",
            description: "GPT-5.1 Codex (advanced code)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-5.1-codex requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT51Codex,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "gpt-5.2-codex",
            provider: Provider::OpenAI,
            api_name: "gpt-5.2-codex",
            description: "GPT-5.2 Codex (latest code model)",
            context_window: 200_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gpt-5.2-codex requires OPENAI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GPT52Codex,
                    gateway,
                )))
            },
        },
        
        // Fireworks models
        ModelDef {
            id: "glm-4p7-fireworks",
            provider: Provider::Fireworks,
            api_name: "accounts/fireworks/models/glm-4p7",
            description: "GLM-4P7 on Fireworks",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("glm-4p7-fireworks requires FIREWORKS_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GLM4P7Fireworks,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "qwen3-coder-fireworks",
            provider: Provider::Fireworks,
            api_name: "accounts/fireworks/models/qwen3-coder-480b-a35b-instruct",
            description: "Qwen3 Coder 480B on Fireworks",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("qwen3-coder-fireworks requires FIREWORKS_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::QwenCoderFireworks,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "deepseek-v3-fireworks",
            provider: Provider::Fireworks,
            api_name: "accounts/fireworks/models/deepseek-v3p1",
            description: "DeepSeek V3 on Fireworks",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("deepseek-v3-fireworks requires FIREWORKS_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::DeepseekV3Fireworks,
                    gateway,
                )))
            },
        },
    ]
}

/// Get the default model definition
#[allow(dead_code)] // Public API
pub fn default_model() -> &'static ModelDef {
    &all_models()[1] // claude-4.5-sonnet as default
}
