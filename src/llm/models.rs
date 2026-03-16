//! Centralized model definitions for all LLM providers
//!
//! This module contains all model definitions in a single location,
//! making it easier to add new models and providers.

/// LLM provider enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAI,
}

impl Provider {
    /// Get the display name for this provider
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAI => "OpenAI",
        }
    }

    /// Lowercase provider name for gateway `provider` header (e.g. "anthropic", "openai").
    pub fn header_value(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAI => "openai",
        }
    }

    /// Get the environment variable name for this provider's API key
    #[allow(dead_code)] // Will be used for error messages
    pub fn api_key_env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
        }
    }
}

/// API format / wire protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    /// Anthropic Messages API
    Anthropic,
    /// `OpenAI` Responses API
    OpenAIResponses,
}

/// Model specification with metadata
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// User-facing model ID (e.g., "claude-4.5-opus")
    pub id: String,
    /// API name used by the provider (e.g., "claude-opus-4-5-20251101")
    pub api_name: String,
    /// Provider for this model
    pub provider: Provider,
    /// API format / wire protocol
    pub api_format: ApiFormat,
    /// Human-readable description
    pub description: String,
    /// Context window size in tokens
    pub context_window: usize,
    /// Recommended for most users (shown by default in UI)
    pub recommended: bool,
}

/// Get all available model specifications
#[allow(clippy::too_many_lines)]
pub fn all_models() -> Vec<ModelSpec> {
    vec![
        // Anthropic models
        // Note: 4.6 models use stable (non-dated) API IDs; id matches api_name for correct lookup.
        ModelSpec {
            id: "claude-opus-4-6".into(),
            api_name: "claude-opus-4-6".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.6 (most capable, slower)".into(),
            context_window: 200_000,
            recommended: true,
        },
        ModelSpec {
            id: "claude-sonnet-4-6".into(),
            api_name: "claude-sonnet-4-6".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Sonnet 4.6 (balanced performance)".into(),
            context_window: 200_000,
            recommended: true,
        },
        ModelSpec {
            id: "claude-haiku-4-5".into(),
            api_name: "claude-haiku-4-5-20251001".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Haiku 4.5 (fast, efficient)".into(),
            context_window: 200_000,
            recommended: true,
        },
        ModelSpec {
            id: "claude-opus-4-5".into(),
            api_name: "claude-opus-4-5-20251101".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.5 (legacy)".into(),
            context_window: 200_000,
            recommended: false,
        },
        // OpenAI models
        ModelSpec {
            id: "gpt-4o".into(),
            api_name: "gpt-4o".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-4o (balanced, multimodal)".into(),
            context_window: 128_000,
            recommended: true,
        },
        ModelSpec {
            id: "gpt-4o-mini".into(),
            api_name: "gpt-4o-mini".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-4o Mini (fast, efficient)".into(),
            context_window: 128_000,
            recommended: false,
        },
        ModelSpec {
            id: "o4-mini".into(),
            api_name: "o4-mini".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "O4-Mini (reasoning model)".into(),
            context_window: 200_000,
            recommended: true,
        },
        // GPT-5 models
        ModelSpec {
            id: "gpt-5".into(),
            api_name: "gpt-5".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5 (reasoning model)".into(),
            context_window: 128_000,
            recommended: true,
        },
        ModelSpec {
            id: "gpt-5-mini".into(),
            api_name: "gpt-5-mini".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5 Mini (fast reasoning)".into(),
            context_window: 128_000,
            recommended: false,
        },
        ModelSpec {
            id: "gpt-5.1".into(),
            api_name: "gpt-5.1".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.1 (latest GPT-5)".into(),
            context_window: 128_000,
            recommended: false,
        },
        // GPT-5 Codex models (responses API)
        ModelSpec {
            id: "gpt-5-codex".into(),
            api_name: "gpt-5-codex".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5 Codex (code generation)".into(),
            context_window: 200_000,
            recommended: false,
        },
        ModelSpec {
            id: "gpt-5.1-codex".into(),
            api_name: "gpt-5.1-codex".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.1 Codex (advanced code)".into(),
            context_window: 200_000,
            recommended: false,
        },
        ModelSpec {
            id: "gpt-5.2-codex".into(),
            api_name: "gpt-5.2-codex".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.2 Codex (latest code model)".into(),
            context_window: 200_000,
            recommended: true,
        },
    ]
}

/// Get the default model specification
#[allow(dead_code)]
pub fn default_model() -> ModelSpec {
    all_models()
        .into_iter()
        .find(|m| m.id == "claude-sonnet-4-6")
        .expect("claude-sonnet-4-6 must be in all_models()")
}
