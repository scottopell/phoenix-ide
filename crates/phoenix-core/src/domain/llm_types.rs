//! Common types for LLM interactions

pub const LLM_SOURCE_HEADER: &str = "phoenix-ide";

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

/// Identifier for `OpenAI`'s `prompt_cache_key` Responses-API field. Required
/// on every `LlmRequest` so callers must explicitly choose a caching strategy
/// — passing the wrong key only loses cache hits, never breaks correctness,
/// but silently omitting one is exactly the failure mode this type prevents.
///
/// Same key + same prefix bytes (system prompt, leading messages, tools) =
/// cache hit on the `OpenAI` Responses backend. GPT-5.6-era direct platform
/// requests also combine this key with implicit and explicit wire breakpoints.
/// `Anthropic` ignores the key and uses per-block `cache_control` instead.
///
/// # Choosing a constructor
///
/// - [`PromptCacheKey::stable`] for any call that belongs to a cohort that
///   should reuse cached prefix tokens. Common ids:
///     - `conversation_id` for the main turn loop (every turn shares the
///       system prompt + earlier-turn cache)
///     - a category like `"title-gen"` for utility calls that share
///       boilerplate across all conversations
///     - a chain or session id for grouped sub-calls
/// - [`PromptCacheKey::ephemeral`] only for cases with no caching cohort
///   (one-off tests, ad-hoc internal calls). Generates a fresh value per
///   call so the request is well-formed but cannot share prefix tokens
///   with anything else.
///
/// There is intentionally no `Default`: each call site has to decide.
#[derive(Debug, Clone)]
pub struct PromptCacheKey(String);

impl PromptCacheKey {
    /// A stable cache key shared by all calls passing the same `id`. Calls
    /// using this key reuse cached prefix tokens against each other on the
    /// `OpenAI` Responses backend.
    pub fn stable(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// A fresh per-call key. The request is well-formed but the cache can
    /// never hit. Use only when there's no natural caching cohort (currently
    /// only test fixtures — production call sites all have a stable cohort).
    #[allow(dead_code)] // public API kept for legitimate one-off production use
    #[must_use]
    pub fn ephemeral() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ModelEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ModelEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    #[must_use]
    pub const fn needs_extended_output_headroom(self) -> bool {
        matches!(self, Self::Xhigh | Self::Max)
    }

    #[must_use]
    pub const fn as_wire_name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for ModelEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_wire_name())
    }
}

impl FromStr for ModelEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => Err(format!("unknown effort level '{other}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum EffortSource {
    NativeKnown,
    NativeUnknown,
    Explicit,
    Unsupported,
}

impl EffortSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeKnown => "native_known",
            Self::NativeUnknown => "native_unknown",
            Self::Explicit => "explicit",
            Self::Unsupported => "unsupported",
        }
    }
}

