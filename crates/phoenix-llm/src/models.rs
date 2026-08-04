//! Centralized model definitions for all LLM providers
//!
use std::collections::HashSet;

use phoenix_core::domain::llm_types::ModelEffort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortCapabilities {
    Unsupported,
    Unknown,
    Supported(SupportedEffortCapabilities),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedEffortCapabilities {
    levels: Vec<ModelEffort>,
    native_default: NativeDefault,
}

impl SupportedEffortCapabilities {
    #[must_use]
    pub fn levels(&self) -> &[ModelEffort] {
        &self.levels
    }

    #[must_use]
    pub const fn native_default(&self) -> NativeDefault {
        self.native_default
    }
}

impl serde::Serialize for EffortCapabilities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        match self {
            Self::Unsupported => map.serialize_entry("support", "unsupported")?,
            Self::Unknown => map.serialize_entry("support", "unknown")?,
            Self::Supported(capabilities) => {
                map.serialize_entry("support", "supported")?;
                map.serialize_entry("levels", capabilities.levels())?;
                map.serialize_entry("native_default", &capabilities.native_default())?;
            }
        }
        map.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDefault {
    Known(ModelEffort),
    Unknown,
}

impl EffortCapabilities {
    #[must_use]
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self::Unknown
    }

    /// # Panics
    ///
    /// Panics when `levels` is empty or a known native default is not one of the
    /// supported levels.
    #[must_use]
    pub fn supported(levels: &[ModelEffort], native_default: NativeDefault) -> Self {
        assert!(
            !levels.is_empty(),
            "supported effort levels must be non-empty"
        );
        if let NativeDefault::Known(level) = native_default {
            assert!(
                levels.contains(&level),
                "native effort default must be a supported level"
            );
        }
        Self::Supported(SupportedEffortCapabilities {
            levels: levels.to_vec(),
            native_default,
        })
    }

    #[must_use]
    pub fn supported_known(levels: &[ModelEffort], native_default: ModelEffort) -> Self {
        Self::supported(levels, NativeDefault::Known(native_default))
    }

    #[must_use]
    pub fn supported_unknown(levels: &[ModelEffort]) -> Self {
        Self::supported(levels, NativeDefault::Unknown)
    }

    #[must_use]
    pub fn supports(&self, effort: ModelEffort) -> bool {
        matches!(self, Self::Supported(capabilities) if capabilities.levels().contains(&effort))
    }
}

const EFFORT_LEVELS_ANTHROPIC_BASE: &[ModelEffort] = &[
    ModelEffort::Low,
    ModelEffort::Medium,
    ModelEffort::High,
    ModelEffort::Max,
];
const EFFORT_LEVELS_ANTHROPIC_XHIGH: &[ModelEffort] = &[
    ModelEffort::Low,
    ModelEffort::Medium,
    ModelEffort::High,
    ModelEffort::Xhigh,
    ModelEffort::Max,
];
const EFFORT_LEVELS_GPT_55_PLUS: &[ModelEffort] = &[
    ModelEffort::None,
    ModelEffort::Low,
    ModelEffort::Medium,
    ModelEffort::High,
    ModelEffort::Xhigh,
    ModelEffort::Max,
];
const EFFORT_LEVELS_GPT_54: &[ModelEffort] = &[
    ModelEffort::None,
    ModelEffort::Low,
    ModelEffort::Medium,
    ModelEffort::High,
    ModelEffort::Xhigh,
];
fn effort_anthropic_base() -> EffortCapabilities {
    EffortCapabilities::supported_known(EFFORT_LEVELS_ANTHROPIC_BASE, ModelEffort::High)
}

fn effort_anthropic_xhigh() -> EffortCapabilities {
    EffortCapabilities::supported_known(EFFORT_LEVELS_ANTHROPIC_XHIGH, ModelEffort::High)
}

fn effort_gpt_55_plus() -> EffortCapabilities {
    EffortCapabilities::supported_known(EFFORT_LEVELS_GPT_55_PLUS, ModelEffort::Medium)
}

fn effort_gpt_54() -> EffortCapabilities {
    EffortCapabilities::supported_known(EFFORT_LEVELS_GPT_54, ModelEffort::None)
}

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
    pub effort_capabilities: EffortCapabilities,
}

/// Backend route + wire protocol used for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    #[serde(rename = "anthropic", alias = "anthropic_messages")]
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
struct ExternalModelSpec {
    id: String,
    api_name: Option<String>,
    backend: ExternalBackend,
    description: String,
    context_window: usize,
    max_output_tokens: Option<u32>,
    recommended: bool,
    supports_tool_search: bool,
    effort_capabilities: Option<ExternalEffortCapabilities>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "support")]
