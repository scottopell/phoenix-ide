# LLM Provider - Design Document

## Overview

The LLM provider abstracts communication with various LLM APIs (Anthropic, OpenAI, etc.) behind a common interface. It handles provider-specific request/response translation, gateway routing for exe.dev, and usage tracking.

## Service Interface (REQ-LLM-001)

```rust
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Make a completion request
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;
    
    /// Get the context window size in tokens
    fn context_window(&self) -> usize;
    
    /// Get max image dimension (for resizing before send)
    fn max_image_dimension(&self) -> Option<u32>;
}

pub struct LlmRequest {
    pub system: Vec<SystemContent>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
}

pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub end_turn: bool,
    pub usage: Usage,
}

pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: Option<f64>,
}
```

## Error Types (REQ-LLM-006)

```rust
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
    pub retry_after: Option<Duration>,
}

pub enum LlmErrorKind {
    /// Network issues, timeouts - retryable
    Network,
    /// Rate limited (429) - retryable with backoff
    RateLimit,
    /// Server error (5xx) - retryable
    ServerError,
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Unknown error
    Unknown,
}

impl LlmErrorKind {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Network | Self::RateLimit | Self::ServerError)
    }
}
```

## Provider Implementations

### Anthropic Provider

```rust
pub struct AnthropicService {
    api_key: String,
    model: AnthropicModel,
    base_url: String,  // Default or gateway URL
}

impl AnthropicService {
    pub fn new(api_key: String, model: AnthropicModel, gateway: Option<&str>) -> Self {
        let base_url = match gateway {
            Some(gw) => format!("{}/_/gateway/anthropic/v1/messages", gw),
            None => "https://api.anthropic.com/v1/messages".to_string(),
        };
        Self { api_key, model, base_url }
    }
}

pub enum AnthropicModel {
    Claude45Opus,
    Claude45Sonnet,
    Claude45Haiku,
}
```

### OpenAI Provider

```rust
pub struct OpenAiService {
    api_key: String,
    model: OpenAiModel,
    base_url: String,
}

impl OpenAiService {
    pub fn new(api_key: String, model: OpenAiModel, gateway: Option<&str>) -> Self {
        let base_url = match gateway {
            Some(gw) => format!("{}/_/gateway/openai/v1", gw),
            None => "https://api.openai.com/v1".to_string(),
        };
        Self { api_key, model, base_url }
    }
}
```

### Fireworks Provider

```rust
pub struct FireworksService {
    api_key: String,
    model: FireworksModel,
    base_url: String,
}

impl FireworksService {
    pub fn new(api_key: String, model: FireworksModel, gateway: Option<&str>) -> Self {
        let base_url = match gateway {
            Some(gw) => format!("{}/_/gateway/fireworks/inference/v1", gw),
            None => "https://api.fireworks.ai/inference/v1".to_string(),
        };
        Self { api_key, model, base_url }
    }
}
```

## Model Registry (REQ-LLM-003)

```rust
pub struct ModelRegistry {
    services: HashMap<String, Arc<dyn LlmService>>,
    logger: slog::Logger,
}

impl ModelRegistry {
    pub fn new(config: &LlmConfig, logger: slog::Logger) -> Self {
        let mut services = HashMap::new();
        
        // Register Anthropic models if API key available
        if let Some(key) = &config.anthropic_api_key {
            services.insert(
                "claude-opus-4.5".to_string(),
                Arc::new(AnthropicService::new(
                    key.clone(),
                    AnthropicModel::Claude45Opus,
                    config.gateway.as_deref(),
                )) as Arc<dyn LlmService>,
            );
            // ... other Claude models
        }
        
        // Register OpenAI models if API key available
        if let Some(key) = &config.openai_api_key {
            services.insert(
                "gpt-5".to_string(),
                Arc::new(OpenAiService::new(
                    key.clone(),
                    OpenAiModel::Gpt5,
                    config.gateway.as_deref(),
                )) as Arc<dyn LlmService>,
            );
            // ... other GPT models
        }
        
        Self { services, logger }
    }
    
    pub fn get(&self, model_id: &str) -> Option<Arc<dyn LlmService>> {
        self.services.get(model_id).cloned()
    }
    
    pub fn available_models(&self) -> Vec<String> {
        self.services.keys().cloned().collect()
    }
}
```

## Gateway URL Construction (REQ-LLM-002)

| Provider | Gateway Suffix | Direct URL |
|----------|---------------|------------|
| Anthropic | `/_/gateway/anthropic/v1/messages` | `https://api.anthropic.com/v1/messages` |
| OpenAI | `/_/gateway/openai/v1` | `https://api.openai.com/v1` |
| Fireworks | `/_/gateway/fireworks/inference/v1` | `https://api.fireworks.ai/inference/v1` |
| Gemini | `/_/gateway/gemini/v1/models/generate` | `https://generativelanguage.googleapis.com/v1` |

## Request Translation (REQ-LLM-004)

### Common to Anthropic

```rust
impl AnthropicService {
    fn translate_request(&self, req: &LlmRequest) -> AnthropicRequest {
        AnthropicRequest {
            model: self.model.api_name(),
            max_tokens: req.max_tokens.unwrap_or(8192),
            system: req.system.iter().map(|s| AnthropicSystemBlock {
                r#type: "text",
                text: &s.text,
                cache_control: s.cache.then_some(CacheControl { r#type: "ephemeral" }),
            }).collect(),
            messages: req.messages.iter().map(|m| self.translate_message(m)).collect(),
            tools: req.tools.iter().map(|t| AnthropicTool {
                name: &t.name,
                description: &t.description,
                input_schema: &t.schema,
            }).collect(),
        }
    }
}
```

