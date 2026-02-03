//! OpenAI and OpenAI-compatible provider implementation

use super::types::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage,
};
use super::{LlmError, LlmService};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// OpenAI-compatible models (OpenAI and Fireworks)
#[derive(Debug, Clone, Copy)]
pub enum OpenAIModel {
    // OpenAI models
    GPT52Codex,
    // Fireworks models (use OpenAI API)
    GLM47Fireworks,
    QwenCoderFireworks,
    GLM4P6Fireworks,
}

impl OpenAIModel {
    pub fn api_name(self) -> &'static str {
        match self {
            OpenAIModel::GPT52Codex => "gpt-5.2-codex",
            OpenAIModel::GLM47Fireworks => "accounts/fireworks/models/glm-4-7b-chat",
            OpenAIModel::QwenCoderFireworks => "accounts/fireworks/models/qwen3-coder-480b-instruct",
            OpenAIModel::GLM4P6Fireworks => "accounts/fireworks/models/glm-4p6-chat",
        }
    }

    pub fn model_id(self) -> &'static str {
        match self {
            OpenAIModel::GPT52Codex => "gpt-5.2-codex",
            OpenAIModel::GLM47Fireworks => "glm-4.7-fireworks",
            OpenAIModel::QwenCoderFireworks => "qwen3-coder-fireworks",
            OpenAIModel::GLM4P6Fireworks => "glm-4p6-fireworks",
        }
    }

    pub fn is_fireworks(self) -> bool {
        matches!(
            self,
            OpenAIModel::GLM47Fireworks | OpenAIModel::QwenCoderFireworks | OpenAIModel::GLM4P6Fireworks
        )
    }
}

/// OpenAI-compatible service implementation
pub struct OpenAIService {
    client: Client,
    api_key: String,
    model: OpenAIModel,
    base_url: String,
    model_id: String,
}

impl OpenAIService {
    pub fn new(api_key: String, model: OpenAIModel, gateway: Option<&str>) -> Self {
        let base_url = match (gateway, model.is_fireworks()) {
            (Some(gw), true) => {
                // Fireworks via gateway
                format!("{}/fireworks/inference/v1/chat/completions", gw.trim_end_matches('/'))
            }
            (Some(gw), false) => {
                // OpenAI via gateway
                format!("{}/openai/v1/chat/completions", gw.trim_end_matches('/'))
            }
            (None, true) => {
                // Direct Fireworks
                "https://api.fireworks.ai/inference/v1/chat/completions".to_string()
            }
            (None, false) => {
                // Direct OpenAI
                "https://api.openai.com/v1/chat/completions".to_string()
            }
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

    fn translate_request(&self, request: &LlmRequest) -> OpenAIRequest {
        let mut messages = Vec::new();

        // Add system messages first
        if !request.system.is_empty() {
            let system_text = request
                .system
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(system_text),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Add conversation messages
        for msg in &request.messages {
            messages.push(self.translate_message(msg));
        }

        // Convert tools
        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| OpenAITool {
                        r#type: "function".to_string(),
                        function: OpenAIFunction {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.input_schema.clone(),
                        },
                    })
                    .collect(),
            )
        };

        OpenAIRequest {
            model: self.model.api_name().to_string(),
            messages,
            tools,
            max_tokens: request.max_tokens,
            temperature: None,
            stream: false,
        }
    }

    fn translate_message(&self, msg: &LlmMessage) -> OpenAIMessage {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        // Handle content
        if msg.content.len() == 1 {
            // Single content block - use simple string format
            match &msg.content[0] {
                ContentBlock::Text { text } => OpenAIMessage {
                    role: role.to_string(),
                    content: Some(text.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                },
                ContentBlock::ToolUse { id, name, input } => {
                    // Convert tool use to tool_calls
                    OpenAIMessage {
                        role: role.to_string(),
                        content: None,
                        tool_calls: Some(vec![OpenAIToolCall {
                            id: id.clone(),
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: name.clone(),
                                arguments: serde_json::to_string(input)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            },
                        }]),
                        tool_call_id: None,
                    }
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // Tool results are sent as separate messages in OpenAI
                    OpenAIMessage {
                        role: "tool".to_string(),
                        content: Some(if *is_error {
                            format!("Error: {}", content)
                        } else {
                            content.clone()
                        }),
                        tool_calls: None,
                        tool_call_id: Some(tool_use_id.clone()),
                    }
                }
                _ => {
                    // Images not supported in basic implementation
                    OpenAIMessage {
                        role: role.to_string(),
                        content: Some("[unsupported content]".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    }
                }
            }
        } else {
            // Multiple content blocks - concatenate text
            let text_parts: Vec<String> = msg
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect();

            OpenAIMessage {
                role: role.to_string(),
                content: Some(text_parts.join("\n")),
                tool_calls: None,
                tool_call_id: None,
            }
        }
    }

    fn normalize_response(resp: OpenAIResponse) -> Result<LlmResponse, LlmError> {
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::unknown("No choices in response"))?;

        let mut content = Vec::new();

        // Add text content if present
        if let Some(text) = choice.message.content {
            if !text.is_empty() {
                content.push(ContentBlock::Text { text });
            }
        }

        // Add tool calls if present
        if let Some(tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                if tc.function.name.is_empty() {
                    continue;
                }
                
                let input = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or_else(|_| serde_json::json!({}));
                
                content.push(ContentBlock::ToolUse {
                    id: tc.id,
                    name: tc.function.name,
                    input,
                });
            }
        }

        let end_turn = choice.finish_reason == Some("stop".to_string());

        Ok(LlmResponse {
            content,
            end_turn,
            usage: Usage {
                input_tokens: resp.usage.prompt_tokens as u64,
                output_tokens: resp.usage.completion_tokens as u64,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }
}

#[async_trait]
impl LlmService for OpenAIService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let openai_request = self.translate_request(request);

        let response = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&openai_request)
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
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::network(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            // Parse error response
            if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(&body) {
                let message = error_resp.error.message;
                return Err(match status.as_u16() {
                    401 => LlmError::auth(format!("Authentication failed: {}", message)),
                    429 => LlmError::rate_limit(format!("Rate limit exceeded: {}", message)),
                    400 => LlmError::invalid_request(format!("Invalid request: {}", message)),
                    500..=599 => LlmError::server_error(format!("Server error: {}", message)),
                    _ => LlmError::unknown(format!("HTTP {}: {}", status, message)),
                });
            }
            return Err(LlmError::unknown(format!(
                "HTTP {} error: {}",
                status, body
            )));
        }

        let openai_response: OpenAIResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::unknown(format!("Failed to parse response: {} - body: {}", e, body))
        })?;

        Self::normalize_response(openai_response)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn context_window(&self) -> usize {
        match self.model {
            OpenAIModel::GPT52Codex => 128_000,
            OpenAIModel::GLM47Fireworks => 128_000,
            OpenAIModel::QwenCoderFireworks => 32_768,
            OpenAIModel::GLM4P6Fireworks => 128_000,
        }
    }

    fn max_image_dimension(&self) -> Option<u32> {
        None // Basic implementation doesn't support images
    }
}

// OpenAI API types

#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    r#type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: OpenAIUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAIErrorResponse {
    error: OpenAIError,
}

#[derive(Debug, Deserialize)]
struct OpenAIError {
    message: String,
    #[allow(dead_code)]
    r#type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}
