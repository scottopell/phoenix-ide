//! Centralized model definitions for all LLM providers
//!
//! This module contains all model definitions in a single location,
//! making it easier to add new models and providers.

/// Credential/auth family required to satisfy a model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthFamily {
    Anthropic,
    OpenAI,
    Gateway,
    None,
}

/// User-facing provider/catalog family. This is presentation metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelFamily {
    Anthropic,
    OpenAI,
    Google,
    Mock,
}

impl ModelFamily {
    pub fn display_name(self) -> &'static str {
        match self {
            ModelFamily::Anthropic => "Anthropic",
            ModelFamily::OpenAI => "OpenAI",
            ModelFamily::Google => "Google",
            ModelFamily::Mock => "Mock",
        }
    }
}

/// Gateway routing metadata: provider header plus path family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRoute {
    pub provider_header: String,
}

impl GatewayRoute {
    pub fn new(provider_header: impl Into<String>) -> Self {
        Self {
            provider_header: provider_header.into(),
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
    /// `OpenAI`-compatible Chat Completions API
    OpenAIChat,
}

/// Model specification with metadata
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// User-facing model ID (e.g., "claude-4.5-opus")
    pub id: String,
    /// API name used by the provider (e.g., "claude-opus-4-5-20251101")
    pub api_name: String,
    /// Presentation-only model family
    pub family: ModelFamily,
    /// Credential/auth family
    pub auth_family: AuthFamily,
    /// Gateway provider alias/header
    pub gateway_route: Option<GatewayRoute>,
    /// API format / wire protocol
    pub api_format: ApiFormat,
    /// Human-readable description
    pub description: String,
    /// Context window size in tokens
    pub context_window: usize,
    /// Recommended for most users (shown by default in UI)
    pub recommended: bool,
    /// Whether this model supports Anthropic's tool search feature
    pub supports_tool_search: bool,
}

impl ModelSpec {
    pub fn gateway_provider_header(&self) -> Option<&str> {
        self.gateway_route
            .as_ref()
            .map(|route| route.provider_header.as_str())
    }
}

fn anthropic_route() -> GatewayRoute {
    GatewayRoute::new("anthropic")
}

fn openai_route() -> GatewayRoute {
    GatewayRoute::new("openai")
}

fn google_route() -> GatewayRoute {
    GatewayRoute::new("google")
}

/// Get all available model specifications
#[allow(clippy::too_many_lines)]
pub fn all_models() -> Vec<ModelSpec> {
    vec![
        // Anthropic models
        // Note: 4.6+ models use stable (non-dated) API IDs; id matches api_name for correct lookup.
        ModelSpec {
            id: "claude-opus-4-7".into(),
            api_name: "claude-opus-4-7".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.7 (most capable, slower)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-7-1m".into(),
            api_name: "claude-opus-4-7".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.7 (1M context)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-6".into(),
            api_name: "claude-opus-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.6 (legacy)".into(),
            context_window: 200_000,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-sonnet-4-6".into(),
            api_name: "claude-sonnet-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Sonnet 4.6 (balanced performance)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-haiku-4-5".into(),
            api_name: "claude-haiku-4-5-20251001".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Haiku 4.5 (fast, efficient)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "claude-opus-4-6-1m".into(),
            api_name: "claude-opus-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.6 (1M context, legacy)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-sonnet-4-6-1m".into(),
            api_name: "claude-sonnet-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Sonnet 4.6 (1M context)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-5".into(),
            api_name: "claude-opus-4-5-20251101".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            gateway_route: Some(anthropic_route()),
            api_format: ApiFormat::Anthropic,
            description: "Claude Opus 4.5 (legacy)".into(),
            context_window: 200_000,
            recommended: false,
            supports_tool_search: true,
        },
        // OpenAI models
        // GPT-5 models
        ModelSpec {
            id: "gpt-5.5".into(),
            api_name: "gpt-5.5".into(),
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            gateway_route: Some(openai_route()),
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.5 (frontier, 1M context)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "gpt-5.4".into(),
            api_name: "gpt-5.4".into(),
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            gateway_route: Some(openai_route()),
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.4 (frontier, native computer use)".into(),
            context_window: 400_000,
            recommended: false,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "gpt-5.4-mini".into(),
            api_name: "gpt-5.4-mini".into(),
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            gateway_route: Some(openai_route()),
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
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            gateway_route: Some(openai_route()),
            api_format: ApiFormat::OpenAIResponses,
            description: "GPT-5.3 Codex (latest code model)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: false,
        },
        // AI Gateway Google model (OpenAI-compatible chat/completions).
        ModelSpec {
            id: "gemini-2.5-flash".into(),
            api_name: "google/gemini-2.5-flash".into(),
            family: ModelFamily::Google,
            auth_family: AuthFamily::Gateway,
            gateway_route: Some(google_route()),
            api_format: ApiFormat::OpenAIChat,
            description: "Gemini 2.5 Flash via AI Gateway (chat completions)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: false,
        },
        // Mock model for frontend development without API keys
        ModelSpec {
            id: "mock".into(),
            api_name: "mock".into(),
            family: ModelFamily::Mock,
            auth_family: AuthFamily::None,
            gateway_route: Some(GatewayRoute::new("mock")),
            api_format: ApiFormat::Anthropic, // unused by mock, but needed for the struct
            description: "Mock (lorem ipsum for UI dev)".into(),
            context_window: 200_000,
            recommended: false,
            supports_tool_search: false,
        },
    ]
}