### Common to OpenAI

```rust
impl OpenAiService {
    fn translate_request(&self, req: &LlmRequest) -> OpenAiRequest {
        let mut messages = vec![];
        
        // System as first message
        if !req.system.is_empty() {
            messages.push(OpenAiMessage {
                role: "system",
                content: req.system.iter().map(|s| &s.text).collect::<Vec<_>>().join("\n"),
            });
        }
        
        // Conversation messages
        for msg in &req.messages {
            messages.push(self.translate_message(msg));
        }
        
        OpenAiRequest {
            model: self.model.api_name(),
            messages,
            tools: req.tools.iter().map(|t| OpenAiTool {
                r#type: "function",
                function: OpenAiFunction {
                    name: &t.name,
                    description: &t.description,
                    parameters: &t.schema,
                },
            }).collect(),
            max_tokens: req.max_tokens,
        }
    }
}
```

## Response Normalization (REQ-LLM-005)

```rust
impl AnthropicService {
    fn normalize_response(&self, resp: AnthropicResponse) -> LlmResponse {
        let content = resp.content.into_iter().map(|block| {
            match block {
                AnthropicBlock::Text { text } => ContentBlock::Text { text },
                AnthropicBlock::ToolUse { id, name, input } => {
                    ContentBlock::ToolUse { id, name, input }
                }
            }
        }).collect();
        
        LlmResponse {
            content,
            end_turn: resp.stop_reason == Some("end_turn"),
            usage: Usage {
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
                cache_creation_tokens: resp.usage.cache_creation_input_tokens.unwrap_or(0),
                cache_read_tokens: resp.usage.cache_read_input_tokens.unwrap_or(0),
                cost_usd: self.calculate_cost(&resp.usage),
            },
        }
    }
}
```

## Usage Tracking (REQ-LLM-007)

```rust
impl Usage {
    pub fn context_window_used(&self) -> u64 {
        self.input_tokens + self.output_tokens + 
        self.cache_creation_tokens + self.cache_read_tokens
    }
    
    pub fn is_zero(&self) -> bool {
        self.input_tokens == 0 && self.output_tokens == 0
    }
}

// Cost calculation per model
impl AnthropicService {
    fn calculate_cost(&self, usage: &AnthropicUsage) -> Option<f64> {
        let (input_cost, output_cost) = match self.model {
            AnthropicModel::Claude45Opus => (15.0 / 1_000_000.0, 75.0 / 1_000_000.0),
            AnthropicModel::Claude45Sonnet => (3.0 / 1_000_000.0, 15.0 / 1_000_000.0),
            AnthropicModel::Claude45Haiku => (0.25 / 1_000_000.0, 1.25 / 1_000_000.0),
        };
        
        Some(
            usage.input_tokens as f64 * input_cost +
            usage.output_tokens as f64 * output_cost
        )
    }
}
```

## Request Logging (REQ-LLM-008)

```rust
pub struct LoggingService {
    inner: Arc<dyn LlmService>,
    logger: slog::Logger,
    model_id: String,
    history: Option<Arc<RequestHistory>>,
}

#[async_trait]
impl LlmService for LoggingService {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let start = Instant::now();
        let result = self.inner.complete(request).await;
        let duration = start.elapsed();
        
        match &result {
            Ok(response) => {
                info!(self.logger, "LLM request completed";
                    "model" => &self.model_id,
                    "duration_ms" => duration.as_millis(),
                    "input_tokens" => response.usage.input_tokens,
                    "output_tokens" => response.usage.output_tokens,
                );
            }
            Err(e) => {
                error!(self.logger, "LLM request failed";
                    "model" => &self.model_id,
                    "duration_ms" => duration.as_millis(),
                    "error" => %e.message,
                    "retryable" => e.kind.is_retryable(),
                );
            }
        }
        
        // Record for debug inspection if enabled
        if let Some(history) = &self.history {
            history.record(RequestRecord {
                timestamp: Utc::now(),
                model_id: self.model_id.clone(),
                duration,
                result: result.as_ref().map(|r| r.usage.clone()).err().map(|e| e.message.clone()),
            });
        }
        
        result
    }
}
```

## Configuration

```rust
pub struct LlmConfig {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub fireworks_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    
    /// exe.dev gateway URL (e.g., "https://meteor-rain.exe.xyz")
    pub gateway: Option<String>,
    
    /// Default model ID
    pub default_model: Option<String>,
}

impl LlmConfig {
    pub fn from_env() -> Self {
        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            fireworks_api_key: std::env::var("FIREWORKS_API_KEY").ok(),
            gemini_api_key: std::env::var("GEMINI_API_KEY").ok(),
            gateway: std::env::var("LLM_GATEWAY").ok(),
            default_model: std::env::var("DEFAULT_MODEL").ok(),
        }
    }
}
```

## File Organization

```
src/llm/
├── mod.rs              # LlmService trait, common types
├── error.rs            # LlmError, LlmErrorKind
├── registry.rs         # ModelRegistry
├── anthropic/
│   ├── mod.rs
│   ├── service.rs      # AnthropicService
│   ├── types.rs        # Anthropic API types
│   └── translate.rs    # Request/response translation
├── openai/
│   ├── mod.rs
│   ├── service.rs
│   ├── types.rs
│   └── translate.rs
├── fireworks/
│   └── ...
└── logging.rs          # LoggingService wrapper
```
