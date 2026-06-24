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

/// Backend route + wire protocol used for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelBackend {
    /// Anthropic Messages-compatible backend.
    Anthropic,
    /// `OpenAI` Responses-compatible backend.
    OpenAIResponses,
    /// In-process deterministic mock backend.
    Mock,
}

impl ModelBackend {
    /// Display name surfaced in `/api/models` and usage reports.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            ModelBackend::Anthropic => "Anthropic",
            ModelBackend::OpenAIResponses => "OpenAI",
            ModelBackend::Mock => "Mock",
        }
    }

    /// Lowercase backend name for compatibility headers.
    #[must_use]
    pub fn header_value(self) -> &'static str {
        match self {
            ModelBackend::Anthropic => "anthropic",
            ModelBackend::OpenAIResponses => "openai",
            ModelBackend::Mock => "mock",
        }
    }

    #[must_use]
    pub(crate) fn api_format(self) -> ApiFormat {
        match self {
            ModelBackend::Anthropic | ModelBackend::Mock => ApiFormat::Anthropic,
            ModelBackend::OpenAIResponses => ApiFormat::OpenAIResponses,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExternalBackend {
    #[serde(alias = "anthropic_messages")]
    Anthropic,
    #[serde(rename = "openai_responses")]
    OpenAIResponses,
}

impl From<ExternalBackend> for ModelBackend {
    fn from(value: ExternalBackend) -> Self {
        match value {
            ExternalBackend::Anthropic => ModelBackend::Anthropic,
            ExternalBackend::OpenAIResponses => ModelBackend::OpenAIResponses,
        }
    }
}

/// API format / wire protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiFormat {
    /// Anthropic Messages API
    Anthropic,
    /// `OpenAI` Responses API
    OpenAIResponses,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalModelSpec {
    id: String,
    api_name: Option<String>,
    backend: ExternalBackend,
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
    /// Backend route + wire protocol for this model.
    pub backend: ModelBackend,
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
/// `api_name` defaults to `id`, so compatible backend aliases can be trialled
/// without duplicating the same identifier in config.
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
            let backend: ModelBackend = spec.backend.into();
            Ok(ModelSpec {
                id,
                api_name,
                backend,
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
            backend: ModelBackend::Anthropic,
            description: "Claude Opus 4.8 (most capable, slower)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
        },
        ModelSpec {
            id: "claude-opus-4-7".into(),
            api_name: "claude-opus-4-7".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Opus 4.7 (legacy)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
        },
        ModelSpec {
            id: "claude-opus-4-6".into(),
            api_name: "claude-opus-4-6".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Opus 4.6 (legacy)".into(),
            context_window: 1_000_000,
            recommended: false,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
        },
        ModelSpec {
            id: "claude-sonnet-4-6".into(),
            api_name: "claude-sonnet-4-6".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Sonnet 4.6 (balanced performance)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
        },
        ModelSpec {
            id: "claude-haiku-4-5".into(),
            api_name: "claude-haiku-4-5-20251001".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Haiku 4.5 (fast, efficient)".into(),
            context_window: 200_000,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
        },
        // OpenAI models
        // Context windows here are the platform-API ceilings. The codex bridge
        // caps every built-in OpenAI Responses model at 272K regardless — that
        // override is applied at registration time in `registry.rs` when the
        // bridge path is selected, so this spec's value reaches the runtime
        // only for direct/provider-compatible routes.
        ModelSpec {
            id: "gpt-5.5".into(),
            api_name: "gpt-5.5".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.5 (frontier, 1M context)".into(),
            context_window: 1_000_000,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
        },
        ModelSpec {
            id: "gpt-5.4".into(),
            api_name: "gpt-5.4".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.4 (frontier, native computer use)".into(),
            context_window: 400_000,
            recommended: false,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
        },
        ModelSpec {
            id: "gpt-5.4-mini".into(),
            api_name: "gpt-5.4-mini".into(),
            backend: ModelBackend::OpenAIResponses,
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
            backend: ModelBackend::OpenAIResponses,
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
            backend: ModelBackend::Mock,
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
            r#"[{"id":"baseten/moonshotai/Kimi-K2.6","backend":"anthropic","description":"Baseten Kimi K2.6 open-weight POC","context_window":262000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect("external model config should parse");

        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.id, "baseten/moonshotai/Kimi-K2.6");
        assert_eq!(model.api_name, model.id);
        assert_eq!(model.backend, ModelBackend::Anthropic);
        assert_eq!(model.backend.api_format(), ApiFormat::Anthropic);
        assert_eq!(model.context_window, 262_000);
        assert!(!model.supports_tool_search);
    }

    #[test]
    fn parses_documented_openai_external_names() {
        let models = parse_external_models(
            r#"[{"id":"openai-compatible/model","backend":"openai_responses","description":"OpenAI-compatible POC","context_window":128000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect("documented OpenAI config values should parse");

        assert_eq!(models[0].backend, ModelBackend::OpenAIResponses);
        assert_eq!(models[0].backend.api_format(), ApiFormat::OpenAIResponses);
    }

    #[test]
    fn rejects_unknown_external_backend() {
        let err = parse_external_models(
            r#"[{"id":"bad-openai","backend":"not-a-backend","description":"Bad backend","context_window":128000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect_err("unknown backend should be rejected");

        assert!(err.contains("unknown variant"));
        assert!(err.contains("openai_responses"));
    }

    #[test]
    fn rejects_invalid_external_context_window() {
        let err = parse_external_models(
            r#"[{"id":"bad","backend":"anthropic","description":"Bad","context_window":0,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect_err("zero context window should be rejected");

        assert!(err.contains("context_window 0"));
    }

    #[test]
    fn duplicate_external_ids_do_not_override_builtins() {
        let external = parse_external_models(
            r#"[{"id":"claude-sonnet-4-6","api_name":"other-wire-name","backend":"anthropic","description":"Override attempt","context_window":123,"recommended":false,"supports_tool_search":false}]"#,
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
