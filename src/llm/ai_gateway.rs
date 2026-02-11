//! Datadog AI Gateway integration.
//!
//! Provides unified access to multiple LLM providers (OpenAI, Anthropic, Gemini, self-hosted)
//! through Datadog's internal AI Gateway service.
//!
//! Authentication uses `ddtool` for service tokens. The gateway is OpenAI-compatible,
//! with provider routing via model name prefixes (e.g., "anthropic/claude-sonnet-4-20250514").

use crate::llm::openai::{
    OpenAIContent, OpenAIFunction, OpenAIFunctionCall, OpenAIMessage, OpenAIRequest,
    OpenAIResponse, OpenAITool, OpenAIToolCall,
};
use crate::llm::{ContentBlock, LlmError, LlmRequest, LlmResponse, LlmService, MessageRole, Usage};
use async_trait::async_trait;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN_TTL: Duration = Duration::from_secs(7200); // 2 hours

/// Get AI Gateway base URL from environment (required when AI Gateway is enabled)
fn get_base_url() -> String {
    std::env::var("AI_GATEWAY_URL")
        .expect("AI_GATEWAY_URL must be set when using AI Gateway mode")
}

/// Get ddtool datacenter from environment (required when AI Gateway is enabled)
fn get_datacenter() -> String {
    std::env::var("AI_GATEWAY_DATACENTER")
        .expect("AI_GATEWAY_DATACENTER must be set when using AI Gateway mode")
}

/// Get AI Gateway service name from environment (required when AI Gateway is enabled)
fn get_service_name() -> String {
    std::env::var("AI_GATEWAY_SERVICE")
        .expect("AI_GATEWAY_SERVICE must be set when using AI Gateway mode")
}

/// Token cache entry with expiry tracking.
#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Datadog AI Gateway service.
///
/// Routes requests to multiple LLM providers through a unified OpenAI-compatible API.
pub struct AIGatewayService {
    client: reqwest::Client,
    model: &'static str,
    provider_prefix: &'static str, // "anthropic", "openai", "gemini", etc.
    source: String,
    org_id: String,
    token_cache: Arc<Mutex<Option<CachedToken>>>,
}

