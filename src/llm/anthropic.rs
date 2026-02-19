//! Anthropic Claude provider implementation

use super::types::{
    ContentBlock, ImageSource, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage,
};
use super::{LlmError, LlmService};
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
    pub fn api_name(self) -> &'static str {
        match self {
            // Use model names from exe.dev/shelley
            AnthropicModel::Claude4Opus => "claude-opus-4-5-20251101",
            AnthropicModel::Claude4Sonnet => "claude-sonnet-4-5-20250929",
            AnthropicModel::Claude35Sonnet => "claude-sonnet-4-20250514",
            AnthropicModel::Claude35Haiku => "claude-haiku-4-5-20251001",
        }
    }

    #[allow(dead_code)] // For future context management
    pub fn context_window(self) -> usize {
        match self {
            AnthropicModel::Claude4Opus
            | AnthropicModel::Claude4Sonnet
            | AnthropicModel::Claude35Sonnet
            | AnthropicModel::Claude35Haiku => 200_000,
        }
    }

    pub fn model_id(self) -> &'static str {
        match self {
            AnthropicModel::Claude4Opus => "claude-4.5-opus",
            AnthropicModel::Claude4Sonnet => "claude-4.5-sonnet",
            AnthropicModel::Claude35Sonnet => "claude-3.5-sonnet",
            AnthropicModel::Claude35Haiku => "claude-4.5-haiku",
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
            Some(gw) => {
                // exe.dev gateway format: gateway_base + /anthropic/v1/messages
                format!("{}/anthropic/v1/messages", gw.trim_end_matches('/'))
            }
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
        let system: Vec<AnthropicSystemBlock> = request
            .system
            .iter()
            .map(|s| AnthropicSystemBlock {
                r#type: "text".to_string(),
                text: s.text.clone(),
                cache_control: if s.cache {
                    Some(CacheControl {
                        r#type: "ephemeral".to_string(),
                    })
                } else {
                    None
                },
            })
            .collect();

        let messages: Vec<AnthropicMessage> = request
            .messages
            .iter()
            .map(Self::translate_message)
            .collect();

        let tools: Vec<AnthropicTool> = request
            .tools
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

    fn translate_message(msg: &LlmMessage) -> AnthropicMessage {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        let content: Vec<AnthropicContentBlock> = msg
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => AnthropicContentBlock::Text { text: text.clone() },
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
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => AnthropicContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                },
            })
            .collect();

        AnthropicMessage {
            role: role.to_string(),
            content,
        }
    }

    fn normalize_response(resp: AnthropicResponse) -> Result<LlmResponse, LlmError> {
        let mut content = Vec::new();
        let raw_block_count = resp.content.len();

        for block in resp.content {
            match block {
                AnthropicContentBlock::Text { text } => {
                    if !text.is_empty() {
                        content.push(ContentBlock::Text { text });
                    }
                }
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    content.push(ContentBlock::ToolUse { id, name, input });
                }
                AnthropicContentBlock::Image { .. } => {
                    return Err(LlmError::unknown(
                        "Unexpected image block in Anthropic response",
                    ));
                }
                AnthropicContentBlock::ToolResult { .. } => {
                    return Err(LlmError::unknown(
                        "Unexpected tool_result block in Anthropic response",
                    ));
                }
            }
        }

        let end_turn = resp.stop_reason.as_deref() == Some("end_turn");

        if content.is_empty() {
            // Log exactly what Anthropic sent so we can diagnose the root cause.
            // output_tokens > 0 with empty content suggests blocks were silently dropped
            // (e.g. empty text blocks filtered above). stop_reason tells us intent.
            tracing::warn!(
                stop_reason = ?resp.stop_reason,
                output_tokens = resp.usage.output_tokens,
                raw_block_count = raw_block_count,
                "Anthropic returned empty content after normalization"
            );
            return Err(LlmError::unknown(format!(
                "Anthropic returned empty response (no content or tool calls, stop_reason={:?}, output_tokens={}, raw_blocks={})",
                resp.stop_reason, resp.usage.output_tokens, raw_block_count
            )));
        }

        Ok(LlmResponse {
            content,
            end_turn,
            usage: Usage {
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
                cache_creation_tokens: resp.usage.cache_creation_input_tokens.unwrap_or(0),
                cache_read_tokens: resp.usage.cache_read_input_tokens.unwrap_or(0),
            },
        })
    }

    fn classify_error(status: reqwest::StatusCode, body: &str) -> LlmError {
        let message = body.to_string();
        match status.as_u16() {
            401 | 403 => LlmError::auth(format!("Authentication failed: {message}")),
            429 => {
                let mut err = LlmError::rate_limit(format!("Rate limited: {message}"));
                // Try to parse retry-after from response
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
                    if let Some(retry_after) = parsed
                        .get("error")
                        .and_then(|e| e.get("retry_after"))
                        .and_then(serde_json::Value::as_f64)
                    {
                        err = err.with_retry_after(Duration::from_secs_f64(retry_after));
                    }
                }
                err
            }
            400 => LlmError::invalid_request(format!("Invalid request: {message}")),
            500..=599 => LlmError::server_error(format!("Server error: {message}")),
            _ => LlmError::unknown(format!("HTTP {status}: {message}")),
        }
    }
}

#[async_trait]
impl LlmService for AnthropicService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let anthropic_request = self.translate_request(request);

        let response = self
            .client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&anthropic_request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::network(format!("Request timeout: {e}"))
                } else if e.is_connect() {
                    LlmError::network(format!("Connection failed: {e}"))
                } else {
                    LlmError::unknown(format!("Request failed: {e}"))
                }
            })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::network(format!("Failed to read response: {e}")))?;

        if !status.is_success() {
            return Err(Self::classify_error(status, &body));
        }

        let anthropic_response: AnthropicResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::unknown(format!("Failed to parse response: {e} - body: {body}"))
        })?;

        Self::normalize_response(anthropic_response)
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
pub(crate) struct AnthropicMessage {
    pub(crate) role: String,
    pub(crate) content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
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
pub(crate) struct AnthropicImageSource {
    pub(crate) r#type: String,
    pub(crate) media_type: String,
    pub(crate) data: String,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicResponse {
    pub(crate) content: Vec<AnthropicContentBlock>,
    pub(crate) stop_reason: Option<String>,
    pub(crate) usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // matches Anthropic API
pub(crate) struct AnthropicUsage {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_creation_input_tokens: Option<u64>,
    pub(crate) cache_read_input_tokens: Option<u64>,
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use crate::llm::types::LlmMessage;

    pub fn translate_message(msg: &LlmMessage) -> AnthropicMessage {
        AnthropicService::translate_message(msg)
    }

    pub fn normalize_response(
        resp: AnthropicResponse,
    ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
        AnthropicService::normalize_response(resp)
    }
}
