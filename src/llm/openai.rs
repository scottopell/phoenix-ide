//! `OpenAI` and `OpenAI`-compatible provider implementation

use super::models::{ModelSpec, Provider};
use super::types::{ContentBlock, LlmMessage, LlmRequest, LlmResponse, MessageRole, Usage};
use super::LlmError;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Complete using OpenAI-compatible API (`OpenAI` or Fireworks)
pub async fn complete(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    if uses_responses_api(&spec.api_name) {
        complete_responses_api(spec, api_key, gateway, request).await
    } else {
        complete_chat_api(spec, api_key, gateway, request).await
    }
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Determine the full endpoint URL based on provider, model, and gateway.
fn resolve_endpoint(spec: &ModelSpec, gateway: Option<&str>) -> String {
    let is_fireworks = spec.provider == Provider::Fireworks;

    match (gateway, is_fireworks) {
        // Fireworks via gateway
        (Some(gw), true) => format!(
            "{}/fireworks/inference/v1/chat/completions",
            gw.trim_end_matches('/')
        ),
        // OpenAI via gateway — always responses endpoint
        (Some(gw), false) => format!("{}/openai/v1/responses", gw.trim_end_matches('/')),
        // Direct Fireworks
        (None, true) => "https://api.fireworks.ai/inference/v1/chat/completions".to_string(),
        // Direct OpenAI — always responses endpoint
        (None, false) => "https://api.openai.com/v1/responses".to_string(),
    }
}

/// All non-Fireworks models use the v1/responses endpoint.
fn uses_responses_api(api_name: &str) -> bool {
    !is_fireworks_model(api_name)
}

/// Fireworks models are identified by their api_name prefix.
fn is_fireworks_model(api_name: &str) -> bool {
    api_name.starts_with("accounts/fireworks/")
}

/// Models that use `max_completion_tokens` instead of `max_tokens`.
fn uses_max_completion_tokens(api_name: &str) -> bool {
    matches!(api_name, "o4-mini" | "gpt-5" | "gpt-5-mini" | "gpt-5.1")
}

// ---------------------------------------------------------------------------
// Chat Completions API
// ---------------------------------------------------------------------------

/// Complete using the chat/completions API.
async fn complete_chat_api(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_endpoint(spec, gateway);
    let openai_request = translate_request(&spec.api_name, request);

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LlmError::unknown(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
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

    let openai_response: OpenAIResponse = serde_json::from_str(&body)
        .map_err(|e| LlmError::unknown(format!("Failed to parse response: {e} - body: {body}")))?;

    normalize_response(openai_response)
}

// ---------------------------------------------------------------------------
// Responses API
// ---------------------------------------------------------------------------

/// Complete using the v1/responses API (all non-Fireworks models).
async fn complete_responses_api(
    spec: &ModelSpec,
    api_key: &str,
    gateway: Option<&str>,
    request: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = resolve_endpoint(spec, gateway);
    let responses_request = translate_to_responses_request(&spec.api_name, request);

    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| LlmError::unknown(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
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

    let responses_response: ResponsesApiResponse = serde_json::from_str(&body)
        .map_err(|e| LlmError::unknown(format!("Failed to parse response: {e} - body: {body}")))?;

    Ok(normalize_responses_api_response(responses_response))
}

// ---------------------------------------------------------------------------
// Request translation
// ---------------------------------------------------------------------------

/// Translate an `LlmRequest` into an `OpenAIRequest` for chat/completions.
fn translate_request(api_name: &str, request: &LlmRequest) -> OpenAIRequest {
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
            content: Some(OpenAIContent::Text(system_text)),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    // Add conversation messages
    for msg in &request.messages {
        messages.extend(translate_message(msg));
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
                        description: Some(t.description.clone()),
                        parameters: t.input_schema.clone(),
                    },
                })
                .collect(),
        )
    };

    // O-series / GPT-5 models use max_completion_tokens, others use max_tokens
    let (max_tokens, max_completion_tokens) = if uses_max_completion_tokens(api_name) {
        (None, request.max_tokens)
    } else {
        (request.max_tokens, None)
    };

    OpenAIRequest {
        model: api_name.to_string(),
        messages,
        tools,
        max_tokens,
        max_completion_tokens,
        temperature: None,
        top_p: None,
        n: None,
        stream: Some(false),
        stop: None,
        presence_penalty: None,
        frequency_penalty: None,
        logit_bias: None,
        user: None,
    }
}

