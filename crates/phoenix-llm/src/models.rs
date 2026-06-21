//! Centralized model definitions for all LLM providers
//!
//! This module contains all model definitions in a single location,
//! making it easier to add new models and providers.

/// Per-model metadata surfaced to API consumers (the `/api/models` response and
/// the model picker). Built by [`super::ModelRegistry::available_model_info`]
/// from a [`ModelSpec`] plus the live service's effective context window.
#[derive(Debug, serde::Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub description: String,
    pub context_window: usize,
    pub recommended: bool,
}

/// LLM provider enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Mock,
}

impl Provider {
    /// Get the display name for this provider
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAI => "OpenAI",
            Provider::Mock => "Mock",
        }
    }

    /// Lowercase provider name for gateway `provider` header (e.g. "anthropic", "openai").
    #[must_use]
    pub fn header_value(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAI => "openai",
            Provider::Mock => "mock",
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
    /// API name used by the provider (e.g., "claude-haiku-4-5-20251001")
    pub api_name: String,
    /// Provider for this model
    pub provider: Provider,
    /// API format / wire protocol
    pub api_format: ApiFormat,
    /// Human-readable description
    pub description: String,
    /// Platform-API context window ceiling. **Not** route-aware — the codex
    /// bridge clamps this lower for every `OpenAI` model. Use
    /// [`Self::context_window_for`] to get the value that actually applies to
    /// a specific routed service. `pub(super)` so siblings inside this crate
    /// (which know whether they're on a bridge route) can still read it
    /// directly when needed; external callers must go through the method.
    pub(super) context_window: usize,
    /// Recommended for most users (shown by default in UI)
    pub recommended: bool,
    /// Whether this model supports Anthropic's tool search feature
    pub supports_tool_search: bool,
}

impl ModelSpec {
    /// Effective context window for this model when reached via `service`.
    /// The single point where the "codex bridge caps at 272K regardless of
    /// platform-API ceiling" rule lives. Every consumer that needs a real
    /// ceiling goes through here — the raw `context_window` field is
    /// `pub(super)` so external code can't accidentally read the unclamped
    /// value.
    pub fn context_window_for(&self, service: &dyn crate::LlmService) -> usize {
        if service.uses_codex_bridge() {
            crate::CODEX_BRIDGE_CONTEXT_WINDOW
        } else {
            self.context_window
        }
    }
}

/// Get all available model specifications
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn all_models() -> Vec<ModelSpec> {
    vec![
        // Anthropic models
        // Note: 4.6+ models use stable (non-dated) API IDs; id matches api_name for correct lookup.
        // Anthropic 4.6+ models have 1M-token context windows natively as of
        // their 2026-03-13 GA. The `context-1m-2025-08-07` beta header was
        // retired April 30, 2026 and is no longer required (or accepted on
        // older models). See migration 009 for legacy `-1m` id rewrite.
        ModelSpec {
            id: "claude-opus-4-8".into(),
            api_name: "claude-opus-4-8".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.8 (most capable, slower)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-7".into(),
            api_name: "claude-opus-4-7".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.7 (legacy)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-6".into(),
            api_name: "claude-opus-4-6".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.6 (legacy)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-sonnet-4-6".into(),
            api_name: "claude-sonnet-4-6".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Sonnet 4.6 (balanced performance)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-haiku-4-5".into(),
            api_name: "claude-haiku-4-5-20251001".into(),
            provider: Provider::Anthropic,
            api_format: ApiFormat::Anthropic,
            description: "Claude Haiku 4.5 (fast, efficient)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: false,
        },
        // OpenAI models
        // Context windows here are the platform-API ceilings (what gateway/
        // direct-API callers can actually use). The codex bridge caps every
        // model at 272K regardless — that override is applied at registration
        // time in `registry.rs` when the bridge path is selected, so this
        // spec's value reaches the runtime only for gateway/direct routes.
        ModelSpec {
            id: "gpt-5.5".into(),
            api_name: "gpt-5.5".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.5 (frontier, 1M context)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "gpt-5.4".into(),
            api_name: "gpt-5.4".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.4 (frontier, native computer use)".into(),
            context_window: 400_000,
            recommended: false,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "gpt-5.4-mini".into(),
            api_name: "gpt-5.4-mini".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.4 Mini (fast, efficient)".into(),
            context_window: 400_000,
            recommended: true,
            supports_tool_search: false,
        },
        // GPT-5 Codex models (responses API)
        ModelSpec {
            id: "gpt-5.3-codex".into(),
            api_name: "gpt-5.3-codex".into(),
            provider: Provider::OpenAI,
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.3 Codex (latest code model)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: false,
        },
        // Mock model for frontend development without API keys
        ModelSpec {
            id: "mock".into(),
            api_name: "mock".into(),
            provider: Provider::Mock,
            api_format: ApiFormat::Anthropic, // unused by mock, but needed for the struct
            description: "Mock (lorem ipsum for UI dev)".into(),
            context_window: 200_000,
            recommended: false,
            supports_tool_search: false,
        },
    ]
}