impl AIGatewayService {
    /// Create a new AI Gateway service for the given model.
    ///
    /// # Arguments
    /// * `model` - Model ID (e.g., "claude-sonnet-4-20250514")
    /// * `api_name` - API-specific model name (used for requests)
    /// * `provider_prefix` - Provider prefix for routing ("anthropic", "openai", etc.)
    /// * `source` - Application identifier for telemetry (e.g., "phoenix-ide")
    /// * `org_id` - Datadog organization ID (use "2" for staging)
    pub fn new(
        _model: &'static str,
        api_name: &'static str,
        provider_prefix: &'static str,
        source: String,
        org_id: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::new(),
            model: api_name,
            provider_prefix,
            source,
            org_id,
            token_cache: Arc::new(Mutex::new(None)),
        })
    }

    /// Get a bearer token from ddtool, with caching.
    fn get_token(&self) -> Result<String, LlmError> {
        // Check cache first
        {
            let cache = self.token_cache.lock().unwrap();
            if let Some(cached) = cache.as_ref() {
                if Instant::now() < cached.expires_at {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Cache miss or expired - fetch new token
        tracing::debug!("Fetching new AI Gateway token from ddtool");

        let service_name = get_service_name();
        let datacenter = get_datacenter();

        let output = Command::new("ddtool")
            .args([
                "auth",
                "token",
                &service_name,
                "--datacenter",
                &datacenter,
            ])
            .output()
            .map_err(|e| {
                LlmError::network(format!("Failed to execute ddtool: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LlmError::auth(format!("ddtool failed: {}", stderr)));
        }

        let token = String::from_utf8(output.stdout)
            .map_err(|e| LlmError::network(format!("Invalid token UTF-8: {}", e)))?
            .trim()
            .to_string();

        if token.is_empty() {
            return Err(LlmError::auth("ddtool returned empty token".to_string()));
        }

        // Update cache
        {
            let mut cache = self.token_cache.lock().unwrap();
            *cache = Some(CachedToken {
                token: token.clone(),
                expires_at: Instant::now() + TOKEN_TTL,
            });
        }

        Ok(token)
    }

    /// Build the full model name with provider prefix.
    fn full_model_name(&self) -> String {
        format!("{}/{}", self.provider_prefix, self.model)
    }

    /// Translate our internal LlmRequest to OpenAI-compatible format.
    fn translate_request(&self, request: &LlmRequest) -> OpenAIRequest {
        let mut messages = Vec::new();

        // System prompts become first message with role "system"
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

        // Convert conversation messages
        for msg in &request.messages {
            messages.extend(Self::convert_message(msg));
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

        OpenAIRequest {
            model: self.full_model_name(),
            messages,
            tools,
            max_tokens: request.max_tokens,
            max_completion_tokens: None,
            temperature: None,
            top_p: None,
            n: None,
            stream: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            logit_bias: None,
            user: None,
        }
    }

    /// Convert a single message to OpenAI format.
    fn convert_message(msg: &crate::llm::LlmMessage) -> Vec<OpenAIMessage> {
        use crate::llm::openai::{OpenAIContent, OpenAIContentPart, OpenAIImageUrl};
        use crate::llm::types::ImageSource;

        let mut result = Vec::new();

        match msg.role {
            MessageRole::User => {
                // User messages: text content + images + tool results
                let mut text_parts = Vec::new();
                let mut images = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            text_parts.push(text.clone());
                        }
                        ContentBlock::Image { source } => {
                            let ImageSource::Base64 { media_type, data } = source;
                            images.push((media_type.clone(), data.clone()));
                        }
                        _ => {}
                    }
                }

                // Build content for user message
                if !text_parts.is_empty() || !images.is_empty() {
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
                                    url: format!("data:{};base64,{}", media_type, data),
                                },
                            });
                        }

                        Some(OpenAIContent::Parts(parts))
                    } else {
                        None
                    };

                    if content.is_some() {
                        result.push(OpenAIMessage {
                            role: "user".to_string(),
                            content,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }

                // Tool results become separate messages with role "tool"
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        result.push(OpenAIMessage {
                            role: "tool".to_string(),
                            content: Some(OpenAIContent::Text(content.clone())),
                            tool_calls: None,
                            tool_call_id: Some(tool_use_id.clone()),
                        });
                    }
                }
            }
            MessageRole::Assistant => {
                // Assistant messages: text + tool calls
                let text_parts: Vec<String> = msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect();

                let tool_calls: Vec<OpenAIToolCall> = msg
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolUse { id, name, input } => Some(OpenAIToolCall {
                            id: id.clone(),
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: name.clone(),
                                arguments: serde_json::to_string(input).unwrap_or_default(),
                            },
                        }),
                        _ => None,
                    })
                    .collect();

                let content = if text_parts.is_empty() {
                    None
                } else {
                    Some(OpenAIContent::Text(text_parts.join("\n")))
                };

                let tool_calls_opt = if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                };

                // Only push assistant message if it has content or tool calls
                // (skip when message only contains ToolResult blocks)
                if content.is_some() || tool_calls_opt.is_some() {
                    result.push(OpenAIMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: tool_calls_opt,
                        tool_call_id: None,
                    });
                }

                // Tool results become separate messages with role "tool"
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        result.push(OpenAIMessage {
                            role: "tool".to_string(),
                            content: Some(OpenAIContent::Text(content.clone())),
                            tool_calls: None,
                            tool_call_id: Some(tool_use_id.clone()),
                        });
                    }
                }
            }
        }

        result
    }

    /// Normalize OpenAI response to our internal format.
    fn normalize_response(resp: OpenAIResponse) -> Result<LlmResponse, LlmError> {
        let choice = resp.choices.into_iter().next().ok_or_else(|| {
            LlmError::unknown("AI Gateway returned no choices in response".to_string())
        })?;

        let mut content = Vec::new();

        // Add text content
        if let Some(msg_content) = choice.message.content {
            use crate::llm::openai::{OpenAIContent, OpenAIContentPart};
            match msg_content {
                OpenAIContent::Text(text) => {
                    if !text.is_empty() {
                        content.push(ContentBlock::Text { text });
                    }
                }
                OpenAIContent::Parts(parts) => {
                    // Extract text from parts (images in responses are not expected)
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

        // Add tool calls
        if let Some(tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                if tc.function.name.is_empty() {
                    return Err(LlmError::unknown(
                        "AI Gateway returned tool call with empty function name",
                    ));
                }

                let input = serde_json::from_str(&tc.function.arguments).map_err(|e| {
                    LlmError::unknown(format!(
                        "Invalid JSON in tool call arguments: {}",
                        e
                    ))
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
                "AI Gateway returned empty response (no content or tool calls)".to_string(),
            ));
        }

        let end_turn = choice.finish_reason.as_deref() == Some("stop");

        let usage = Usage {
            input_tokens: resp.usage.prompt_tokens as u64,
            output_tokens: resp.usage.completion_tokens as u64,
            cache_creation_tokens: 0, // AI Gateway doesn't expose cache metrics via OpenAI format
            cache_read_tokens: 0,
        };

        Ok(LlmResponse {
            content,
            end_turn,
            usage,
        })
    }

    /// Classify HTTP error responses.
    fn classify_error(status: reqwest::StatusCode, body: &str) -> LlmError {
        match status.as_u16() {
            401 | 403 => LlmError::auth(format!("Authentication failed: {}", body)),
            429 => LlmError::rate_limit(format!("Rate limited: {}", body)),
            400 => LlmError::invalid_request(format!("Invalid request: {}", body)),
            500..=599 => LlmError::server_error(format!("Server error: {}", body)),
            _ => LlmError::unknown(format!("HTTP {}: {}", status, body)),
        }
    }
}

#[async_trait]
impl LlmService for AIGatewayService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let token = self.get_token()?;
        let base_url = get_base_url();
        let url = format!("{}/v1/chat/completions", base_url);

        let openai_request = self.translate_request(request);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("source", &self.source)
            .header("org-id", &self.org_id)
            .header("Content-Type", "application/json")
            .json(&openai_request)
            .send()
            .await
            .map_err(|e| LlmError::network(format!("Request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("(no body)"));
            return Err(Self::classify_error(status, &body));
        }

        let openai_response: OpenAIResponse = response.json().await.map_err(|e| {
            LlmError::unknown(format!("Failed to parse AI Gateway response: {}", e))
        })?;

        Self::normalize_response(openai_response)
    }

    fn model_id(&self) -> &str {
        self.model
    }

    fn context_window(&self) -> usize {
        // AI Gateway supports various models with different context windows
        // Return a safe default; the actual limit depends on the model
        200_000
    }

    fn max_image_dimension(&self) -> Option<u32> {
        // Depends on the model; return None for now
        None
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use crate::llm::openai::OpenAIResponse;
    use crate::llm::types::LlmMessage;

    pub fn convert_message(msg: &LlmMessage) -> Vec<OpenAIMessage> {
        AIGatewayService::convert_message(msg)
    }

    pub fn normalize_response(
        resp: OpenAIResponse,
    ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
        AIGatewayService::normalize_response(resp)
    }
}
