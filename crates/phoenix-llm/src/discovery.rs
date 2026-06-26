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
    /// URL for `OpenAI` models endpoint
    pub openai_models_url: Option<String>,
    /// Auth headers to send to the Anthropic models endpoint.
    pub anthropic_auth_headers: Vec<(String, String)>,
    /// Auth headers to send to the `OpenAI` models endpoint.
    pub openai_auth_headers: Vec<(String, String)>,
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

#[derive(Debug, Default)]
pub struct DiscoveredModels {
    pub anthropic_listed: bool,
    pub anthropic: HashSet<String>,
    pub openai_responses_listed: bool,
    pub openai_responses: HashSet<String>,
}

fn empty_ids() -> &'static HashSet<String> {
    static EMPTY: OnceLock<HashSet<String>> = OnceLock::new();
    EMPTY.get_or_init(HashSet::new)
}

impl DiscoveredModels {
    #[must_use]
    pub fn any_listed(&self) -> bool {
        self.anthropic_listed || self.openai_responses_listed
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anthropic.is_empty() && self.openai_responses.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.anthropic.len() + self.openai_responses.len()
    }

    #[must_use]
    pub fn was_listed(&self, backend: ModelBackend) -> bool {
        match backend {
            ModelBackend::Anthropic => self.anthropic_listed,
            ModelBackend::OpenAIResponses => self.openai_responses_listed,
            // Chat Completions models bypass discovery filtering; they always
            // appear as configured regardless of what the gateway listing returns.
            ModelBackend::OpenAIChatCompletions | ModelBackend::Mock => false,
        }
    }

    #[must_use]
    pub fn ids_for_backend(&self, backend: ModelBackend) -> &HashSet<String> {
        match backend {
            ModelBackend::Anthropic => &self.anthropic,
            ModelBackend::OpenAIResponses => &self.openai_responses,
            // Chat Completions has no dedicated discovery bucket.
            ModelBackend::OpenAIChatCompletions | ModelBackend::Mock => empty_ids(),
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

    if let Some(ref url) = config.openai_models_url {
        match discover_provider(
            url,
            "openai",
            config.openai_auth_headers.as_slice(),
            &config.custom_headers,
            &[],
        )
        .await
        {
            Ok(m) => {
                models.openai_responses_listed = true;
                models.openai_responses.extend(m);
            }
            Err(e) => tracing::warn!(provider = "openai", error = %e, "Discovery failed"),
        }
    }

    models
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
