use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::models::{external_model_spec_from_config, ExternalModelSpec};
use crate::{parse_external_models, ModelSpec};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelConfigFile {
    version: u32,
    models: Option<Vec<ExternalModelSpec>>,
    #[serde(rename = "agents")]
    _agents: Option<serde::de::IgnoredAny>,
    #[serde(rename = "tiers")]
    _tiers: Option<serde::de::IgnoredAny>,
}

fn parse_models(content: &str) -> Result<Option<Vec<ModelSpec>>, String> {
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
    config
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
        .transpose()
}

pub(super) fn load_external_models(
    path: &Path,
    environment_models: Option<&str>,
) -> Result<Vec<ModelSpec>, String> {
    let models = match std::fs::read_to_string(path) {
        Ok(content) => parse_models(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read Phoenix config: {}", error.kind())),
    }
    .map_err(|error| format!("{}: {error}", path.display()))?;
    let environment_models = environment_models.filter(|raw| !raw.trim().is_empty());
    match (models, environment_models) {
        (Some(_), Some(_)) => Err(format!(
            "{} defines models but PHOENIX_LLM_MODELS is also set; remove PHOENIX_LLM_MODELS to use TOML models",
            path.display()
        )),
        (Some(models), None) => {
            tracing::info!(count = models.len(), path = %path.display(), "loaded TOML external models");
            Ok(models)
        }
        (None, Some(raw)) => match parse_external_models(raw) {
            Ok(models) => {
                tracing::info!(count = models.len(), "loaded PHOENIX_LLM_MODELS additions");
                Ok(models)
            }
            Err(error) => {
                tracing::warn!(error = %error, "invalid PHOENIX_LLM_MODELS; ignoring externally configured LLM models");
                Ok(Vec::new())
            }
        },
        (None, None) => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests;