/// Translate an LLM message to `OpenAI` format.
/// Returns a Vec because tool results need separate messages with role "tool".
pub(crate) fn translate_message(msg: &LlmMessage) -> Vec<OpenAIMessage> {
    use super::types::ImageSource;

    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

    // Separate content into categories
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            ContentBlock::Image { source } => {
                let ImageSource::Base64 { media_type, data } = source;
                images.push((media_type.clone(), data.clone()));
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
                images: _,
                is_error,
            } => {
                tool_results.push((tool_use_id.clone(), content.clone(), *is_error));
            }
        }
    }

    let mut messages = Vec::new();

    // Build message based on what content we have
    if !text_parts.is_empty() || !images.is_empty() || !tool_calls.is_empty() {
        let content = if images.is_empty() && !text_parts.is_empty() {
            // Simple text-only message
            Some(OpenAIContent::Text(text_parts.join("\n")))
        } else if !images.is_empty() {
            // Vision message with images (must use parts format)
            let mut parts = Vec::new();

            // Add text parts
            for text in text_parts {
                parts.push(OpenAIContentPart::Text { text });
            }

            // Add images
            for (media_type, data) in images {
                parts.push(OpenAIContentPart::ImageUrl {
                    image_url: OpenAIImageUrl {
                        url: format!("data:{media_type};base64,{data}"),
                    },
                });
            }

            Some(OpenAIContent::Parts(parts))
        } else {
            // No text or images (tool calls only)
            None
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
            content: Some(OpenAIContent::Text(if is_error {
                format!("Error: {content}")
            } else {
                content
            })),
            tool_calls: None,
            tool_call_id: Some(tool_use_id),
        });
    }

    // Edge case: empty message (shouldn't happen, but handle gracefully)
    if messages.is_empty() {
        messages.push(OpenAIMessage {
            role: role.to_string(),
            content: Some(OpenAIContent::Text(String::new())),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    messages
}

/// Translate `LlmRequest` to `ResponsesApiRequest`.
fn translate_to_responses_request(api_name: &str, request: &LlmRequest) -> ResponsesApiRequest {
    use super::types::ImageSource;

    let mut input_items = Vec::new();

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

    // Process each message as a unit to allow grouping text + images
    for msg in &request.messages {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };

        let mut text_blocks: Vec<&str> = vec![];
        let mut image_blocks: Vec<&ImageSource> = vec![];
        let mut tool_calls: Vec<&ContentBlock> = vec![];
        let mut tool_results: Vec<&ContentBlock> = vec![];

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => text_blocks.push(text),
                ContentBlock::Image { source } => image_blocks.push(source),
                ContentBlock::ToolUse { .. } => tool_calls.push(block),
                ContentBlock::ToolResult { .. } => tool_results.push(block),
            }
        }

        // Emit single Message item for text + image content
        if !text_blocks.is_empty() || !image_blocks.is_empty() {
            let content = if image_blocks.is_empty() {
                ResponsesApiMessageContent::Text(text_blocks.join("\n"))
            } else {
                let mut parts: Vec<ResponsesApiContentPart> = text_blocks
                    .iter()
                    .map(|t| ResponsesApiContentPart::InputText {
                        text: (*t).to_string(),
                    })
                    .collect();
                for source in &image_blocks {
                    let ImageSource::Base64 { media_type, data } = source;
                    parts.push(ResponsesApiContentPart::InputImage {
                        image_url: format!("data:{media_type};base64,{data}"),
                    });
                }
                ResponsesApiMessageContent::Parts(parts)
            };
            input_items.push(ResponsesApiInputItem::Message {
                role: role.to_string(),
                content,
            });
        }

        // Emit FunctionCall items
        for block in tool_calls {
            if let ContentBlock::ToolUse { id, name, input } = block {
                input_items.push(ResponsesApiInputItem::FunctionCall {
                    call_id: id.clone(),
                    name: name.clone(),
                    arguments: serde_json::to_string(input)
                        .unwrap_or_else(|_| "{}".to_string()),
                });
            }
        }

        // Emit FunctionCallOutput items with image support
        for block in tool_results {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                images,
                is_error,
            } = block
            {
                let text = if *is_error {
                    format!("Error: {content}")
                } else {
                    content.clone()
                };
                let output = if images.is_empty() {
                    ResponsesApiFunctionOutput::Text(text)
                } else {
                    let mut parts = vec![ResponsesApiOutputPart::Text { text }];
                    for img in images {
                        let ImageSource::Base64 { media_type, data } = img;
                        parts.push(ResponsesApiOutputPart::ImageUrl {
                            image_url: ResponsesApiImageUrl {
                                url: format!("data:{media_type};base64,{data}"),
                            },
                        });
                    }
                    ResponsesApiFunctionOutput::Parts(parts)
                };
                input_items.push(ResponsesApiInputItem::FunctionCallOutput {
                    call_id: tool_use_id.clone(),
                    output,
                });
            }
        }
    }

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
        model: api_name.to_string(),
        input: input_items,
        instructions,
        tools,
        max_output_tokens: request.max_tokens,
    }
}

// ---------------------------------------------------------------------------
// Response normalization
// ---------------------------------------------------------------------------

