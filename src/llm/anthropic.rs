//! Anthropic Claude provider implementation

use super::models::ModelSpec;
use super::types::{
    ContentBlock, ImageSource, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage,
};
use super::LlmError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Complete using Anthropic Messages API
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let base_url = match gateway {
        Some(gw) => format!("{}/anthropic/v1/messages", gw.trim_end_matches('/')),
        None => "https://api.anthropic.com/v1/messages".to_string(),
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LlmError::unknown(format!("Failed to create HTTP client: {e}")))?;

    let anthropic_request = translate_request(&spec.api_name, request);

    let response = client
        .post(&base_url)
        .header("x-api-key", api_key)
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
        return Err(LlmError::from_http_status(status.as_u16(), &body));
    }

    let anthropic_response: AnthropicResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::invalid_response(format!("Failed to parse response: {e} - body: {body}"))
    })?;

    normalize_response(anthropic_response)
}

fn translate_request(model_api_name: &str, request: &LlmRequest) -> AnthropicRequest {
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
        .map(translate_message)
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
        model: model_api_name.to_string(),
        max_tokens: request.max_tokens.unwrap_or(8192),
        system,
        messages,
        tools: if tools.is_empty() { None } else { Some(tools) },
    }
}

pub(crate) fn translate_message(msg: &LlmMessage) -> AnthropicMessage {
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

pub(crate) fn normalize_response(resp: AnthropicResponse) -> Result<LlmResponse, LlmError> {
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
                return Err(LlmError::invalid_response(
                    "Unexpected image block in Anthropic response",
                ));
            }
            AnthropicContentBlock::ToolResult { .. } => {
                return Err(LlmError::invalid_response(
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
        return Err(LlmError::invalid_response(format!(
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
    
    pub(crate) fn normalize_response(
        resp: AnthropicResponse,
    ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
        super::normalize_response(resp)
    }
    
    pub(crate) fn translate_message(msg: &crate::llm::types::LlmMessage) -> AnthropicMessage {
        super::translate_message(msg)
    }
}