impl FromStr for EffortSource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "native_known" => Ok(Self::NativeKnown),
            "native_unknown" => Ok(Self::NativeUnknown),
            "explicit" => Ok(Self::Explicit),
            "unsupported" => Ok(Self::Unsupported),
            other => Err(format!("unknown effort source '{other}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveEffort {
    Explicit(ModelEffort),
    NativeKnown(ModelEffort),
    NativeUnknown,
    Unsupported,
}

impl EffectiveEffort {
    #[must_use]
    pub const fn explicit(level: ModelEffort) -> Self {
        Self::Explicit(level)
    }

    #[must_use]
    pub const fn native_known(level: ModelEffort) -> Self {
        Self::NativeKnown(level)
    }

    #[must_use]
    pub const fn native_unknown() -> Self {
        Self::NativeUnknown
    }

    #[must_use]
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }

    #[must_use]
    pub const fn source(self) -> EffortSource {
        match self {
            Self::Explicit(_) => EffortSource::Explicit,
            Self::NativeKnown(_) => EffortSource::NativeKnown,
            Self::NativeUnknown => EffortSource::NativeUnknown,
            Self::Unsupported => EffortSource::Unsupported,
        }
    }

    #[must_use]
    pub const fn level(self) -> Option<ModelEffort> {
        match self {
            Self::Explicit(level) | Self::NativeKnown(level) => Some(level),
            Self::NativeUnknown | Self::Unsupported => None,
        }
    }

    #[must_use]
    pub const fn explicit_level(self) -> Option<ModelEffort> {
        match self {
            Self::Explicit(level) => Some(level),
            Self::NativeKnown(_) | Self::NativeUnknown | Self::Unsupported => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmRequestTelemetry {
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub request_id: String,
    pub retry_attempt: u32,
    pub attempt_capture: LlmAttemptCapture,
}

/// Content-free summary of a provider streaming attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamTelemetryOutputKind {
    None,
    Text,
    Reasoning,
    Tool,
    Structured,
    Mixed,
}

/// Final, content-free snapshot of provider stream timing and shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStreamTelemetry {
    pub dispatch_to_first_provider_event_ms: Option<u64>,
    pub dispatch_to_first_generation_event_ms: Option<u64>,
    pub dispatch_to_first_visible_text_ms: Option<u64>,
    pub provider_event_count: u32,
    pub generation_event_count: u32,
    pub visible_text_event_count: u32,
    pub max_provider_gap_ms: Option<u64>,
    pub max_generation_gap_ms: Option<u64>,
    pub output_kind: StreamTelemetryOutputKind,
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmTransport {
    HttpSse,
    Websocket,
    InProcess,
    HttpJson,
}

impl LlmTransport {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HttpSse => "http_sse",
            Self::Websocket => "websocket",
            Self::InProcess => "in_process",
            Self::HttpJson => "http_json",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmAttemptOutcome {
    Success,
    RateLimited,
    UsageLimitReached,
    ServerError,
    InvalidResponse,
    ServerOverloaded,
    NetworkError,
    TokenBudgetExceeded,
    AuthError,
    RequestRejected,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmAttemptMetrics {
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub request_id: String,
    pub retry_attempt: u32,
    pub provider: String,
    pub model: String,
    pub transport: LlmTransport,
    pub total_duration_ms: u64,
    pub stream: ProviderStreamTelemetry,
    pub outcome: LlmAttemptOutcome,
}

#[derive(Debug, Clone)]
pub struct LlmAttemptFinalization {
    pub stream: Option<ProviderStreamTelemetry>,
    pub outcome: LlmAttemptOutcome,
}

#[derive(Debug, Clone)]
pub struct LlmAttemptCapture(Arc<Mutex<LlmAttemptCaptureState>>);

#[derive(Debug, Clone, Default)]
struct LlmAttemptCaptureState {
    progress: Option<ProviderStreamTelemetry>,
    transport: Option<LlmTransport>,
    identity: Option<LlmAttemptIdentity>,
    finalized: Option<LlmAttemptMetrics>,
}

#[derive(Debug, Clone)]
struct LlmAttemptIdentity {
    conversation_id: String,
    root_conversation_id: String,
    request_id: String,
    retry_attempt: u32,
    provider: String,
    model: String,
    fallback_transport: LlmTransport,
    started_at: std::time::Instant,
}

impl Default for LlmAttemptCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmAttemptCapture {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(LlmAttemptCaptureState::default())))
    }

    pub fn publish_progress(&self, progress: ProviderStreamTelemetry) {
        if let Ok(mut state) = self.0.lock() {
            state.progress = Some(progress);
        }
    }

    pub fn set_transport(&self, transport: LlmTransport) {
        if let Ok(mut state) = self.0.lock() {
            state.transport = Some(transport);
        }
    }

    pub fn begin(
        &self,
        telemetry: &LlmRequestTelemetry,
        provider: &str,
        model: &str,
        fallback_transport: LlmTransport,
    ) {
        if let Ok(mut state) = self.0.lock() {
            state.identity = Some(LlmAttemptIdentity {
                conversation_id: telemetry.conversation_id.clone(),
                root_conversation_id: telemetry.root_conversation_id.clone(),
                request_id: telemetry.request_id.clone(),
                retry_attempt: telemetry.retry_attempt,
                provider: provider.to_string(),
                model: model.to_string(),
                fallback_transport,
                started_at: std::time::Instant::now(),
            });
        }
    }

    #[must_use]
    pub fn finalize_cancelled(&self) -> Option<LlmAttemptMetrics> {
        let mut state = self.0.lock().ok()?;
        if let Some(metrics) = state.finalized.clone() {
            return Some(metrics);
        }
        let identity = state.identity.clone()?;
        let metrics = LlmAttemptMetrics {
            conversation_id: identity.conversation_id,
            root_conversation_id: identity.root_conversation_id,
            request_id: identity.request_id,
            retry_attempt: identity.retry_attempt,
            provider: identity.provider,
            model: identity.model,
            transport: state.transport.unwrap_or(identity.fallback_transport),
            total_duration_ms: u64::try_from(identity.started_at.elapsed().as_millis())
                .unwrap_or(u64::MAX),
            stream: state
                .progress
                .clone()
                .unwrap_or_else(ProviderStreamTelemetry::non_streaming),
            outcome: LlmAttemptOutcome::Cancelled,
        };
        state.finalized = Some(metrics.clone());
        Some(metrics)
    }

    #[must_use]
    pub fn progress(&self) -> Option<ProviderStreamTelemetry> {
        self.0.lock().ok()?.progress.clone()
    }

    #[must_use]
    pub fn finalized(&self) -> Option<LlmAttemptMetrics> {
        self.0.lock().ok()?.finalized.clone()
    }

    #[must_use]
    pub fn finalize(&self, finalization: LlmAttemptFinalization) -> Option<LlmAttemptMetrics> {
        let mut state = self.0.lock().ok()?;
        let identity = state.identity.clone()?;
        let metrics = LlmAttemptMetrics {
            conversation_id: identity.conversation_id,
            root_conversation_id: identity.root_conversation_id,
            request_id: identity.request_id,
            retry_attempt: identity.retry_attempt,
            provider: identity.provider,
            model: identity.model,
            transport: state.transport.unwrap_or(identity.fallback_transport),
            total_duration_ms: u64::try_from(identity.started_at.elapsed().as_millis())
                .unwrap_or(u64::MAX),
            stream: finalization
                .stream
                .or_else(|| state.progress.clone())
                .unwrap_or_else(ProviderStreamTelemetry::non_streaming),
            outcome: finalization.outcome,
        };
        state.finalized = Some(metrics.clone());
        Some(metrics)
    }
}

impl ProviderStreamTelemetry {
    #[must_use]
    pub const fn non_streaming() -> Self {
        Self {
            dispatch_to_first_provider_event_ms: None,
            dispatch_to_first_generation_event_ms: None,
            dispatch_to_first_visible_text_ms: None,
            provider_event_count: 0,
            generation_event_count: 0,
            visible_text_event_count: 0,
            max_provider_gap_ms: None,
            max_generation_gap_ms: None,
            output_kind: StreamTelemetryOutputKind::None,
            completed: false,
        }
    }
}

/// LLM request
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub system: Vec<SystemContent>,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
    pub effective_effort: EffectiveEffort,
    pub telemetry: Option<LlmRequestTelemetry>,
    /// Required cache key. See [`PromptCacheKey`] for how to pick one — the
    /// choice is the caller's because only the caller knows its caching
    /// cohort. Used as `prompt_cache_key` on the `OpenAI` Responses path,
    /// ignored by `Anthropic`.
    pub cache_key: PromptCacheKey,
}

