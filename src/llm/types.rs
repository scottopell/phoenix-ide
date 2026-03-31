//! Common types for LLM interactions

pub const LLM_SOURCE_HEADER: &str = "phoenix-ide";

use serde::{Deserialize, Serialize};

/// LLM request
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub system: Vec<SystemContent>,
    pub messages: Vec<LlmMessage>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
}

/// System prompt content
#[derive(Debug, Clone)]
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
        /// Images to include in the tool result (`Anthropic` only; `OpenAI` drops them).
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

    #[cfg(test)]
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
}

impl LlmResponse {
    /// Extract all tool use requests from the response
    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                _ => None,
            })
            .collect()
    }

    /// Get text content from the response
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Usage statistics
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)] // tokens suffix is meaningful
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

impl Usage {
    pub fn context_window_used(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

// ContentBlock serde and tool_uses() invariants are covered by property tests
// in src/llm/proptests.rs: prop_content_block_serde_round_trip,
// prop_content_block_type_tag_valid, prop_tool_uses_only_returns_tool_use.
