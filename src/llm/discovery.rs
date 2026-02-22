//! Dynamic model discovery from LLM gateway
//!
//! Queries gateway endpoints to discover available models at runtime,
//! merging with hardcoded metadata for context windows and descriptions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Discovered model information from gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub provider: String,
    pub display_name: Option<String>,
    pub context_length: Option<usize>,
    pub supports_chat: Option<bool>,
    pub supports_tools: Option<bool>,
}

impl DiscoveredModel {
    /// Convert discovered model to ModelSpec
    pub fn to_model_spec(&self) -> super::ModelSpec {
        use super::{ApiFormat, ModelSpec, Provider};
        
        // Infer provider from string
        let provider = match self.provider.as_str() {
            "Anthropic" => Provider::Anthropic,
            "OpenAI" => Provider::OpenAI,
            "Fireworks" => Provider::Fireworks,
            _ => {
                tracing::warn!("Unknown provider '{}', defaulting to OpenAI", self.provider);
                Provider::OpenAI
            }
        };
        
        // Infer API format from provider
        let api_format = match provider {
            Provider::Anthropic => ApiFormat::Anthropic,
            Provider::OpenAI | Provider::Fireworks => ApiFormat::OpenAIChat,
        };
        
        // Use display_name or id for description
        let description = self.display_name
            .clone()
            .unwrap_or_else(|| self.id.clone());
        
        // Default context window if not provided
        let context_window = self.context_length.unwrap_or(128_000);
        
        ModelSpec {
            id: self.id.clone(),
            api_name: self.id.clone(), // Gateway models use same name
            provider,
            api_format,
            description,
            context_window,
            recommended: false, // Discovered models not recommended by default
        }
    }
}

/// Anthropic /v1/models response
#[derive(Debug, Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModelData>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModelData {
    id: String,
    display_name: Option<String>,
}

/// `OpenAI` /v1/models response (also used by Fireworks)
#[derive(Debug, Deserialize)]
struct OpenAIModelsResponse {
    data: Vec<OpenAIModelData>,
}

#[derive(Debug, Deserialize)]
struct OpenAIModelData {
    id: String,
    #[serde(default)]
    context_length: Option<usize>,
    #[serde(default)]
    supports_chat: Option<bool>,
    #[serde(default)]
    supports_tools: Option<bool>,
}

/// Discover models from the LLM gateway
pub async fn discover_models(gateway_url: &str) -> HashMap<String, DiscoveredModel> {
    let mut models = HashMap::new();

    // Try each provider endpoint
    if let Ok(anthropic_models) = discover_anthropic(gateway_url).await {
        models.extend(anthropic_models);
    }

    if let Ok(openai_models) = discover_openai(gateway_url).await {
        models.extend(openai_models);
    }

    if let Ok(fireworks_models) = discover_fireworks(gateway_url).await {
        models.extend(fireworks_models);
    }

    models
}

/// Discover Anthropic models
async fn discover_anthropic(
    gateway_url: &str,
) -> Result<HashMap<String, DiscoveredModel>, Box<dyn std::error::Error>> {
    let url = format!("{}/anthropic/v1/models", gateway_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("anthropic-version", "2023-06-01")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("Anthropic models endpoint returned {}", response.status()).into());
    }

    let models_response: AnthropicModelsResponse = response.json().await?;

    let mut models = HashMap::new();
    for model in models_response.data {
        models.insert(
            model.id.clone(),
            DiscoveredModel {
                id: model.id,
                provider: "Anthropic".to_string(),
                display_name: model.display_name,
                context_length: None, // Anthropic doesn't provide this
                supports_chat: Some(true),
                supports_tools: Some(true),
            },
        );
    }

    tracing::info!("Discovered {} Anthropic models from gateway", models.len());
    Ok(models)
}

/// Discover `OpenAI` models
async fn discover_openai(
    gateway_url: &str,
) -> Result<HashMap<String, DiscoveredModel>, Box<dyn std::error::Error>> {
    let url = format!("{}/openai/v1/models", gateway_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("OpenAI models endpoint returned {}", response.status()).into());
    }

    let models_response: OpenAIModelsResponse = response.json().await?;

    let mut models = HashMap::new();
    for model in models_response.data {
        models.insert(
            model.id.clone(),
            DiscoveredModel {
                id: model.id,
                provider: "OpenAI".to_string(),
                display_name: None,
                context_length: model.context_length,
                supports_chat: model.supports_chat,
                supports_tools: model.supports_tools,
            },
        );
    }

    tracing::info!("Discovered {} OpenAI models from gateway", models.len());
    Ok(models)
}

/// Discover Fireworks models
async fn discover_fireworks(
    gateway_url: &str,
) -> Result<HashMap<String, DiscoveredModel>, Box<dyn std::error::Error>> {
    let url = format!(
        "{}/fireworks/inference/v1/models",
        gateway_url.trim_end_matches('/')
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("Fireworks models endpoint returned {}", response.status()).into());
    }

    let models_response: OpenAIModelsResponse = response.json().await?;

    let mut models = HashMap::new();
    for model in models_response.data {
        // Filter to chat models with tool support for Phoenix
        if model.supports_chat.unwrap_or(false) {
            models.insert(
                model.id.clone(),
                DiscoveredModel {
                    id: model.id,
                    provider: "Fireworks".to_string(),
                    display_name: None,
                    context_length: model.context_length,
                    supports_chat: model.supports_chat,
                    supports_tools: model.supports_tools,
                },
            );
        }
    }

    tracing::info!("Discovered {} Fireworks models from gateway", models.len());
    Ok(models)
}
