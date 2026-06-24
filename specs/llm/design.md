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
}
```

## Error Types (REQ-LLM-006, REQ-LLM-006a)

No `Unknown` variant. No `#[non_exhaustive]`. No `_ =>` in match arms. Adding a new
HTTP status class requires adding a variant and handling it in every consumer — the
compiler forces it.

`RateLimit` and `UsageLimitReached` are split deliberately. A 429 can mean either a
transient per-window throttle (retry in seconds) or a quota window having reset-on-a-
calendar-boundary (retry next Sunday). Treating both as `RateLimit` either wastes work
or misclassifies recovery, depending on which way the default lands. The split keeps
automatic retry policy honest and separate from user-triggered resume policy.

```rust
pub struct LlmError {
    pub kind: LlmErrorKind,
    pub message: String,
    pub retry_after: Option<Duration>,
    /// Present iff `kind == UsageLimitReached`. Structured payload extracted
    /// from the codex backend's 429 response (body + headers). Used to render
    /// plan-aware messages and (later) drive a quota status indicator.
    pub quota: Option<QuotaDetails>,
}

pub enum LlmErrorKind {
    /// Network issues, timeouts - retryable
    Network,
    /// Transient rate-limit throttle (per-minute, per-second windows) - retryable with backoff
    RateLimit,
    /// Quota window exhausted (plan-level cap hit, credits depleted, etc.) -
    /// never auto-retried, but user-resumable once the window resets
    UsageLimitReached,
    /// Server error (5xx) - retryable
    ServerError,
    /// Selected model is at capacity (`server_is_overloaded` / `slow_down`) - NOT retryable
    ServerOverloaded,
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Provider returned bytes we could not parse or understand (malformed SSE
    /// event, unparseable body, unexpected content-block shape) - retryable
    InvalidResponse,
    /// Content filter or safety block - not retryable
    ContentFilter,
    /// Context window exceeded - not retryable in current conversation
    ContextWindowExceeded,
    // No Unknown. No _.
}

pub enum AutoRetryPolicy { AutoRetryable, NoAutoRetry }
pub enum UserResumePolicy { Resumable, NotResumable }

impl LlmErrorKind {
    pub fn auto_retry_policy(&self) -> AutoRetryPolicy {
        match self {
            Self::Network | Self::RateLimit | Self::ServerError | Self::InvalidResponse => {
                AutoRetryPolicy::AutoRetryable
            }
            Self::UsageLimitReached
            | Self::ServerOverloaded
            | Self::Auth
            | Self::InvalidRequest
            | Self::ContentFilter
            | Self::ContextWindowExceeded => AutoRetryPolicy::NoAutoRetry,
        }
    }

    pub fn user_resume_policy(&self) -> UserResumePolicy {
        match self {
            // A usage-limit window resets on a clock boundary, so — like
            // `ServerOverloaded` — the user can resume once it clears, even
            // though it is never auto-retried.
            Self::Auth
            | Self::Network
            | Self::RateLimit
            | Self::ServerError
            | Self::InvalidResponse
            | Self::ServerOverloaded
            | Self::UsageLimitReached => UserResumePolicy::Resumable,
            Self::InvalidRequest
            | Self::ContentFilter
            | Self::ContextWindowExceeded => UserResumePolicy::NotResumable,
        }
    }
}

/// Structured quota state extracted from the codex backend on 429.
///
/// All fields are optional: the codex backend populates a subset depending on
/// which limit was hit (per-model vs global), the user's plan, and whether
/// credits are tracked. Consumers must handle every field being None.
pub struct QuotaDetails {
    pub plan_type: Option<String>,         // raw "plus" / "pro" / "team" / ...
    pub resets_at: Option<DateTime<Utc>>,  // when the active window resets
    pub limit_id: Option<String>,          // active limit family, e.g. "codex"
    pub limit_name: Option<String>,        // human label, e.g. "gpt-5.2-codex-sonic"
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub credits: Option<CreditsSnapshot>,
    pub promo_message: Option<String>,
}

pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,            // unix seconds
}

pub struct CreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}
```

### Codex backend 429 parsing (REQ-LLM-006a)

When `use_codex_backend == true` and a request returns HTTP 429, both the
non-streaming and streaming paths run an additional parsing step before
falling through to the generic `LlmError::rate_limit` path:

