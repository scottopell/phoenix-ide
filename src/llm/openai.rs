//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::types::{ContentBlock, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage};
use super::{LlmError, LlmService};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// `OpenAI`-compatible models (`OpenAI` and Fireworks)
#[derive(Debug, Clone, Copy)]
pub enum OpenAIModel {
    // OpenAI GPT-4 models
    GPT4o,
    GPT4oMini,
    O4Mini,
    // OpenAI GPT-5 models (chat endpoint)
    GPT5,
    GPT5Mini,
    GPT51,
    // OpenAI GPT-5 Codex models (responses endpoint)
    GPT5Codex,
    GPT51Codex,
    GPT52Codex,
    // Fireworks models (use OpenAI API)
    GLM4P7Fireworks,
    QwenCoderFireworks,
    DeepseekV3Fireworks,
}

impl OpenAIModel {
    pub fn api_name(self) -> &'static str {
        match self {
            OpenAIModel::GPT4o => "gpt-4o",
            OpenAIModel::GPT4oMini => "gpt-4o-mini",
            OpenAIModel::O4Mini => "o4-mini",
            OpenAIModel::GPT5 => "gpt-5",
            OpenAIModel::GPT5Mini => "gpt-5-mini",
            OpenAIModel::GPT51 => "gpt-5.1",
            OpenAIModel::GPT5Codex => "gpt-5-codex",
            OpenAIModel::GPT51Codex => "gpt-5.1-codex",
            OpenAIModel::GPT52Codex => "gpt-5.2-codex",
            OpenAIModel::GLM4P7Fireworks => "accounts/fireworks/models/glm-4p7",
            OpenAIModel::QwenCoderFireworks => {
                "accounts/fireworks/models/qwen3-coder-480b-a35b-instruct"
            }
            OpenAIModel::DeepseekV3Fireworks => "accounts/fireworks/models/deepseek-v3p1",
        }
    }

    pub fn model_id(self) -> &'static str {
        match self {
            OpenAIModel::GPT4o => "gpt-4o",
            OpenAIModel::GPT4oMini => "gpt-4o-mini",
            OpenAIModel::O4Mini => "o4-mini",
            OpenAIModel::GPT5 => "gpt-5",
            OpenAIModel::GPT5Mini => "gpt-5-mini",
            OpenAIModel::GPT51 => "gpt-5.1",
            OpenAIModel::GPT5Codex => "gpt-5-codex",
            OpenAIModel::GPT51Codex => "gpt-5.1-codex",
            OpenAIModel::GPT52Codex => "gpt-5.2-codex",
            OpenAIModel::GLM4P7Fireworks => "glm-4p7-fireworks",
            OpenAIModel::QwenCoderFireworks => "qwen3-coder-fireworks",
            OpenAIModel::DeepseekV3Fireworks => "deepseek-v3-fireworks",
        }
    }

    pub fn is_fireworks(self) -> bool {
        matches!(
            self,
            OpenAIModel::GLM4P7Fireworks
                | OpenAIModel::QwenCoderFireworks
                | OpenAIModel::DeepseekV3Fireworks
        )
    }

    /// Models that use `max_completion_tokens` instead of `max_tokens`
    pub fn uses_max_completion_tokens(self) -> bool {
        matches!(
            self,
            OpenAIModel::O4Mini | OpenAIModel::GPT5 | OpenAIModel::GPT5Mini | OpenAIModel::GPT51
        )
    }

    /// Codex models use the v1/responses endpoint instead of chat/completions
    pub fn uses_responses_api(self) -> bool {
        matches!(
            self,
            OpenAIModel::GPT5Codex | OpenAIModel::GPT51Codex | OpenAIModel::GPT52Codex
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
        let base_url = match (gateway, model.is_fireworks(), model.uses_responses_api()) {
            (Some(gw), true, _) => {
                // Fireworks via gateway
                format!(
                    "{}/fireworks/inference/v1/chat/completions",
                    gw.trim_end_matches('/')
                )
            }
            (Some(gw), false, true) => {
                // OpenAI responses API via gateway (for codex models)
                format!("{}/openai/v1/responses", gw.trim_end_matches('/'))
            }
            (Some(gw), false, false) => {
                // OpenAI chat API via gateway
                format!("{}/openai/v1/chat/completions", gw.trim_end_matches('/'))
            }
            (None, true, _) => {
                // Direct Fireworks
                "https://api.fireworks.ai/inference/v1/chat/completions".to_string()
            }
            (None, false, true) => {
                // Direct OpenAI responses API
                "https://api.openai.com/v1/responses".to_string()
            }
            (None, false, false) => {
                // Direct OpenAI chat API
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
            // translate_message may return multiple messages (e.g., tool results need separate messages)
            messages.extend(Self::translate_message(msg));
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

        // O-series models use max_completion_tokens, others use max_tokens
        let (max_tokens, max_completion_tokens) = if self.model.uses_max_completion_tokens() {
            (None, request.max_tokens)
        } else {
            (request.max_tokens, None)
        };

        OpenAIRequest {
            model: self.model.api_name().to_string(),
            messages,
            tools,
            max_tokens,
            max_completion_tokens,
            temperature: None,
            stream: false,
        }
    }

    /// Translate an LLM message to `OpenAI` format.
    /// Returns a Vec because tool results need separate messages with role "tool".
    fn translate_message(msg: &LlmMessage) -> Vec<OpenAIMessage> {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        // Separate content into categories
        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut tool_results = Vec::new();

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    text_parts.push(text.clone());
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(OpenAIToolCall {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: OpenAIFunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(input)
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    });
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    tool_results.push((tool_use_id.clone(), content.clone(), *is_error));
                }
                ContentBlock::Image { .. } => {
                    // Images not yet supported
                    tracing::warn!("Images not supported in OpenAI Chat Completions translation");
                }
            }
        }

        let mut messages = Vec::new();

        // Build message based on what content we have
        if !text_parts.is_empty() || !tool_calls.is_empty() {
            let content = if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            };

            let tool_calls_opt = if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            };

            messages.push(OpenAIMessage {
                role: role.to_string(),
                content,
                tool_calls: tool_calls_opt,
                tool_call_id: None,
            });
        }

        // Tool results are separate messages with role "tool"
        for (tool_use_id, content, is_error) in tool_results {
            messages.push(OpenAIMessage {
                role: "tool".to_string(),
                content: Some(if is_error {
                    format!("Error: {content}")
                } else {
                    content
                }),
                tool_calls: None,
                tool_call_id: Some(tool_use_id),
            });
        }

        // Edge case: empty message (shouldn't happen, but handle gracefully)
        if messages.is_empty() {
            messages.push(OpenAIMessage {
                role: role.to_string(),
                content: Some(String::new()),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        messages
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
                input_tokens: u64::from(resp.usage.prompt_tokens),
                output_tokens: u64::from(resp.usage.completion_tokens),
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }
}

#[async_trait]
#[async_trait]
impl LlmService for OpenAIService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        // Route to appropriate API based on model type
        if self.model.uses_responses_api() {
            self.complete_responses_api(request).await
        } else {
            self.complete_chat_api(request).await
        }
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn context_window(&self) -> usize {
        match self.model {
            OpenAIModel::O4Mini
            | OpenAIModel::GPT5Codex
            | OpenAIModel::GPT51Codex
            | OpenAIModel::GPT52Codex => 200_000,
            OpenAIModel::GPT4o
            | OpenAIModel::GPT4oMini
            | OpenAIModel::GPT5
            | OpenAIModel::GPT5Mini
            | OpenAIModel::GPT51
            | OpenAIModel::GLM4P7Fireworks
            | OpenAIModel::QwenCoderFireworks
            | OpenAIModel::DeepseekV3Fireworks => 128_000,
        }
    }

    fn max_image_dimension(&self) -> Option<u32> {
        None // Basic implementation doesn't support images
    }
}

