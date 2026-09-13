//! Dynamic model discovery from provider-compatible model listing endpoints.
//!
//! Queries `/v1/models` endpoints derived from configured base URLs to validate
//! which configured models are available.

use crate::ModelBackend;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::OnceLock;

/// Configuration for model discovery
pub struct DiscoveryConfig {
    /// URL for Anthropic models endpoint
    pub anthropic_models_url: Option<String>,
    /// URL for the `OpenAI` Responses models endpoint.
    pub openai_responses_models_url: Option<String>,
    /// URL for the `OpenAI` Chat Completions models endpoint.
    pub openai_chat_completions_models_url: Option<String>,
    /// Auth headers to send to the Anthropic models endpoint.
    pub anthropic_auth_headers: Vec<(String, String)>,
    /// Auth headers to send to the `OpenAI` Responses models endpoint.
    pub openai_responses_auth_headers: Vec<(String, String)>,
    /// Auth headers to send to the `OpenAI` Chat Completions models endpoint.
    pub openai_chat_completions_auth_headers: Vec<(String, String)>,
    /// Custom headers to inject on discovery requests
    pub custom_headers: Vec<(String, String)>,
}

/// `/v1/models` response — works for both Anthropic and `OpenAI`.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelData>,
}

#[derive(Debug, Deserialize)]
struct ModelData {
    id: String,
}

#[derive(Debug, Deserialize)]
struct CodexModelsResponse {
    models: Vec<CodexModelData>,
}

#[derive(Debug, Deserialize)]
struct CodexModelData {
    slug: String,
}

#[derive(Debug, Default)]
pub struct DiscoveredModels {
    pub anthropic_listed: bool,
    pub anthropic: HashSet<String>,
    pub openai_responses_listed: bool,
    pub openai_responses: HashSet<String>,
    pub openai_chat_completions_listed: bool,
    pub openai_chat_completions: HashSet<String>,
}

fn empty_ids() -> &'static HashSet<String> {
    static EMPTY: OnceLock<HashSet<String>> = OnceLock::new();
    EMPTY.get_or_init(HashSet::new)
}

impl DiscoveredModels {
    #[must_use]
    pub fn any_listed(&self) -> bool {
        self.anthropic_listed || self.openai_responses_listed || self.openai_chat_completions_listed
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anthropic.is_empty()
            && self.openai_responses.is_empty()
            && self.openai_chat_completions.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.anthropic.len() + self.openai_responses.len() + self.openai_chat_completions.len()
    }

    #[must_use]
    pub fn was_listed(&self, backend: ModelBackend) -> bool {
        match backend {
            ModelBackend::Anthropic => self.anthropic_listed,
            ModelBackend::OpenAIResponses => self.openai_responses_listed,
            ModelBackend::OpenAIChatCompletions => self.openai_chat_completions_listed,
            ModelBackend::Mock => false,
        }
    }

    #[must_use]
    pub fn ids_for_backend(&self, backend: ModelBackend) -> &HashSet<String> {
        match backend {
            ModelBackend::Anthropic => &self.anthropic,
            ModelBackend::OpenAIResponses => &self.openai_responses,
            ModelBackend::OpenAIChatCompletions => &self.openai_chat_completions,
            ModelBackend::Mock => empty_ids(),
        }
    }
}

