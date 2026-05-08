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
    /// API name used by the provider (e.g., "claude-opus-4-5-20251101").
    /// May be provider-prefixed (e.g., "google/gemini-2.5-flash") for gateway models.
    pub api_name: String,
    /// Presentation-only model family
    pub family: ModelFamily,
    /// Credential/auth family — also determines `api_format()`.
    pub auth_family: AuthFamily,
    /// Human-readable description
    pub description: String,
    /// Context window size in tokens
    pub context_window: usize,
    /// Max output tokens to request per turn. Used by the executor when building
    /// the LLM request and by the state machine to size the continuation
    /// threshold (we have to leave at least this many tokens of headroom or the
    /// next request would exceed `context_window`).
    pub max_output_tokens: u32,
    /// Recommended for most users (shown by default in UI)
    pub recommended: bool,
    /// Whether this model supports Anthropic's tool search feature
    pub supports_tool_search: bool,
}

impl ModelSpec {
    /// Wire format derived from `auth_family`. The mapping is 1:1 today;
    /// if a future model needs a different combination, this becomes a field.
    pub fn api_format(&self) -> ApiFormat {
        match self.auth_family {
            AuthFamily::Anthropic | AuthFamily::None => ApiFormat::Anthropic,
            AuthFamily::OpenAI => ApiFormat::OpenAIResponses,
            AuthFamily::Gateway => ApiFormat::OpenAIChat,
        }
    }

    /// Provider prefix used for the legacy gateway `provider:` header and for
    /// constructing provider-prefixed names during discovery matching. Derived
    /// from `api_name` if it contains a `/`, else from `auth_family`.
    pub fn provider_prefix(&self) -> &str {
        if let Some((prefix, _)) = self.api_name.split_once('/') {
            return prefix;
        }
        match self.auth_family {
            AuthFamily::Anthropic => "anthropic",
            AuthFamily::OpenAI | AuthFamily::Gateway => "openai",
            AuthFamily::None => "mock",
        }
    }
}