impl LlmRequest {
    #[must_use]
    pub fn reserved_output_tokens(&self) -> u32 {
        self.raised_output_token_ceiling().unwrap_or_else(|| {
            if self
                .effective_effort
                .level()
                .is_some_and(ModelEffort::needs_extended_output_headroom)
            {
                64_000
            } else {
                16_384
            }
        })
    }

    #[must_use]
    pub const fn raised_output_token_ceiling(&self) -> Option<u32> {
        self.max_tokens
    }
}

/// System prompt content
#[derive(Debug, Clone)]
/// A system segment's `cache` flag is an Anthropic cache anchor. `OpenAI`'s
/// instructions field cannot carry a breakpoint; its explicit markers are
/// placed only on supported message content blocks during translation.
pub struct SystemContent {
    pub text: String,
    pub cache: bool,
}

impl SystemContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: false,
        }
    }

    pub fn cached(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: true,
        }
    }
}

/// Message in conversation
#[derive(Debug, Clone)]
pub struct LlmMessage {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

/// Content block in a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        /// Images to include in the tool result.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageSource>,
        #[serde(default)]
        is_error: bool,
    },

    // ---- Server-handled blocks (Anthropic) ----
    // These blocks are executed by the API, not by Phoenix. They MUST be
    // preserved in conversation history for multi-turn correctness (e.g.
    // tool search discovers deferred tools on turn N; turn N+1 needs the
    // server_tool_use + tool_search_tool_result blocks in history or the
    // API returns 400).
    /// Server-side tool invocation (tool search, web search, code execution).
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool search result -- contains references to discovered deferred tools.
    ToolSearchToolResult {
        tool_use_id: String,
        content: ToolSearchResultContent,
    },
    /// Web search result -- opaque round-trip.
    WebSearchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// Web fetch result -- opaque round-trip.
    WebFetchToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// Code execution result (legacy) -- opaque round-trip.
    CodeExecutionToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// Bash code execution result -- opaque round-trip.
    BashCodeExecutionToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// Text editor code execution result -- opaque round-trip.
    TextEditorCodeExecutionToolResult {
        tool_use_id: String,
        content: serde_json::Value,
    },
    /// MCP tool invocation (Anthropic MCP connector, beta) -- opaque round-trip.
    McpToolUse {
        id: String,
        name: String,
        server_name: String,
        input: serde_json::Value,
    },
    /// MCP tool result (Anthropic MCP connector, beta) -- opaque round-trip.
    McpToolResult {
        tool_use_id: String,
        #[serde(default)]
        is_error: bool,
        content: serde_json::Value,
    },
}