/// Discover available model IDs from configured model-listing endpoints.
///
/// Returns backend-scoped model IDs that the endpoints report as available.
pub async fn discover_models(config: &DiscoveryConfig) -> DiscoveredModels {
    let mut models = DiscoveredModels::default();

    if let Some(ref url) = config.anthropic_models_url {
        match discover_provider(
            url,
            "anthropic",
            config.anthropic_auth_headers.as_slice(),
            &config.custom_headers,
            &[("anthropic-version", "2023-06-01")],
        )
        .await
        {
            Ok(m) => {
                models.anthropic_listed = true;
                models.anthropic.extend(m);
            }
            Err(e) => tracing::warn!(provider = "anthropic", error = %e, "Discovery failed"),
        }
    }

    if let Some(ref url) = config.openai_responses_models_url {
        match discover_provider(
            url,
            "openai",
            config.openai_responses_auth_headers.as_slice(),
            &config.custom_headers,
            &[],
        )
        .await
        {
            Ok(m) => {
                models.openai_responses_listed = true;
                models.openai_responses.extend(m);
            }
            Err(e) => {
                tracing::warn!(provider = "openai", backend = "responses", error = %e, "Discovery failed");
            }
        }
    }

    if let Some(ref url) = config.openai_chat_completions_models_url {
        match discover_provider(
            url,
            "openai",
            config.openai_chat_completions_auth_headers.as_slice(),
            &config.custom_headers,
            &[],
        )
        .await
        {
            Ok(m) => {
                models.openai_chat_completions_listed = true;
                models.openai_chat_completions.extend(m);
            }
            Err(e) => {
                tracing::warn!(provider = "openai", backend = "chat_completions", error = %e, "Discovery failed");
            }
        }
    }

    models
}

/// Discover the model catalog available to one ChatGPT/Codex account.
///
/// The Codex backend uses `{ "models": [{ "slug": ... }] }`, which is
/// intentionally distinct from the public `OpenAI` `{ "data": [{ "id": ... }] }`
/// response. `client_version` declares the minimum Codex catalog contract that
/// Phoenix implements; it is not Phoenix's application version.
///
/// # Errors
///
/// Returns an error when the request fails, the account is rejected, or the
/// response does not satisfy the Codex model-catalog schema.
pub async fn discover_codex_models(
    access_token: &str,
    account_id: Option<&str>,
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    const CODEX_CATALOG_CONTRACT_VERSION: &str = "0.153.0";
    let url = format!(
        "https://chatgpt.com/backend-api/codex/models?client_version={CODEX_CATALOG_CONTRACT_VERSION}"
    );
    let client = reqwest::Client::new();
    let mut request = client
        .get(url)
        .bearer_auth(access_token)
        .timeout(std::time::Duration::from_secs(5));
    if let Some(account_id) = account_id {
        request = request.header("chatgpt-account-id", account_id);
    }

    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(format!("Codex models endpoint returned {}", response.status()).into());
    }

    let models: CodexModelsResponse = response.json().await?;
    Ok(codex_model_slugs(models))
}

fn codex_model_slugs(models: CodexModelsResponse) -> HashSet<String> {
    models.models.into_iter().map(|model| model.slug).collect()
}

/// Discover model IDs from a single provider endpoint.
async fn discover_provider(
    url: &str,
    provider_name: &str,
    auth_headers: &[(String, String)],
    custom_headers: &[(String, String)],
    extra_headers: &[(&str, &str)],
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let mut request = client
        .get(url)
        .header("provider", provider_name)
        .timeout(std::time::Duration::from_secs(5));

    for &(key, value) in extra_headers {
        request = request.header(key, value);
    }
    for (key, value) in auth_headers {
        request = request.header(key.as_str(), value.as_str());
    }
    for (key, value) in custom_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        return Err(format!(
            "{provider_name} models endpoint returned {}",
            response.status()
        )
        .into());
    }

    let models_response: ModelsResponse = response.json().await?;
    let ids: HashSet<String> = models_response.data.into_iter().map(|m| m.id).collect();

    tracing::info!("Discovered {} {} models", ids.len(), provider_name);
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_catalog_uses_model_slugs() {
        let response: CodexModelsResponse = serde_json::from_value(serde_json::json!({
            "models": [
                { "slug": "gpt-6-astra", "display_name": "GPT-6 Astra" },
                { "slug": "gpt-5.6-sol", "display_name": "GPT-5.6 Sol" }
            ]
        }))
        .expect("Codex catalog fixture");

        assert_eq!(
            codex_model_slugs(response),
            HashSet::from(["gpt-6-astra".to_string(), "gpt-5.6-sol".to_string()])
        );
    }
}