/// Normalize an `OpenAIResponse` (chat/completions) to `LlmResponse`.
pub(crate) fn normalize_response(resp: OpenAIResponse) -> Result<LlmResponse, LlmError> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::unknown("No choices in response"))?;

    let mut content = Vec::new();

    // Add text content if present
    if let Some(msg_content) = choice.message.content {
        match msg_content {
            OpenAIContent::Text(text) => {
                if !text.is_empty() {
                    content.push(ContentBlock::Text { text });
                }
            }
            OpenAIContent::Parts(parts) => {
                for part in parts {
                    if let OpenAIContentPart::Text { text } = part {
                        if !text.is_empty() {
                            content.push(ContentBlock::Text { text });
                        }
                    }
                }
            }
        }
    }

    // Add tool calls if present
    if let Some(tool_calls) = choice.message.tool_calls {
        for tc in tool_calls {
            if tc.function.name.is_empty() {
                return Err(LlmError::unknown(
                    "OpenAI returned tool call with empty function name",
                ));
            }

            let input = serde_json::from_str(&tc.function.arguments).map_err(|e| {
                LlmError::unknown(format!("Invalid JSON in tool call arguments: {e}"))
            })?;

            content.push(ContentBlock::ToolUse {
                id: tc.id,
                name: tc.function.name,
                input,
            });
        }
    }

    if content.is_empty() {
        return Err(LlmError::unknown(
            "OpenAI returned empty response (no content or tool calls)".to_string(),
        ));
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

/// Normalize `ResponsesApiResponse` to `LlmResponse`.
fn normalize_responses_api_response(resp: ResponsesApiResponse) -> LlmResponse {
    let mut content = Vec::new();

    for output in resp.output {
        match output.r#type.as_str() {
            "message" => {
                if let Some(output_content) = output.content {
                    for item in output_content {
                        if item.r#type == "output_text" {
                            if let Some(text) = item.text {
                                if !text.is_empty() {
                                    content.push(ContentBlock::Text { text });                                }
                            }
                        }
                    }
                }
            }
            "function_call" => {
                if let (Some(name), Some(arguments), Some(call_id)) =
                    (output.name, output.arguments, output.call_id)
                {
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
                // Skip reasoning outputs — internal model thinking
            }
            other => {
                tracing::debug!(output_type = %other, "Ignoring unknown output type");
            }
        }
    }

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

// ===========================================================================
// OpenAI API types
// ===========================================================================

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<std::collections::HashMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OpenAIContentPart {
    Text { text: String },
    ImageUrl { image_url: OpenAIImageUrl },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIImageUrl {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAITool {
    pub r#type: String,
    pub function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIResponse {
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIChoice {
    pub message: OpenAIMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[allow(dead_code)] // Part of API response, not always used
    pub total_tokens: u32,
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
pub(crate) struct ResponsesApiRequest {
    model: String,
    pub(crate) input: Vec<ResponsesApiInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(crate) enum ResponsesApiInputItem {
    #[serde(rename = "message")]
    Message { role: String, content: ResponsesApiMessageContent },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        call_id: String,
        output: ResponsesApiFunctionOutput,
    },
}

/// Message content: plain string when text-only, array of parts when images present
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesApiMessageContent {
    Text(String),
    Parts(Vec<ResponsesApiContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesApiContentPart {
    InputText { text: String },
    InputImage { image_url: String }, // "data:{media_type};base64,{data}"
}

/// Function call output: plain string when text-only, array of parts when images present
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum ResponsesApiFunctionOutput {
    Text(String),
    Parts(Vec<ResponsesApiOutputPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ResponsesApiOutputPart {
    Text { text: String },
    ImageUrl { image_url: ResponsesApiImageUrl },
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesApiImageUrl {
    pub(crate) url: String, // "data:{media_type};base64,{data}"
}

#[derive(Debug, Serialize)]
struct ResponsesApiTool {
    r#type: String,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiResponse {
    pub(crate) status: String,
    pub(crate) output: Vec<ResponsesApiOutput>,
    pub(crate) usage: ResponsesApiUsage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiOutput {
    pub(crate) r#type: String,
    #[serde(default)]
    pub(crate) content: Option<Vec<ResponsesApiContent>>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) arguments: Option<String>,
    #[serde(default)]
    pub(crate) call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiContent {
    pub(crate) r#type: String,
    #[serde(default)]
    pub(crate) text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResponsesApiUsage {
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    pub(crate) fn normalize_response(
        resp: OpenAIResponse,
    ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
        super::normalize_response(resp)
    }

    pub(crate) fn translate_message(msg: &crate::llm::types::LlmMessage) -> Vec<OpenAIMessage> {
        super::translate_message(msg)
    }

    pub fn translate_to_responses_request(
        api_name: &str,
        request: &crate::llm::types::LlmRequest,
    ) -> ResponsesApiRequest {
        super::translate_to_responses_request(api_name, request)
    }
}