enum ExternalEffortCapabilities {
    Unsupported,
    Unknown,
    Supported {
        levels: Vec<ModelEffort>,
        native_default: Option<ModelEffort>,
    },
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
    pub(super) max_output_tokens: Option<u32>,
    /// Recommended for most users (shown by default in UI)
    pub recommended: bool,
    /// Whether this model supports Anthropic's tool search feature
    pub supports_tool_search: bool,
    /// Where this model definition came from. External `OpenAI`-compatible
    /// specs bypass the Codex bridge because their endpoint is operator-configured,
    /// not `ChatGPT`'s backend.
    pub source: ModelSource,
    /// Route-aware effort capabilities. Built-in specs describe the native
    /// provider defaults, while external specs carry validated optional metadata
    /// when an operator knows the target route's support. When absent on an
    /// external model, Phoenix represents that absence honestly instead of
    /// fabricating unsupported/optional/native.
    pub effort_capabilities: EffortCapabilities,
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

    #[must_use]
    pub const fn output_token_limit(&self) -> Option<u32> {
        self.max_output_tokens
    }

    #[must_use]
    pub fn effort_capabilities_for(&self, _service: &dyn crate::LlmService) -> EffortCapabilities {
        self.effort_capabilities.clone()
    }
}

/// Parse additional model specs from the `PHOENIX_LLM_MODELS` inline JSON format.
///
/// `api_name` defaults to `id`, so compatible backend aliases can be trialled
/// without duplicating the same identifier in config.
///
/// # Errors
/// Returns valid model specs. Invalid entries are logged and skipped so one bad
/// item does not suppress the rest of the configured external models.
pub fn parse_external_models(raw: &str) -> Result<Vec<ModelSpec>, String> {
    let specs: Vec<serde_json::Value> =
        serde_json::from_str(raw).map_err(|_| "invalid JSON for PHOENIX_LLM_MODELS".to_string())?;

    let mut parsed = Vec::new();
    for (index, value) in specs.into_iter().enumerate() {
        match serde_json::from_value::<ExternalModelSpec>(value)
            .map_err(|_| format!("model at index {index} has invalid shape"))
            .and_then(|spec| external_model_spec_from_config(index, spec))
        {
            Ok(spec) => parsed.push(spec),
            Err(error) => {
                tracing::warn!(error = %error, "ignoring invalid PHOENIX_LLM_MODELS entry");
            }
        }
    }
    Ok(parsed)
}

fn validate_external_effort_capabilities(
    spec: &ExternalModelSpec,
) -> Result<EffortCapabilities, String> {
    let Some(caps) = spec.effort_capabilities.as_ref() else {
        return Ok(EffortCapabilities::unknown());
    };
    match caps {
        ExternalEffortCapabilities::Unsupported => Ok(EffortCapabilities::unsupported()),
        ExternalEffortCapabilities::Unknown => Ok(EffortCapabilities::unknown()),
        ExternalEffortCapabilities::Supported {
            levels,
            native_default,
        } => {
            if levels.is_empty() {
                return Err(format!(
                    "model '{}' must declare at least one supported effort level",
                    spec.id
                ));
            }
            if matches!(spec.backend, ExternalBackend::Anthropic)
                && levels
                    .iter()
                    .any(|level| matches!(level, ModelEffort::None | ModelEffort::Minimal))
            {
                return Err(format!(
                    "model '{}' declares an Anthropic-invalid effort level",
                    spec.id
                ));
            }
            let native_default = match native_default {
                Some(level) => {
                    if !levels.contains(level) {
                        return Err(format!(
                            "model '{}' effort native_default must be included in supported levels",
                            spec.id
                        ));
                    }
                    NativeDefault::Known(*level)
                }
                None => NativeDefault::Unknown,
            };
            Ok(EffortCapabilities::supported(levels, native_default))
        }
    }
}