/// Default per-turn output cap. Used by every hardcoded `ModelSpec` unless it
/// has a specific reason to deviate (e.g. small-context models that need more
/// input headroom). Tune in one place.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 16_384;

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
            description: "Claude Opus 4.7 (most capable, slower)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-7-1m".into(),
            api_name: "claude-opus-4-7".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Opus 4.7 (1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-6".into(),
            api_name: "claude-opus-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Opus 4.6 (legacy)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-sonnet-4-6".into(),
            api_name: "claude-sonnet-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Sonnet 4.6 (balanced performance)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-haiku-4-5".into(),
            api_name: "claude-haiku-4-5-20251001".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Haiku 4.5 (fast, efficient)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "claude-opus-4-6-1m".into(),
            api_name: "claude-opus-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Opus 4.6 (1M context, legacy)".into(),
            context_window: 1_000_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-sonnet-4-6-1m".into(),
            api_name: "claude-sonnet-4-6".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Sonnet 4.6 (1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: false,
            supports_tool_search: true,
        },
        ModelSpec {
            id: "claude-opus-4-5".into(),
            api_name: "claude-opus-4-5-20251101".into(),
            family: ModelFamily::Anthropic,
            auth_family: AuthFamily::Anthropic,
            description: "Claude Opus 4.5 (legacy)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
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
            description: "GPT-5.5 (frontier, 1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "gpt-5.4".into(),
            api_name: "gpt-5.4".into(),
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            description: "GPT-5.4 (frontier, native computer use)".into(),
            context_window: 400_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: false,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "gpt-5.4-mini".into(),
            api_name: "gpt-5.4-mini".into(),
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            description: "GPT-5.4 Mini (fast, efficient)".into(),
            context_window: 400_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        // GPT-5 Codex models (responses API)
        ModelSpec {
            id: "gpt-5.3-codex".into(),
            api_name: "gpt-5.3-codex".into(),
            family: ModelFamily::OpenAI,
            auth_family: AuthFamily::OpenAI,
            description: "GPT-5.3 Codex (latest code model)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        // AI Gateway Google/Gemini models (OpenAI-compatible chat/completions).
        ModelSpec {
            id: "gemini-2.5-flash".into(),
            api_name: "google/gemini-2.5-flash".into(),
            family: ModelFamily::Google,
            auth_family: AuthFamily::Gateway,
            description: "Gemini 2.5 Flash (fast, 1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "kimi-k2".into(),
            api_name: "google/moonshotai/kimi-k2-6".into(),
            family: ModelFamily::Google,
            auth_family: AuthFamily::Gateway,
            description: "Kimi K2 (Moonshot AI, strong at coding/reasoning)".into(),
            context_window: 131_072,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "qwen3-coder".into(),
            api_name: "google/qwen/qwen3-coder-next".into(),
            family: ModelFamily::Google,
            auth_family: AuthFamily::Gateway,
            description: "Qwen3 Coder Next (Alibaba, code-specialized)".into(),
            context_window: 131_072,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: true,
            supports_tool_search: false,
        },
        ModelSpec {
            id: "qwen3.5-4b".into(),
            api_name: "datadoginternal/Qwen/Qwen3.5-4B".into(),
            family: ModelFamily::Google,
            auth_family: AuthFamily::Gateway,
            description: "Qwen3.5-4B (small, fast, tool-calling capable)".into(),
            context_window: 65_536,
            // Smaller cap to leave headroom in the 64K window — 8K still fits
            // long outputs but trims the ContextExhausted threshold sensibly.
            max_output_tokens: 8_192,
            recommended: true,
            supports_tool_search: false,
        },
        // Mock model for frontend development without API keys
        ModelSpec {
            id: "mock".into(),
            api_name: "mock".into(),
            family: ModelFamily::Mock,
            auth_family: AuthFamily::None,
            description: "Mock (lorem ipsum for UI dev)".into(),
            context_window: 200_000,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            recommended: false,
            supports_tool_search: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock the 1:1 coupling between `auth_family` and `api_format()`.
    /// If a future model genuinely needs a different combination, both
    /// this test AND `ModelSpec::api_format()` must change — promoting
    /// `api_format` back to a stored field is the explicit hatch.
    #[test]
    fn api_format_matches_auth_family_for_every_model() {
        for spec in all_models() {
            let expected = match spec.auth_family {
                AuthFamily::Anthropic | AuthFamily::None => ApiFormat::Anthropic,
                AuthFamily::OpenAI => ApiFormat::OpenAIResponses,
                AuthFamily::Gateway => ApiFormat::OpenAIChat,
            };
            assert_eq!(
                spec.api_format(),
                expected,
                "auth_family/api_format mismatch for model {}",
                spec.id
            );
        }
    }

    /// `provider_prefix()` is load-bearing for both inference header
    /// injection AND discovery name matching. Lock the rules for every
    /// hardcoded model.
    #[test]
    fn provider_prefix_for_every_model() {
        for spec in all_models() {
            let prefix = spec.provider_prefix();
            assert!(
                !prefix.is_empty(),
                "provider_prefix must be non-empty for {}",
                spec.id
            );

            // If api_name has a slash, the prefix is the part before it.
            if let Some((expected, _)) = spec.api_name.split_once('/') {
                assert_eq!(
                    prefix, expected,
                    "model {} api_name '{}' should yield prefix '{}', got '{}'",
                    spec.id, spec.api_name, expected, prefix
                );
            } else {
                // Bare api_name → fall back to auth_family default.
                let expected = match spec.auth_family {
                    AuthFamily::Anthropic => "anthropic",
                    AuthFamily::OpenAI | AuthFamily::Gateway => "openai",
                    AuthFamily::None => "mock",
                };
                assert_eq!(
                    prefix, expected,
                    "model {} (auth={:?}) should default to prefix '{}', got '{}'",
                    spec.id, spec.auth_family, expected, prefix
                );
            }
        }
    }

    /// Sanity bounds — every model's max_output_tokens fits in its
    /// context_window with at least the safety margin the threshold
    /// check uses (2_000). Catches obvious misconfigurations early.
    #[test]
    fn max_output_fits_in_context_window() {
        const MIN_HEADROOM: u32 = 2_000;
        for spec in all_models() {
            let window: u32 = spec
                .context_window
                .try_into()
                .expect("context_window should fit u32 for sanity check");
            assert!(
                spec.max_output_tokens + MIN_HEADROOM <= window,
                "model {} max_output_tokens ({}) leaves no room (window={})",
                spec.id,
                spec.max_output_tokens,
                window
            );
        }
    }
}
