//! Google Gemini provider implementation

use super::types::{
    ContentBlock, LlmRequest, LlmResponse, MessageRole, Usage,
};
use super::{LlmError, LlmService};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Gemini models
#[derive(Debug, Clone, Copy)]
pub enum GeminiModel {
    Gemini3Pro,
    Gemini3Flash,
}

impl GeminiModel {
    pub fn api_name(self) -> &'static str {
        match self {
            GeminiModel::Gemini3Pro => "gemini-3.0-pro",
            GeminiModel::Gemini3Flash => "gemini-3.0-flash",
        }
    }

    pub fn model_id(self) -> &'static str {
        match self {
            GeminiModel::Gemini3Pro => "gemini-3-pro",
            GeminiModel::Gemini3Flash => "gemini-3-flash",
        }
    }

    pub fn context_window(self) -> usize {
        match self {
            GeminiModel::Gemini3Pro => 2_097_152,   // 2M
            GeminiModel::Gemini3Flash => 1_048_576, // 1M
        }
    }
}

/// Gemini service implementation
pub struct GeminiService {
    client: Client,
    api_key: String,
    model: GeminiModel,
    base_url: String,
    model_id: String,
}

impl GeminiService {
    pub fn new(api_key: String, model: GeminiModel, gateway: Option<&str>) -> Self {
        let base_url = match gateway {
            Some(gw) => {
                // exe.dev gateway format
                format!(
                    "{}/gemini/v1/models/{}-latest:generateContent",
                    gw.trim_end_matches('/'),
                    model.api_name()
                )
            }
            None => {
                // Direct Gemini API
                format!(
                    "https://generativelanguage.googleapis.com/v1/models/{}-latest:generateContent",
                    model.api_name()
                )
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

    fn translate_request(&self, request: &LlmRequest) -> GeminiRequest {
        let mut contents = Vec::new();

        // Add system instruction if present
        let system_instruction = if !request.system.is_empty() {
            Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart::Text {
                    text: request
                        .system
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                }],
            })
        } else {
            None
        };

        // Convert messages
        for msg in &request.messages {
            let role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "model",
            };

            let parts: Vec<GeminiPart> = msg
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(GeminiPart::Text { text: text.clone() }),
                    ContentBlock::ToolUse { id: _, name, input } => {
                        Some(GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: name.clone(),
                                args: input.clone(),
                            },
                        })
                    }
                    ContentBlock::ToolResult {
                        tool_use_id: _,
                        content,
                        is_error,
                    } => {
                        Some(GeminiPart::FunctionResponse {
                            function_response: GeminiFunctionResponse {
                                name: "function".to_string(), // Gemini doesn't track IDs
                                response: serde_json::json!({
                                    "result": content,
                                    "error": is_error
                                }),
                            },
                        })
                    }
                    _ => None, // Skip images for now
                })
                .collect();

            if !parts.is_empty() {
                contents.push(GeminiContent {
                    role: Some(role.to_string()),
                    parts,
                });
            }
        }

        // Convert tools
        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(vec![GeminiTool {
                function_declarations: request
                    .tools
                    .iter()
                    .map(|t| GeminiFunctionDeclaration {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.input_schema.clone(),
                    })
                    .collect(),
            }])
        };

        GeminiRequest {
            contents,
            system_instruction,
            tools,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: request.max_tokens.map(|t| t as i32),
                temperature: None,
                top_p: None,
                top_k: None,
            }),
        }
    }

    fn normalize_response(resp: GeminiResponse) -> Result<LlmResponse, LlmError> {
        let candidate = resp
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::unknown("No candidates in response"))?;

        let mut content = Vec::new();

        // Extract content from parts
        for part in candidate.content.parts {
            match part {
                GeminiPart::Text { text } => {
                    if !text.is_empty() {
                        content.push(ContentBlock::Text { text });
                    }
                }
                GeminiPart::FunctionCall { function_call } => {
                    content.push(ContentBlock::ToolUse {
                        id: format!("call_{}", function_call.name), // Generate ID
                        name: function_call.name,
                        input: function_call.args,
                    });
                }
                _ => {} // Ignore other types
            }
        }

        let end_turn = candidate
            .finish_reason
            .map(|r| r == "STOP")
            .unwrap_or(false);

        Ok(LlmResponse {
            content,
            end_turn,
            usage: Usage {
                input_tokens: resp.usage_metadata.prompt_token_count as u64,
                output_tokens: resp.usage_metadata.candidates_token_count as u64,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }
}

#[async_trait]
impl LlmService for GeminiService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let gemini_request = self.translate_request(request);

        let url = if self.api_key.starts_with("implicit") {
            // Gateway mode - key in URL not needed
            self.base_url.clone()
        } else {
            // Direct mode - add API key to URL
            format!("{}?key={}", self.base_url, self.api_key)
        };

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&gemini_request)
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
            if let Ok(error_resp) = serde_json::from_str::<GeminiErrorResponse>(&body) {
                let message = error_resp.error.message;
                return Err(match status.as_u16() {
                    400 => LlmError::invalid_request(format!("Invalid request: {}", message)),
                    401 | 403 => LlmError::auth(format!("Authentication failed: {}", message)),
                    429 => LlmError::rate_limit(format!("Rate limit exceeded: {}", message)),
                    500..=599 => LlmError::server_error(format!("Server error: {}", message)),
                    _ => LlmError::unknown(format!("HTTP {}: {}", status, message)),
                });
            }
            return Err(LlmError::unknown(format!(
                "HTTP {} error: {}",
                status, body
            )));
        }

        let gemini_response: GeminiResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::unknown(format!("Failed to parse response: {} - body: {}", e, body))
        })?;

        Self::normalize_response(gemini_response)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn context_window(&self) -> usize {
        self.model.context_window()
    }

    fn max_image_dimension(&self) -> Option<u32> {
        Some(2048) // Gemini supports images up to 2048x2048
    }
}

// Gemini API types

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: GeminiUsageMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: u32,
    candidates_token_count: u32,
    total_token_count: u32,
}

#[derive(Debug, Deserialize)]
struct GeminiErrorResponse {
    error: GeminiError,
}

#[derive(Debug, Deserialize)]
struct GeminiError {
    message: String,
    #[allow(dead_code)]
    code: Option<i32>,
    #[allow(dead_code)]
    status: Option<String>,
}
