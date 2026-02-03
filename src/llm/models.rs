//! Centralized model definitions for all LLM providers
//!
//! This module contains all model definitions in a single location,
//! making it easier to add new models and providers.

use super::{AnthropicService, GeminiService, OpenAIService, LlmService};
use super::anthropic::AnthropicModel;
use super::gemini::GeminiModel;
use super::openai::OpenAIModel;
use std::sync::Arc;

/// LLM provider enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    #[allow(dead_code)] // Future providers
    OpenAI,
    #[allow(dead_code)]
    Fireworks,
    #[allow(dead_code)]
    Gemini,
}

impl Provider {
    /// Get the display name for this provider
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAI => "OpenAI",
            Provider::Fireworks => "Fireworks",
            Provider::Gemini => "Gemini",
        }
    }
    
    /// Get the environment variable name for this provider's API key
    #[allow(dead_code)] // Will be used for error messages
    pub fn api_key_env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            Provider::Fireworks => "FIREWORKS_API_KEY",
            Provider::Gemini => "GEMINI_API_KEY",
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
            id: "gpt-5.2-codex",
            provider: Provider::OpenAI,
            api_name: "gpt-5.2-codex",
            description: "GPT-5.2 Codex (advanced coding)",
            context_window: 128_000,
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
            id: "glm-4.7-fireworks",
            provider: Provider::Fireworks,
            api_name: "accounts/fireworks/models/glm-4-7b-chat",
            description: "GLM-4.7 on Fireworks",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("glm-4.7-fireworks requires FIREWORKS_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GLM47Fireworks,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "qwen3-coder-fireworks",
            provider: Provider::Fireworks,
            api_name: "accounts/fireworks/models/qwen3-coder-480b-instruct",
            description: "Qwen3 Coder 480B on Fireworks",
            context_window: 32_768,
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
            id: "glm-4p6-fireworks",
            provider: Provider::Fireworks,
            api_name: "accounts/fireworks/models/glm-4p6-chat",
            description: "GLM-4P6 on Fireworks",
            context_window: 128_000,
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("glm-4p6-fireworks requires FIREWORKS_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(OpenAIService::new(
                    api_key.to_string(),
                    OpenAIModel::GLM4P6Fireworks,
                    gateway,
                )))
            },
        },
        
        // Gemini models
        ModelDef {
            id: "gemini-3-pro",
            provider: Provider::Gemini,
            api_name: "gemini-3.0-pro",
            description: "Gemini 3 Pro",
            context_window: 2_097_152, // 2M context
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gemini-3-pro requires GEMINI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(GeminiService::new(
                    api_key.to_string(),
                    GeminiModel::Gemini3Pro,
                    gateway,
                )))
            },
        },
        ModelDef {
            id: "gemini-3-flash",
            provider: Provider::Gemini,
            api_name: "gemini-3.0-flash",
            description: "Gemini 3 Flash",
            context_window: 1_048_576, // 1M context
            factory: |api_key, gateway| {
                if api_key.is_empty() {
                    return Err("gemini-3-flash requires GEMINI_API_KEY or gateway".to_string());
                }
                Ok(Arc::new(GeminiService::new(
                    api_key.to_string(),
                    GeminiModel::Gemini3Flash,
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