impl ContentBlock {
    /// Render this block to plain searchable/readable text — the single source
    /// of truth shared by the retrieval index extractor and the chain Q&A read
    /// path, so the two never disagree on what a block's text is. Exhaustive on
    /// purpose (no wildcard): a new variant must be given a rendering here
    /// rather than silently vanishing from search and reads.
    #[must_use]
    pub fn render_text(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::ToolUse { name, input, .. } => format!("[tool call: {name} {input}]"),
            Self::ServerToolUse { name, input, .. } => {
                format!("[server tool call: {name} {input}]")
            }
            Self::McpToolUse {
                name,
                server_name,
                input,
                ..
            } => format!("[mcp tool call: {server_name}/{name} {input}]"),
            Self::ToolSearchToolResult { content, .. } => format!(
                "[tool search result: {}]",
                serde_json::to_string(content).unwrap_or_default()
            ),
            Self::WebSearchToolResult { content, .. } => format!("[web search result: {content}]"),
            Self::WebFetchToolResult { content, .. } => format!("[web fetch result: {content}]"),
            Self::CodeExecutionToolResult { content, .. } => {
                format!("[code execution result: {content}]")
            }
            Self::BashCodeExecutionToolResult { content, .. } => {
                format!("[bash execution result: {content}]")
            }
            Self::TextEditorCodeExecutionToolResult { content, .. } => {
                format!("[text editor execution result: {content}]")
            }
            Self::McpToolResult {
                content, is_error, ..
            } => format!(
                "[mcp tool result{}: {content}]",
                if *is_error { " (error)" } else { "" }
            ),
            // Images carry no text; a generic marker keeps the gap visible.
            Self::Image { .. } => "[image]".to_string(),
            // Results live in the following user message, not the assistant
            // block — but if one ever appears here, render its text.
            Self::ToolResult { content, .. } => content.clone(),
        }
    }
}

/// Content of a `tool_search_tool_result` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSearchResultContent {
    pub r#type: String, // "tool_search_tool_search_result" or "tool_search_tool_result_error"
    #[serde(default)]
    pub tool_references: Vec<ToolReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// A single tool reference inside a `ToolSearchResultContent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolReference {
    pub r#type: String, // "tool_reference"
    pub tool_name: String,
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }

    /// Wire discriminant for this block — the same string serde emits as the
    /// `type` tag. Single source of truth for the variant name in logs and
    /// diagnostics; `prop_content_block_type_tag_valid` asserts it stays in
    /// lockstep with the serde output.
    #[must_use]
    pub fn type_tag(&self) -> &'static str {
        match self {
            ContentBlock::Text { .. } => "text",
            ContentBlock::Image { .. } => "image",
            ContentBlock::ToolUse { .. } => "tool_use",
            ContentBlock::ToolResult { .. } => "tool_result",
            ContentBlock::ServerToolUse { .. } => "server_tool_use",
            ContentBlock::ToolSearchToolResult { .. } => "tool_search_tool_result",
            ContentBlock::WebSearchToolResult { .. } => "web_search_tool_result",
            ContentBlock::WebFetchToolResult { .. } => "web_fetch_tool_result",
            ContentBlock::CodeExecutionToolResult { .. } => "code_execution_tool_result",
            ContentBlock::BashCodeExecutionToolResult { .. } => "bash_code_execution_tool_result",
            ContentBlock::TextEditorCodeExecutionToolResult { .. } => {
                "text_editor_code_execution_tool_result"
            }
            ContentBlock::McpToolUse { .. } => "mcp_tool_use",
            ContentBlock::McpToolResult { .. } => "mcp_tool_result",
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }
}

/// Image source
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
}

/// Tool definition
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// Whether this tool should use Anthropic's deferred loading (zero context
    /// tokens until discovered via tool search).
    pub defer_loading: bool,
}

/// LLM response
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub end_turn: bool,
    pub usage: Usage,
    pub stream_telemetry: ProviderStreamTelemetry,
}

impl LlmResponse {
    #[must_use]
    pub fn non_streaming(content: Vec<ContentBlock>, end_turn: bool, usage: Usage) -> Self {
        Self {
            content,
            end_turn,
            usage,
            stream_telemetry: ProviderStreamTelemetry::non_streaming(),
        }
    }