1. Attempt to deserialize the body as `{ error: { type, plan_type, resets_at } }`.
2. On `type == "usage_limit_reached"`:
   - Read response headers via the `x-codex-*` family
     (`primary-used-percent`, `primary-window-minutes`, `primary-reset-at`,
     secondary variants, `credits-*`, `active-limit`, `limit-name`,
     `promo-message`) to populate `QuotaDetails`.
   - Render the message using plan-aware wording (recovery action per plan,
     absolute reset time in user's local timezone).
   - Return `LlmError { kind: UsageLimitReached, ... }`.
3. On `type == "usage_not_included"`: return `LlmError { kind: Auth, ... }`
   with an upgrade-required message.
4. Otherwise (no recognized `type` field): return
   `LlmError { kind: RateLimit, ... }` — transient throttle, retryable.

HTTP 503 with body `error.code in {server_is_overloaded, slow_down}` returns
`LlmError { kind: ServerOverloaded, ... }`. Platform Responses API requests
(`use_codex_backend == false`) skip this entire path — they're unchanged.

This mirrors the canonical client behavior of the codex CLI (against the
same backend), so user-facing wording aligns with adjacent tools.

## Streaming Interface (REQ-LLM-009)

The `LlmService` trait gains a streaming method alongside `complete()`:

```rust
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Non-streaming: blocks until full response is available
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Streaming: emits chunks via the sender, then returns the final assembled response.
    /// The final LlmResponse is identical to what complete() would return.
    /// Default implementation calls complete() with no streaming.
    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &broadcast::Sender<TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        // Default: no streaming, just call complete()
        self.complete(request).await
    }

    fn context_window(&self) -> usize;
    fn max_image_dimension(&self) -> Option<u32>;
}

/// Chunks emitted during streaming. Only text deltas are forwarded to the UI;
/// tool input fragments are accumulated internally.
pub enum TokenChunk {
    Text(String),
    ToolUseStart { tool_use_id: String, tool_name: String },
    ToolUseInput { tool_use_id: String, partial_json: String },
    ToolUseDone { tool_use_id: String },
}
```

### Provider Streaming Implementation

Each provider reads the HTTP response body as a stream, maintaining an accumulator:

- **Current block type** (`text` or `tool_use`) — declared before deltas arrive
  (Anthropic/OpenAI Responses), or inferred from delta field (Chat Completions)
- **Per-block text buffer** — for text blocks: forwarded via `chunk_tx` immediately;
  for tool blocks: accumulated for final JSON parse
- **Tool call metadata** — `id`, `name` known from block-start before any deltas

On stream end: parse all accumulated tool input buffers as JSON, assemble final
`LlmResponse` with full `ContentBlock` list and `Usage` data.

### LlmOutcome (Executor Boundary Type)

The executor's LLM background task produces an `LlmOutcome` for the oneshot channel.
This is the typed boundary between provider I/O and the pure state machine:

```rust
pub enum LlmOutcome {
    Response(AssistantMessage, TokenUsage),
    /// Transient rate-limit throttle - retryable
    RateLimited { retry_after: Option<Duration> },
    /// Quota window exhausted - terminal. `details` carries plan + reset + windows.
    UsageLimitReached { details: QuotaDetails, message: String },
    ServerError { status: u16, body: String },
    /// Selected model at capacity - terminal, suggest different model
    ServerOverloaded { message: String },
    NetworkError { message: String },
    TokenBudgetExceeded { partial: Option<AssistantMessage> },
    Cancelled,
}
```

The executor maps `Result<LlmResponse, LlmError>` → `LlmOutcome` via a total function
(no `_ =>` arm). This mapping is where FM-3 prevention lives.

## Backend Implementations

Phoenix has two HTTP backend families plus the in-process mock:

| Backend | Request URL | Wire protocol |
|---------|-------------|---------------|
| `anthropic` | `ANTHROPIC_BASE_URL` as-is, or `https://api.anthropic.com/v1/messages` | Anthropic Messages |
| `openai_responses` | `OPENAI_BASE_URL` as-is, or `https://api.openai.com/v1/responses` | OpenAI Responses |

Configured base URLs are exact endpoints. Phoenix never treats them as gateway roots and never appends hidden provider path suffixes.

```rust
pub enum ModelBackend {
    Anthropic,
    OpenAIResponses,
    Mock,
}

pub struct ModelSpec {
    pub id: String,
    pub api_name: String,
    pub backend: ModelBackend,
    pub description: String,
    pub context_window: usize,
    pub recommended: bool,
    pub supports_tool_search: bool,
}
```

## Model Registry (REQ-LLM-003, REQ-SA-007)

The registry starts from built-in model specs and merges valid additive specs from `PHOENIX_LLM_MODELS`. External specs declare a backend and may omit `api_name`, in which case Phoenix sends `id` as the wire model name. Duplicate IDs keep the built-in or earlier configured definition.

```rust
pub struct ModelRegistry {
    services: HashMap<String, Arc<dyn LlmService>>,
    specs: HashMap<String, ModelSpec>,
}
```

Registration requires an auth route for the model backend: static API key, credential helper, Codex bridge for built-in OpenAI Responses models, or mock opt-in. Externally configured OpenAI Responses models bypass the Codex bridge and use explicit endpoint/auth configuration.

### Model Discovery

When a credential helper and compatible base URL are configured, Phoenix derives model-listing URLs by replacing the final path segment with `models`:

| Base URL | Discovery URL |
|----------|---------------|
| `https://proxy.example/v1/messages` | `https://proxy.example/v1/models` |
| `https://proxy.example/v1/responses` | `https://proxy.example/v1/models` |

Discovery is backend-scoped. Anthropic discovery results only validate `anthropic` models; OpenAI Responses discovery results only validate `openai_responses` models. If discovery returns no usable configured models, Phoenix falls back to the configured model list and logs a warning.

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
```

## Request Logging (REQ-LLM-008)

```rust
pub struct LoggingService {
    inner: Arc<dyn LlmService>,
    logger: slog::Logger,
    model_id: String,
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
                    "auto_retryable" => e.kind.is_auto_retryable(),
                );
            }
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

    /// Exact Anthropic Messages-compatible endpoint override.
    pub anthropic_base_url: Option<String>,

    /// Exact OpenAI Responses-compatible endpoint override.
    pub openai_base_url: Option<String>,

    /// Default model ID
    pub default_model: Option<String>,
}

impl LlmConfig {
    pub fn from_env() -> Self {
        Self {
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            anthropic_base_url: std::env::var("ANTHROPIC_BASE_URL").ok(),
            openai_base_url: std::env::var("OPENAI_BASE_URL").ok(),
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
