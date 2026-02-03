//! Centralized model definitions for all LLM providers
//!
//! This module contains all model definitions in a single location,
//! making it easier to add new models and providers.

use super::{AnthropicService, LlmService};
use super::anthropic::AnthropicModel;
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
        // Future providers will be added here:
        // - OpenAI: gpt-5.2-codex, o3, o3-mini
        // - Fireworks: qwen3-coder-fireworks, glm-4.7-fireworks, glm-4p6-fireworks
        // - Gemini: gemini-3-pro, gemini-3-flash
    ]
}

/// Get the default model definition
#[allow(dead_code)] // Public API
pub fn default_model() -> &'static ModelDef {
    &all_models()[1] // claude-4.5-sonnet as default
}
