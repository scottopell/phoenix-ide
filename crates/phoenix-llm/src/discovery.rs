//! Dynamic model discovery from provider-compatible model listing endpoints.
//!
//! Queries `/v1/models` endpoints derived from configured base URLs to validate
//! which configured models are available.

use serde::Deserialize;
use std::collections::HashSet;

/// Configuration for model discovery
pub struct DiscoveryConfig {
    /// URL for Anthropic models endpoint
    pub anthropic_models_url: Option<String>,
    /// URL for `OpenAI` models endpoint
    pub openai_models_url: Option<String>,
    /// Auth token to send as Authorization: Bearer (if any)
    pub auth_token: Option<String>,
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

/// Discover available model IDs from configured model-listing endpoints.
///
/// Returns a set of model IDs that the endpoints report as available.
pub async fn discover_models(config: &DiscoveryConfig) -> HashSet<String> {
    let mut models = HashSet::new();

    if let Some(ref url) = config.anthropic_models_url {
        match discover_provider(
            url,
            "anthropic",
            config.auth_token.as_deref(),
            &config.custom_headers,
            &[("anthropic-version", "2023-06-01")],
        )
        .await
        {
            Ok(m) => models.extend(m),
            Err(e) => tracing::warn!(provider = "anthropic", error = %e, "Discovery failed"),
        }
    }

    if let Some(ref url) = config.openai_models_url {
        match discover_provider(
            url,
            "openai",
            config.auth_token.as_deref(),
            &config.custom_headers,
            &[],
        )
        .await
        {
            Ok(m) => models.extend(m),
            Err(e) => tracing::warn!(provider = "openai", error = %e, "Discovery failed"),
        }
    }

    models
}

/// Discover model IDs from a single provider endpoint.
async fn discover_provider(
    url: &str,
    provider_name: &str,
    auth_token: Option<&str>,
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
    if let Some(token) = auth_token {
        request = request.header("Authorization", format!("Bearer {token}"));
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