fn external_model_spec_from_config(
    index: usize,
    spec: ExternalModelSpec,
) -> Result<ModelSpec, String> {
    let id = spec.id.trim().to_string();
    if id.is_empty() {
        return Err(format!("model at index {index} has an empty id"));
    }
    let api_name = spec
        .api_name
        .as_ref()
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
    if let Some(max_output_tokens) = spec.max_output_tokens {
        if max_output_tokens == 0
            || usize::try_from(max_output_tokens).unwrap_or(usize::MAX) >= spec.context_window
        {
            return Err(format!(
                "model '{id}' has invalid max_output_tokens {max_output_tokens} for context_window {}",
                spec.context_window
            ));
        }
    }
    if spec
        .effort_capabilities
        .as_ref()
        .is_some_and(|capabilities| {
            matches!(capabilities, ExternalEffortCapabilities::Supported { levels, .. }
            if levels.iter().any(|level| level.needs_extended_output_headroom()))
        })
        && spec.max_output_tokens.is_none_or(|limit| limit < 64_000)
    {
        return Err(format!(
            "model '{id}' declares xhigh/max effort but max_output_tokens is below 64000"
        ));
    }
    if spec
        .effort_capabilities
        .as_ref()
        .is_some_and(|capabilities| {
            matches!(capabilities, ExternalEffortCapabilities::Supported { levels, .. }
            if levels.iter().any(|level| level.needs_extended_output_headroom()))
        })
        && spec.context_window <= 64_000 + 4_096
    {
        return Err(format!(
            "model '{id}' declares xhigh/max effort but context_window must exceed 68096"
        ));
    }
    let effort_capabilities = validate_external_effort_capabilities(&spec)?;
    Ok(ModelSpec {
        id,
        api_name,
        backend: spec.backend.into(),
        description,
        context_window: spec.context_window,
        max_output_tokens: spec.max_output_tokens,
        recommended: spec.recommended,
        supports_tool_search: spec.supports_tool_search,
        source: ModelSource::External,
        effort_capabilities,
    })
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
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_anthropic_xhigh(),
        },
        ModelSpec {
            id: "claude-opus-4-7".into(),
            api_name: "claude-opus-4-7".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Opus 4.7 (legacy)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: false,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_anthropic_xhigh(),
        },
        ModelSpec {
            id: "claude-opus-4-6".into(),
            api_name: "claude-opus-4-6".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Opus 4.6 (legacy)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: false,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_anthropic_base(),
        },
        ModelSpec {
            id: "claude-sonnet-5".into(),
            api_name: "claude-sonnet-5".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Sonnet 5 (balanced performance)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_anthropic_xhigh(),
        },
        ModelSpec {
            id: "claude-sonnet-4-6".into(),
            api_name: "claude-sonnet-4-6".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Sonnet 4.6 (legacy)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: false,
            supports_tool_search: true,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_anthropic_base(),
        },
        ModelSpec {
            id: "claude-haiku-4-5".into(),
            api_name: "claude-haiku-4-5-20251001".into(),
            backend: ModelBackend::Anthropic,
            description: "Claude Haiku 4.5 (fast, efficient)".into(),
            context_window: 200_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: EffortCapabilities::unsupported(),
        },
        // OpenAI models
        // Context windows here are the platform-API ceilings. The codex bridge
        // caps every built-in OpenAI Responses model at 272K regardless — that
        // override is applied at registration time in `registry.rs` when the
        // bridge path is selected, so this spec's value reaches the runtime
        // only for direct/provider-compatible routes.
        ModelSpec {
            id: "gpt-5.6-sol".into(),
            api_name: "gpt-5.6-sol".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.6 Sol (frontier, 1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_gpt_55_plus(),
        },
        ModelSpec {
            id: "gpt-5.6-luna".into(),
            api_name: "gpt-5.6-luna".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.6 Luna (frontier, 1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_gpt_55_plus(),
        },
        ModelSpec {
            id: "gpt-5.6-terra".into(),
            api_name: "gpt-5.6-terra".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.6 Terra (frontier, 1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_gpt_55_plus(),
        },
        ModelSpec {
            id: "gpt-5.5".into(),
            api_name: "gpt-5.5".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.5 (frontier, 1M context)".into(),
            context_window: 1_000_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_gpt_55_plus(),
        },
        ModelSpec {
            id: "gpt-5.4".into(),
            api_name: "gpt-5.4".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.4 (frontier, native computer use)".into(),
            context_window: 400_000,
            max_output_tokens: None,
            recommended: false,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_gpt_54(),
        },
        ModelSpec {
            id: "gpt-5.4-mini".into(),
            api_name: "gpt-5.4-mini".into(),
            backend: ModelBackend::OpenAIResponses,
            description: "GPT-5.4 Mini (fast, efficient)".into(),
            context_window: 400_000,
            max_output_tokens: None,
            recommended: true,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: effort_gpt_54(),
        },
        // Mock model for frontend development without API keys
        ModelSpec {
            id: "mock".into(),
            api_name: "mock".into(),
            backend: ModelBackend::Mock,
            description: "Mock (lorem ipsum for UI dev)".into(),
            context_window: 200_000,
            max_output_tokens: None,
            recommended: false,
            supports_tool_search: false,
            source: ModelSource::BuiltIn,
            effort_capabilities: EffortCapabilities::unknown(),
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
    fn built_in_effort_capabilities_match_model_contracts() {
        let models = all_models();
        let by_id = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .unwrap_or_else(|| panic!("missing built-in model {id}"))
        };

        assert_eq!(
            by_id("claude-sonnet-5").effort_capabilities,
            effort_anthropic_xhigh()
        );
        assert_eq!(
            by_id("claude-sonnet-4-6").effort_capabilities,
            effort_anthropic_base()
        );
        assert_eq!(
            by_id("claude-haiku-4-5").effort_capabilities,
            EffortCapabilities::Unsupported
        );
        assert_eq!(
            by_id("gpt-5.6-sol").effort_capabilities,
            effort_gpt_55_plus()
        );
        assert_eq!(by_id("gpt-5.4-mini").effort_capabilities, effort_gpt_54());
        assert!(by_id("gpt-5.6-sol")
            .effort_capabilities
            .supports(ModelEffort::Max));
        assert!(!by_id("gpt-5.4-mini")
            .effort_capabilities
            .supports(ModelEffort::Max));
        assert!(models.iter().all(|model| model.id != "gpt-5.3-codex"));
    }

    #[test]
    fn external_effort_metadata_distinguishes_unknown_unsupported_and_supported() {
        let models = parse_external_models(
            r#"[
                {"id":"absent","backend":"openai_responses","description":"Absent metadata","context_window":128000,"recommended":false,"supports_tool_search":false},
                {"id":"unsupported","backend":"anthropic","description":"Unsupported","context_window":128000,"recommended":false,"supports_tool_search":false,"effort_capabilities":{"support":"unsupported"}},
                {"id":"known-levels","backend":"openai_responses","description":"Known levels","context_window":128000,"recommended":false,"supports_tool_search":false,"effort_capabilities":{"support":"supported","levels":["low","high"]}}
            ]"#,
        )
        .expect("external models parse");

        assert_eq!(models[0].effort_capabilities, EffortCapabilities::Unknown);
        assert_eq!(
            models[1].effort_capabilities,
            EffortCapabilities::Unsupported
        );
        assert_eq!(
            models[2].effort_capabilities,
            EffortCapabilities::supported_unknown(&[ModelEffort::Low, ModelEffort::High])
        );
    }

    #[test]
    fn extended_external_effort_requires_output_capacity() {
        let models = parse_external_models(
            r#"[{"id":"small","backend":"openai_responses","description":"Small","context_window":50000,"max_output_tokens":50000,"recommended":false,"supports_tool_search":false,"effort_capabilities":{"support":"supported","levels":["xhigh"]}}]"#,
        )
        .expect("top-level external model array parses");
        assert!(models.is_empty());

        let models = parse_external_models(
            r#"[{"id":"large","backend":"openai_responses","description":"Large","context_window":128000,"max_output_tokens":64000,"recommended":false,"supports_tool_search":false,"effort_capabilities":{"support":"supported","levels":["xhigh"]}}]"#,
        )
        .expect("top-level external model array parses");
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn anthropic_external_models_reject_non_native_effort_levels() {
        let models = parse_external_models(
            r#"[{"id":"bad-anthropic","backend":"anthropic","description":"Bad","context_window":128000,"recommended":false,"supports_tool_search":false,"effort_capabilities":{"support":"supported","levels":["none","high"]}}]"#,
        )
        .expect("top-level external model array parses");
        assert!(models.is_empty());
    }

    #[test]
    fn external_native_default_must_be_supported() {
        let models = parse_external_models(
            r#"[{"id":"bad","backend":"openai_responses","description":"Bad metadata","context_window":128000,"recommended":false,"supports_tool_search":false,"effort_capabilities":{"support":"supported","levels":["low"],"native_default":"high"}}]"#,
        )
        .expect("top-level external model array parses");

        assert!(models.is_empty());
    }

    #[test]
    fn skips_unknown_external_backend_entry() {
        let models = parse_external_models(
            r#"[{"id":"bad-openai","backend":"not-a-backend","description":"Bad backend","context_window":128000,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect("array syntax should parse even when an entry is invalid");

        assert!(models.is_empty());
    }

    #[test]
    fn skips_invalid_external_context_window_entry() {
        let models = parse_external_models(
            r#"[{"id":"bad","backend":"anthropic","description":"Bad","context_window":0,"recommended":false,"supports_tool_search":false}]"#,
        )
        .expect("array syntax should parse even when an entry is invalid");

        assert!(models.is_empty());
    }

    #[test]
    fn bad_external_models_json_is_rejected() {
        let err = parse_external_models("not json")
            .expect_err("top-level invalid JSON should still reject the config");

        assert_eq!(err, "invalid JSON for PHOENIX_LLM_MODELS");
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