impl OpenAIService {
    /// Complete using the chat/completions API
    async fn complete_chat_api(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
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
            // Parse error response
            if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(&body) {
                let message = error_resp.error.message;
                return Err(match status.as_u16() {
                    401 => LlmError::auth(format!("Authentication failed: {message}")),
                    429 => LlmError::rate_limit(format!("Rate limit exceeded: {message}")),
                    400 => LlmError::invalid_request(format!("Invalid request: {message}")),
                    500..=599 => LlmError::server_error(format!("Server error: {message}")),
                    _ => LlmError::unknown(format!("HTTP {status}: {message}")),
                });
            }
            return Err(LlmError::unknown(format!("HTTP {status} error: {body}")));
        }

        let openai_response: OpenAIResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::unknown(format!("Failed to parse response: {e} - body: {body}"))
        })?;

        Self::normalize_response(openai_response)
    }

    /// Complete using the v1/responses API (for codex models)
    async fn complete_responses_api(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let responses_request = self.translate_to_responses_request(request);

        let response = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&responses_request)
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
            if let Ok(error_resp) = serde_json::from_str::<OpenAIErrorResponse>(&body) {
                let message = error_resp.error.message;
                return Err(match status.as_u16() {
                    401 => LlmError::auth(format!("Authentication failed: {message}")),
                    429 => LlmError::rate_limit(format!("Rate limit exceeded: {message}")),
                    400 => LlmError::invalid_request(format!("Invalid request: {message}")),
                    500..=599 => LlmError::server_error(format!("Server error: {message}")),
                    _ => LlmError::unknown(format!("HTTP {status}: {message}")),
                });
            }
            return Err(LlmError::unknown(format!("HTTP {status} error: {body}")));
        }

        let responses_response: ResponsesApiResponse =
            serde_json::from_str(&body).map_err(|e| {
                LlmError::unknown(format!("Failed to parse response: {e} - body: {body}"))
            })?;

        Ok(Self::normalize_responses_api_response(responses_response))
    }

    /// Translate `LlmRequest` to `ResponsesApiRequest`
    fn translate_to_responses_request(&self, request: &LlmRequest) -> ResponsesApiRequest {
        // Build input as array of conversation items
        let mut input_items = Vec::new();

        // Add system prompt as instructions
        let instructions = if request.system.is_empty() {
            None
        } else {
            Some(
                request
                    .system
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        };

        // Process messages into conversation items
        for msg in &request.messages {
            let role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
            };

            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => {
                        input_items.push(ResponsesApiInputItem::Message {
                            role: role.to_string(),
                            content: text.clone(),
                        });
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        // Assistant's function call - include it for context
                        input_items.push(ResponsesApiInputItem::FunctionCall {
                            call_id: id.clone(),
                            name: name.clone(),
                            arguments: serde_json::to_string(input)
                                .unwrap_or_else(|_| "{}".to_string()),
                        });
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // Tool result - provide the output
                        let output = if *is_error {
                            format!("Error: {content}")
                        } else {
                            content.clone()
                        };
                        input_items.push(ResponsesApiInputItem::FunctionCallOutput {
                            call_id: tool_use_id.clone(),
                            output,
                        });
                    }
                    ContentBlock::Image { .. } => {
                        // Images not supported yet in Responses API
                        tracing::warn!("Images not supported in Responses API");
                    }
                }
            }
        }

        // Convert tools to responses API format
        let tools: Option<Vec<ResponsesApiTool>> = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| ResponsesApiTool {
                        r#type: "function".to_string(),
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.input_schema.clone(),
                    })
                    .collect(),
            )
        };

        ResponsesApiRequest {
            model: self.model.api_name().to_string(),
            input: input_items,
            instructions,
            tools,
            max_output_tokens: request.max_tokens,
        }
    }

    /// Normalize `ResponsesApiResponse` to `LlmResponse`
    fn normalize_responses_api_response(resp: ResponsesApiResponse) -> LlmResponse {
        let mut content = Vec::new();

        // Process all outputs
        for output in resp.output {
            match output.r#type.as_str() {
                "message" => {
                    // Extract text content from message outputs
                    if let Some(output_content) = output.content {
                        for item in output_content {
                            if item.r#type == "output_text" {
                                if let Some(text) = item.text {
                                    if !text.is_empty() {
                                        content.push(ContentBlock::Text { text });
                                    }
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    // Extract tool use from function_call outputs
                    if let (Some(name), Some(arguments), Some(call_id)) =
                        (output.name, output.arguments, output.call_id)
                    {
                        // Parse arguments JSON
                        let input = serde_json::from_str(&arguments).unwrap_or_else(|e| {
                            tracing::warn!(error = %e, arguments = %arguments, "Failed to parse function call arguments");
                            serde_json::Value::Object(serde_json::Map::new())
                        });
                        content.push(ContentBlock::ToolUse {
                            id: call_id,
                            name,
                            input,
                        });
                    }
                }
                "reasoning" => {
                    // Skip reasoning outputs - they're internal model thinking
                }
                other => {
                    tracing::debug!(output_type = %other, "Ignoring unknown output type");
                }
            }
        }

        // Determine end_turn: if there are tool calls, the model wants to continue
        let has_tool_calls = content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        let end_turn = resp.status == "completed" && !has_tool_calls;

        LlmResponse {
            content,
            end_turn,
            usage: Usage {
                input_tokens: u64::from(resp.usage.input_tokens),
                output_tokens: u64::from(resp.usage.output_tokens),
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        }
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
    max_completion_tokens: Option<u32>,
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
#[allow(clippy::struct_field_names)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    #[allow(dead_code)] // Part of API response, not always used
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

// Responses API types (for codex models)

#[derive(Debug, Serialize)]
struct ResponsesApiRequest {
    model: String,
    /// Input can be a string or array of conversation items
    input: Vec<ResponsesApiInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

/// Input item for the Responses API conversation
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ResponsesApiInputItem {
    /// User or assistant message
    #[serde(rename = "message")]
    Message { role: String, content: String },
    /// Function call from assistant (echoed back for context)
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Output from a function call
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

#[derive(Debug, Serialize)]
struct ResponsesApiTool {
    r#type: String,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ResponsesApiResponse {
    status: String,
    output: Vec<ResponsesApiOutput>,
    usage: ResponsesApiUsage,
}

#[derive(Debug, Deserialize)]
struct ResponsesApiOutput {
    r#type: String,
    /// For message outputs
    #[serde(default)]
    content: Option<Vec<ResponsesApiContent>>,
    /// For `function_call` outputs
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesApiContent {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}