    #[must_use]
    pub fn with_stream_telemetry(mut self, stream_telemetry: ProviderStreamTelemetry) -> Self {
        self.stream_telemetry = stream_telemetry;
        self
    }

    /// Extract all tool use requests from the response
    #[must_use]
    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                ContentBlock::Image { .. }
                | ContentBlock::Text { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::WebSearchToolResult { .. }
                | ContentBlock::WebFetchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. }
                | ContentBlock::BashCodeExecutionToolResult { .. }
                | ContentBlock::TextEditorCodeExecutionToolResult { .. }
                | ContentBlock::McpToolUse { .. }
                | ContentBlock::McpToolResult { .. } => None,
            })
            .collect()
    }

    /// Get text content from the response
    #[must_use]
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Image { .. }
                | ContentBlock::ToolUse { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::ServerToolUse { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::WebSearchToolResult { .. }
                | ContentBlock::WebFetchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. }
                | ContentBlock::BashCodeExecutionToolResult { .. }
                | ContentBlock::TextEditorCodeExecutionToolResult { .. }
                | ContentBlock::McpToolUse { .. }
                | ContentBlock::McpToolResult { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Usage statistics
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[allow(clippy::struct_field_names)] // tokens suffix is meaningful
#[ts(export, export_to = "../../../ui/src/generated/", rename = "UsageData")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    // owned: pre-feature persisted usage blobs had no reasoning token count.
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

impl Usage {
    #[must_use]
    pub fn context_window_used(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

// ContentBlock serde and tool_uses() invariants are covered by property tests
// in src/llm/proptests.rs: prop_content_block_serde_round_trip,
// prop_content_block_type_tag_valid, prop_tool_uses_only_returns_tool_use.

#[cfg(test)]
mod attempt_capture_tests {
    use super::*;

    #[test]
    fn historical_usage_without_reasoning_tokens_deserializes_losslessly() {
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 5
        }))
        .unwrap();
        assert_eq!(usage.reasoning_tokens, None);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn explicit_utility_cap_is_not_raised_by_effective_effort() {
        let request = LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            max_tokens: Some(50),
            effective_effort: EffectiveEffort::native_known(ModelEffort::Max),
            telemetry: None,
            cache_key: PromptCacheKey::ephemeral(),
        };
        assert_eq!(request.raised_output_token_ceiling(), Some(50));
        assert_eq!(request.reserved_output_tokens(), 50);
    }

    #[test]
    fn pre_dispatch_cancellation_is_not_a_provider_attempt() {
        let capture = LlmAttemptCapture::new();

        assert_eq!(capture.finalize_cancelled(), None);
        assert_eq!(capture.finalized(), None);
    }

    #[test]
    fn finalization_without_provider_dispatch_is_not_an_attempt() {
        let capture = LlmAttemptCapture::new();

        assert_eq!(
            capture.finalize(LlmAttemptFinalization {
                stream: None,
                outcome: LlmAttemptOutcome::AuthError,
            }),
            None
        );
        assert_eq!(capture.finalized(), None);
    }

    #[test]
    fn cancelled_attempt_preserves_partial_provider_progress() {
        let capture = LlmAttemptCapture::new();
        let telemetry = LlmRequestTelemetry {
            conversation_id: "conv".to_string(),
            root_conversation_id: "root".to_string(),
            request_id: "request".to_string(),
            retry_attempt: 2,
            attempt_capture: capture.clone(),
        };
        capture.begin(&telemetry, "openai", "gpt-test", LlmTransport::HttpSse);
        capture.set_transport(LlmTransport::Websocket);
        capture.publish_progress(ProviderStreamTelemetry {
            dispatch_to_first_provider_event_ms: Some(10),
            dispatch_to_first_generation_event_ms: Some(20),
            dispatch_to_first_visible_text_ms: None,
            provider_event_count: 3,
            generation_event_count: 1,
            visible_text_event_count: 0,
            max_provider_gap_ms: Some(10),
            max_generation_gap_ms: None,
            output_kind: StreamTelemetryOutputKind::Reasoning,
            completed: false,
        });

        let metrics = capture.finalize_cancelled().expect("started attempt");
        assert_eq!(metrics.outcome, LlmAttemptOutcome::Cancelled);
        assert_eq!(metrics.transport, LlmTransport::Websocket);
        assert_eq!(metrics.stream.provider_event_count, 3);
        assert_eq!(
            metrics.stream.dispatch_to_first_generation_event_ms,
            Some(20)
        );
        assert!(!metrics.stream.completed);
    }
}
