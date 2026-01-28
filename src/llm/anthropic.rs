//! Anthropic Claude provider implementation

use super::{LlmError, LlmErrorKind, LlmService};
use super::types::*;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Anthropic model variants
#[derive(Debug, Clone, Copy)]
pub enum AnthropicModel {
    Claude4Opus,
    Claude4Sonnet,
    Claude35Sonnet,
    Claude35Haiku,
}

impl AnthropicModel {
    pub fn api_name(&self) -> &'static str {
        match self {
            AnthropicModel::Claude4Opus => "claude-sonnet-4-20250514",
            AnthropicModel::Claude4Sonnet => "claude-sonnet-4-20250514",
            AnthropicModel::Claude35Sonnet => "claude-3-5-sonnet-20241022",
            AnthropicModel::Claude35Haiku => "claude-3-5-haiku-20241022",
        }
    }

    pub fn context_window(&self) -> usize {
        match self {
            AnthropicModel::Claude4Opus => 200_000,
            AnthropicModel::Claude4Sonnet => 200_000,
            AnthropicModel::Claude35Sonnet => 200_000,
            AnthropicModel::Claude35Haiku => 200_000,
        }
    }

    pub fn model_id(&self) -> &'static str {
        match self {
            AnthropicModel::Claude4Opus => "claude-4-opus",
            AnthropicModel::Claude4Sonnet => "claude-4-sonnet",
            AnthropicModel::Claude35Sonnet => "claude-3.5-sonnet",
            AnthropicModel::Claude35Haiku => "claude-3.5-haiku",
        }
    }
}

/// Anthropic service implementation
pub struct AnthropicService {
    client: Client,
    api_key: String,
    model: AnthropicModel,
    base_url: String,
    model_id: String,
}

impl AnthropicService {
    pub fn new(api_key: String, model: AnthropicModel, gateway: Option<&str>) -> Self {
        let base_url = match gateway {
            Some(gw) => format!("{}/_/gateway/anthropic/v1/messages", gw.trim_end_matches('/')),
            None => "https://api.anthropic.com/v1/messages".to_string(),
        };
        
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("Failed to create HTTP client");
        
        Self {
            client,
            api_key,
            model,
            base_url,
            model_id: model.model_id().to_string(),
        }
    }

    fn translate_request(&self, request: &LlmRequest) -> AnthropicRequest {
        let system: Vec<AnthropicSystemBlock> = request.system
            .iter()
            .map(|s| AnthropicSystemBlock {
                r#type: "text".to_string(),
                text: s.text.clone(),
                cache_control: if s.cache {
                    Some(CacheControl { r#type: "ephemeral".to_string() })
                } else {
                    None
                },
            })
            .collect();

        let messages: Vec<AnthropicMessage> = request.messages
            .iter()
            .map(|m| self.translate_message(m))
            .collect();

        let tools: Vec<AnthropicTool> = request.tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        AnthropicRequest {
            model: self.model.api_name().to_string(),
            max_tokens: request.max_tokens.unwrap_or(8192),
            system,
            messages,
            tools: if tools.is_empty() { None } else { Some(tools) },
        }
    }

    fn translate_message(&self, msg: &LlmMessage) -> AnthropicMessage {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        let content: Vec<AnthropicContentBlock> = msg.content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => AnthropicContentBlock::Text {
                    text: text.clone(),
                },
                ContentBlock::Image { source } => {
                    let ImageSource::Base64 { media_type, data } = source;
                    AnthropicContentBlock::Image {
                        source: AnthropicImageSource {
                            r#type: "base64".to_string(),
                            media_type: media_type.clone(),
                            data: data.clone(),
                        },
                    }
                }
                ContentBlock::ToolUse { id, name, input } => AnthropicContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
                ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                    AnthropicContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: content.clone(),
                        is_error: *is_error,
                    }
                }
            })
            .collect();

        AnthropicMessage {
            role: role.to_string(),
            content,
        }
    }

    fn normalize_response(&self, resp: AnthropicResponse) -> LlmResponse {
        let content: Vec<ContentBlock> = resp.content
            .into_iter()
            .map(|block| match block {
                AnthropicContentBlock::Text { text } => ContentBlock::Text { text },
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    ContentBlock::ToolUse { id, name, input }
                }
                AnthropicContentBlock::Image { .. } => {
                    // Images shouldn't appear in responses
                    ContentBlock::Text { text: "[image]".to_string() }
                }
                AnthropicContentBlock::ToolResult { .. } => {
                    // Tool results shouldn't appear in responses
                    ContentBlock::Text { text: "[tool result]".to_string() }
                }
            })
            .collect();

        let end_turn = resp.stop_reason.as_deref() == Some("end_turn");

        LlmResponse {
            content,
            end_turn,
            usage: Usage {
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
                cache_creation_tokens: resp.usage.cache_creation_input_tokens.unwrap_or(0),
                cache_read_tokens: resp.usage.cache_read_input_tokens.unwrap_or(0),
            },
        }
    }

    fn classify_error(&self, status: reqwest::StatusCode, body: &str) -> LlmError {
        let message = body.to_string();
        match status.as_u16() {
            401 | 403 => LlmError::auth(format!("Authentication failed: {}", message)),
            429 => {
                let mut err = LlmError::rate_limit(format!("Rate limited: {}", message));
                // Try to parse retry-after from response
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
                    if let Some(retry_after) = parsed.get("error")
                        .and_then(|e| e.get("retry_after"))
                        .and_then(|r| r.as_f64())
                    {
                        err = err.with_retry_after(Duration::from_secs_f64(retry_after));
                    }
                }
                err
            }
            400 => LlmError::invalid_request(format!("Invalid request: {}", message)),
            500..=599 => LlmError::server_error(format!("Server error: {}", message)),
            _ => LlmError::unknown(format!("HTTP {}: {}", status, message)),
        }
    }
}

#[async_trait]
impl LlmService for AnthropicService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let anthropic_request = self.translate_request(request);
        
        let response = self.client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&anthropic_request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::network(format!("Request timeout: {}", e))
                } else if e.is_connect() {
                    LlmError::network(format!("Connection failed: {}", e))
                } else {
                    LlmError::unknown(format!("Request failed: {}", e))
                }
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            LlmError::network(format!("Failed to read response: {}", e))
        })?;

        if !status.is_success() {
            return Err(self.classify_error(status, &body));
        }

        let anthropic_response: AnthropicResponse = serde_json::from_str(&body)
            .map_err(|e| LlmError::unknown(format!("Failed to parse response: {} - body: {}", e, body)))?;

        Ok(self.normalize_response(anthropic_response))
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn context_window(&self) -> usize {
        self.model.context_window()
    }

    fn max_image_dimension(&self) -> Option<u32> {
        Some(1568) // Anthropic's max image dimension
    }
}

// Anthropic API types

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: Vec<AnthropicSystemBlock>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
}

#[derive(Debug, Serialize)]
struct AnthropicSystemBlock {
    r#type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct CacheControl {
    r#type: String,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text { text: String },
    Image { source: AnthropicImageSource },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicImageSource {
    r#type: String,
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}
