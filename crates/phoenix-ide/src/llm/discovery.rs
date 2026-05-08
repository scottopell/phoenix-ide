//! Dynamic model discovery from LLM gateway
//!
//! Queries a single un-headered `/v1/models` endpoint to discover available
//! models at runtime, validating which hardcoded models the gateway proxies.

use serde::Deserialize;
use std::collections::HashSet;

/// Configuration for model discovery
pub struct DiscoveryConfig {
    /// URL for the gateway's `/v1/models` endpoint
    pub models_url: String,
    /// Auth token to send as Authorization: Bearer (if any)
    pub auth_token: Option<String>,
    /// Custom headers to inject on the discovery request
    pub custom_headers: Vec<(String, String)>,
}

/// `/v1/models` response — works for both Anthropic and `OpenAI`-compatible gateways.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelData>,
}

#[derive(Debug, Deserialize)]
struct ModelData {
    id: String,
}

/// Probe gateway reachability with a lightweight HEAD/GET request.
///
/// Returns `true` if the gateway responds with any HTTP status (even an error),
/// meaning the host is up and listening. Returns `false` on network/timeout errors.
pub async fn probe_gateway(
    gateway_url: &str,
    auth_token: Option<&str>,
    custom_headers: &[(String, String)],
) -> bool {
    let url = format!("{}/_proxy/status", gateway_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let mut request = client.get(&url).timeout(std::time::Duration::from_secs(3));

    if let Some(token) = auth_token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    for (key, value) in custom_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    match request.send().await {
        Ok(_) => {
            tracing::debug!(url = %url, "Gateway probe succeeded");
            true
        }
        Err(err) => {
            tracing::debug!(url = %url, error = %err, "Gateway probe failed");
            false
        }
    }
}

/// Discover available model IDs from the LLM gateway.
///
/// Returns a set of model IDs that the gateway reports as available.
/// Used to validate which hardcoded models are actually reachable.
pub async fn discover_models(config: &DiscoveryConfig) -> HashSet<String> {
    let client = reqwest::Client::new();
    let mut request = client
        .get(&config.models_url)
        .timeout(std::time::Duration::from_secs(5));

    if let Some(ref token) = config.auth_token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    for (key, value) in &config.custom_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let response = match request.send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(error = %e, "Model discovery request failed");
            return HashSet::new();
        }
    };

    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "Models endpoint returned non-success");
        return HashSet::new();
    }

    let parsed: ModelsResponse = match response.json().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse models response");
            return HashSet::new();
        }
    };

    let ids: HashSet<String> = parsed.data.into_iter().map(|m| m.id).collect();
    tracing::info!("Discovered {} models from gateway", ids.len());
    ids
}
