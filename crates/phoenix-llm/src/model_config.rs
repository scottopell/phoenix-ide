use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Deserialize;

use crate::models::{external_model_spec_from_config, ExternalModelSpec};
use crate::{parse_external_models, AuthStyle, ModelSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum UserAuthHeader {
    Bearer,
    ApiKey,
}

impl From<UserAuthHeader> for AuthStyle {
    fn from(value: UserAuthHeader) -> Self {
        match value {
            UserAuthHeader::Bearer => Self::PlainBearer,
            UserAuthHeader::ApiKey => Self::ApiKey,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserLlmConfig {
    pub api_key_helper: Option<String>,
    pub api_key_helper_ttl_ms: Option<u64>,
    pub auth_header: Option<UserAuthHeader>,
    pub default_model: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_responses_base_url: Option<String>,
    pub openai_chat_completions_base_url: Option<String>,
    #[serde(default)]
    pub custom_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub request_tags: BTreeMap<String, String>,
    pub request_models: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

#[derive(Debug, Default)]
pub(super) struct LoadedUserConfig {
    pub external_models: Vec<ModelSpec>,
    pub llm: Option<UserLlmConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelConfigFile {
    version: u32,
    llm: Option<UserLlmConfig>,
    models: Option<Vec<ExternalModelSpec>>,
    #[serde(rename = "agents")]
    _agents: Option<serde::de::IgnoredAny>,
    #[serde(rename = "tiers")]
    _tiers: Option<serde::de::IgnoredAny>,
}

fn validate_optional_nonblank(
    value: Option<String>,
    field: &str,
) -> Result<Option<String>, String> {
    value
        .map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err(format!("{field} must not be blank"))
            } else {
                Ok(trimmed.to_string())
            }
        })
        .transpose()
}

fn validate_llm(raw: UserLlmConfig) -> Result<UserLlmConfig, String> {
    if raw.api_key_helper_ttl_ms == Some(0) {
        return Err("llm.api_key_helper_ttl_ms must be greater than 0".to_string());
    }
    let mut custom_headers = BTreeMap::new();
    for (name, value) in raw.custom_headers {
        let name = name.trim();
        if name.is_empty() {
            return Err("llm.custom_headers keys must not be blank".to_string());
        }
        custom_headers.insert(name.to_string(), value.trim().to_string());
    }
    let mut request_tags = BTreeMap::new();
    for (key, value) in raw.request_tags {
        let key = key.trim();
        if key.is_empty() {
            return Err("llm.request_tags keys must not be blank".to_string());
        }
        request_tags.insert(key.to_string(), value.trim().to_string());
    }
    Ok(UserLlmConfig {
        api_key_helper: validate_optional_nonblank(raw.api_key_helper, "llm.api_key_helper")?,
        api_key_helper_ttl_ms: raw.api_key_helper_ttl_ms,
        auth_header: raw.auth_header,
        default_model: validate_optional_nonblank(raw.default_model, "llm.default_model")?,
        anthropic_base_url: validate_optional_nonblank(
            raw.anthropic_base_url,
            "llm.anthropic_base_url",
        )?,
        openai_base_url: validate_optional_nonblank(raw.openai_base_url, "llm.openai_base_url")?,
        openai_responses_base_url: validate_optional_nonblank(
            raw.openai_responses_base_url,
            "llm.openai_responses_base_url",
        )?,
        openai_chat_completions_base_url: validate_optional_nonblank(
            raw.openai_chat_completions_base_url,
            "llm.openai_chat_completions_base_url",
        )?,
        custom_headers,
        request_tags,
        request_models: raw.request_models,
    })
}

fn parse_user_config(
    content: &str,
) -> Result<(Option<Vec<ModelSpec>>, Option<UserLlmConfig>), String> {
    let config: ModelConfigFile = toml::from_str(content).map_err(|error: toml::de::Error| {
        let detail: String = error.message().chars().take(240).collect();
        format!("Invalid Phoenix config: {detail}")
    })?;
    if config.version != 1 {
        return Err(format!(
            "Unsupported Phoenix config version {}; expected 1",
            config.version
        ));
    }
    let llm = config.llm.map(validate_llm).transpose()?;
    let models = config
        .models
        .map(|definitions| {
            let mut ids = HashSet::new();
            definitions
                .into_iter()
                .enumerate()
                .map(|(index, definition)| {
                    let model = external_model_spec_from_config(index, definition)?;
                    if !ids.insert(model.id.clone()) {
                        return Err(format!("Duplicate external model id '{}'", model.id));
                    }
                    Ok(model)
                })
                .collect()
        })
        .transpose()?;
    Ok((models, llm))
}

#[cfg(test)]
fn parse_models(content: &str) -> Result<Option<Vec<ModelSpec>>, String> {
    parse_user_config(content).map(|(models, _)| models)
}

#[cfg(test)]
pub(super) fn load_external_models(
    path: &Path,
    environment_models: Option<&str>,
) -> Result<Vec<ModelSpec>, String> {
    load_user_config(path, environment_models).map(|loaded| loaded.external_models)
}

pub(super) fn load_user_config(
    path: &Path,
    environment_models: Option<&str>,
) -> Result<LoadedUserConfig, String> {
    let (models, llm) = match std::fs::read_to_string(path) {
        Ok(content) => parse_user_config(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((None, None)),
        Err(error) => Err(format!("Cannot read Phoenix config: {}", error.kind())),
    }
    .map_err(|error| format!("{}: {error}", path.display()))?;
    let environment_models = environment_models.filter(|raw| !raw.trim().is_empty());
    let external_models = match (models, environment_models) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{} defines models but PHOENIX_LLM_MODELS is also set; remove PHOENIX_LLM_MODELS to use TOML models",
                path.display()
            ))
        }
        (Some(models), None) => {
            tracing::info!(count = models.len(), path = %path.display(), "loaded TOML external models");
            models
        }
        (None, Some(raw)) => match parse_external_models(raw) {
            Ok(models) => {
                tracing::info!(count = models.len(), "loaded PHOENIX_LLM_MODELS additions");
                models
            }
            Err(error) => {
                tracing::warn!(error = %error, "invalid PHOENIX_LLM_MODELS; ignoring externally configured LLM models");
                Vec::new()
            }
        },
        (None, None) => Vec::new(),
    };
    if let Some(llm_cfg) = llm.as_ref() {
        tracing::info!(
            path = %path.display(),
            has_api_key_helper = llm_cfg.api_key_helper.is_some(),
            has_anthropic_base_url = llm_cfg.anthropic_base_url.is_some(),
            has_openai_base_url = llm_cfg.openai_base_url.is_some(),
            has_openai_responses_base_url = llm_cfg.openai_responses_base_url.is_some(),
            has_openai_chat_completions_base_url = llm_cfg.openai_chat_completions_base_url.is_some(),
            custom_header_count = llm_cfg.custom_headers.len(),
            request_tag_count = llm_cfg.request_tags.len(),
            "loaded TOML [llm] configuration"
        );
    }
    Ok(LoadedUserConfig {
        external_models,
        llm,
    })
}

#[cfg(test)]
mod tests;
