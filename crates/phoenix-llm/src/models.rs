//! Centralized model definitions for all LLM providers
//!
use std::collections::HashSet;

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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExternalProvider {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
}

impl From<ExternalProvider> for Provider {
    fn from(value: ExternalProvider) -> Self {
        match value {
            ExternalProvider::Anthropic => Provider::Anthropic,
            ExternalProvider::OpenAI => Provider::OpenAI,
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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExternalApiFormat {
    Anthropic,
    #[serde(rename = "openai_responses")]
    OpenAIResponses,
}

impl From<ExternalApiFormat> for ApiFormat {
    fn from(value: ExternalApiFormat) -> Self {
        match value {
            ExternalApiFormat::Anthropic => ApiFormat::Anthropic,
            ExternalApiFormat::OpenAIResponses => ApiFormat::OpenAIResponses,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalModelSpec {
    id: String,
    api_name: Option<String>,
    provider: ExternalProvider,
    api_format: ExternalApiFormat,
    description: String,
    context_window: usize,
    recommended: bool,
    supports_tool_search: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    BuiltIn,
    External,
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
    /// Where this model definition came from. External `OpenAI`-compatible
    /// specs bypass the Codex bridge because their endpoint is operator-configured,
    /// not `ChatGPT`'s backend.
    pub source: ModelSource,
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

/// Parse additional model specs from the `PHOENIX_LLM_MODELS` inline JSON format.
///
/// `api_name` defaults to `id`, so Anthropic-compatible gateway aliases can be
/// trialled without duplicating the same identifier in config.
///
/// # Errors
/// Returns an actionable validation error. The raw JSON is never included in the
/// error string, so callers can log it without echoing deployment config.
pub fn parse_external_models(raw: &str) -> Result<Vec<ModelSpec>, String> {
    let specs: Vec<ExternalModelSpec> = serde_json::from_str(raw)
        .map_err(|e| format!("invalid JSON for PHOENIX_LLM_MODELS: {e}"))?;

    specs
        .into_iter()
        .enumerate()
        .map(|(index, spec)| {
            let id = spec.id.trim().to_string();
            if id.is_empty() {
                return Err(format!("model at index {index} has an empty id"));
            }
            let api_name = spec
                .api_name
                .map_or_else(|| id.clone(), |name| name.trim().to_string());
            if api_name.is_empty() {
                return Err(format!("model '{id}' has an empty api_name"));
            }
            let description = spec.description.trim().to_string();
            if description.is_empty() {
                return Err(format!("model '{id}' has an empty description"));
            }
            if spec.context_window == 0 {
                return Err(format!("model '{id}' has invalid context_window 0"));
            }
            let provider: Provider = spec.provider.into();
            let api_format: ApiFormat = spec.api_format.into();
            if !matches_provider_api_format(provider, api_format) {
                return Err(format!(
                    "model '{id}' has mismatched provider/api_format: {} requires {}",
                    provider.header_value(),
                    provider.expected_api_format_name()
                ));
            }
            Ok(ModelSpec {
                id,
                api_name,
                provider,
                api_format,
                description,
                context_window: spec.context_window,
                recommended: spec.recommended,
                supports_tool_search: spec.supports_tool_search,
                source: ModelSource::External,
            })
        })
        .collect()
}

/// Merge built-in models with externally configured additions.
///
/// Duplicate IDs are rejected: the first definition wins, which preserves the
/// built-in model contract and prevents silent overrides from config.
#[must_use]
pub fn merge_model_specs(mut builtins: Vec<ModelSpec>, external: &[ModelSpec]) -> Vec<ModelSpec> {
    let mut ids: HashSet<String> = builtins.iter().map(|spec| spec.id.clone()).collect();
    for spec in external {
        if !ids.insert(spec.id.clone()) {
            tracing::warn!(
                model_id = %spec.id,
                "PHOENIX_LLM_MODELS duplicate model id ignored; built-in or earlier configured model kept"
            );
            continue;
        }
        builtins.push(spec.clone());
    }
    builtins
}

fn matches_provider_api_format(provider: Provider, api_format: ApiFormat) -> bool {
    matches!(
        (provider, api_format),
        (Provider::Anthropic, ApiFormat::Anthropic)
            | (Provider::OpenAI, ApiFormat::OpenAIResponses)
    )
}

impl Provider {
    fn expected_api_format_name(self) -> &'static str {
        match self {
            Provider::OpenAI => "openai_responses",
            Provider::Anthropic | Provider::Mock => "anthropic",
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
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
            source: ModelSource::BuiltIn,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_external_model_with_api_name_defaulting_to_id() {
        let models = parse_external_models(
            r#"[{"id":"baseten/moonshotai/Kimi-K2.6","provider":"anthropic","api_format":"anthropic","description":"Baseten Kimi K2.6 open-weight POC","context_window":262000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect("external model config should parse");

        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.id, "baseten/moonshotai/Kimi-K2.6");
        assert_eq!(model.api_name, model.id);
        assert_eq!(model.provider, Provider::Anthropic);
        assert_eq!(model.api_format, ApiFormat::Anthropic);
        assert_eq!(model.context_window, 262_000);
        assert!(!model.supports_tool_search);
    }

    #[test]
    fn parses_documented_openai_external_names() {
        let models = parse_external_models(
            r#"[{"id":"openai-compatible/model","provider":"openai","api_format":"openai_responses","description":"OpenAI-compatible POC","context_window":128000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect("documented OpenAI config values should parse");

        assert_eq!(models[0].provider, Provider::OpenAI);
        assert_eq!(models[0].api_format, ApiFormat::OpenAIResponses);
    }

    #[test]
    fn rejects_crossed_provider_api_format_pairs() {
        let err = parse_external_models(
            r#"[{"id":"bad-openai","provider":"openai","api_format":"anthropic","description":"Bad crossed pair","context_window":128000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect_err("crossed provider/api_format pairs should be rejected");

        assert!(err.contains("mismatched provider/api_format"));
        assert!(err.contains("openai_responses"));
    }

    #[test]
    fn rejects_invalid_external_context_window() {
        let err = parse_external_models(
            r#"[{"id":"bad","provider":"anthropic","api_format":"anthropic","description":"Bad","context_window":0,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect_err("zero context window should be rejected");

        assert!(err.contains("context_window 0"));
    }

    #[test]
    fn duplicate_external_ids_do_not_override_builtins() {
        let external = parse_external_models(
            r#"[{"id":"claude-sonnet-4-6","api_name":"other-wire-name","provider":"anthropic","api_format":"anthropic","description":"Override attempt","context_window":123,"recommended":false,"supports_tool_search":false}]"#,
        )
        .unwrap();

        let merged = merge_model_specs(all_models(), &external);
        let sonnet = merged
            .iter()
            .find(|spec| spec.id == "claude-sonnet-4-6")
            .unwrap();

        assert_ne!(sonnet.api_name, "other-wire-name");
        assert_ne!(sonnet.context_window, 123);
    }
}
