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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that ContentBlock server variants serialize to the correct
    /// `type` tag strings and round-trip through JSON.
    #[test]
    fn content_block_server_variants_serde_round_trip() {
        let blocks = vec![
            ContentBlock::ServerToolUse {
                id: "srvtoolu_123".into(),
                name: "tool_search_tool_regex".into(),
                input: serde_json::json!({"query": "weather"}),
            },
            ContentBlock::ToolSearchToolResult {
                tool_use_id: "srvtoolu_123".into(),
                content: ToolSearchResultContent {
                    r#type: "tool_search_tool_search_result".into(),
                    tool_references: vec![ToolReference {
                        r#type: "tool_reference".into(),
                        tool_name: "get_weather".into(),
                    }],
                    error_code: None,
                },
            },
            ContentBlock::WebSearchToolResult {
                tool_use_id: "srvtoolu_456".into(),
                content: serde_json::json!({"type": "web_search_result"}),
            },
            ContentBlock::WebFetchToolResult {
                tool_use_id: "srvtoolu_789".into(),
                content: serde_json::json!({"type": "web_fetch_result"}),
            },
            ContentBlock::McpToolUse {
                id: "mcptoolu_abc".into(),
                name: "slack_send".into(),
                server_name: "slack".into(),
                input: serde_json::json!({"channel": "#general"}),
            },
            ContentBlock::McpToolResult {
                tool_use_id: "mcptoolu_abc".into(),
                is_error: false,
                content: serde_json::json!([{"type": "text", "text": "sent"}]),
            },
        ];

        for block in &blocks {
            let json = serde_json::to_value(block).expect("serialize");

            // Every block must have a "type" field
            let type_str = json.get("type").and_then(|v| v.as_str())
                .expect("missing type field");

            // Round-trip: deserialize back and compare
            let round_tripped: ContentBlock =
                serde_json::from_value(json.clone()).unwrap_or_else(|e| {
                    panic!("failed to deserialize {type_str}: {e}\njson: {json}")
                });
            assert_eq!(
                block, &round_tripped,
                "round-trip mismatch for {type_str}"
            );
        }
    }

    /// Verify the exact type tag strings match what the Anthropic API expects.
    #[test]
    fn content_block_type_tag_strings() {
        let cases: Vec<(ContentBlock, &str)> = vec![
            (
                ContentBlock::ServerToolUse {
                    id: "x".into(),
                    name: "y".into(),
                    input: serde_json::json!({}),
                },
                "server_tool_use",
            ),
            (
                ContentBlock::ToolSearchToolResult {
                    tool_use_id: "x".into(),
                    content: ToolSearchResultContent {
                        r#type: "tool_search_tool_search_result".into(),
                        tool_references: vec![],
                        error_code: None,
                    },
                },
                "tool_search_tool_result",
            ),
            (
                ContentBlock::WebSearchToolResult {
                    tool_use_id: "x".into(),
                    content: serde_json::json!({}),
                },
                "web_search_tool_result",
            ),
            (
                ContentBlock::WebFetchToolResult {
                    tool_use_id: "x".into(),
                    content: serde_json::json!({}),
                },
                "web_fetch_tool_result",
            ),
            (
                ContentBlock::CodeExecutionToolResult {
                    tool_use_id: "x".into(),
                    content: serde_json::json!({}),
                },
                "code_execution_tool_result",
            ),
            (
                ContentBlock::BashCodeExecutionToolResult {
                    tool_use_id: "x".into(),
                    content: serde_json::json!({}),
                },
                "bash_code_execution_tool_result",
            ),
            (
                ContentBlock::TextEditorCodeExecutionToolResult {
                    tool_use_id: "x".into(),
                    content: serde_json::json!({}),
                },
                "text_editor_code_execution_tool_result",
            ),
            (
                ContentBlock::McpToolUse {
                    id: "x".into(),
                    name: "y".into(),
                    server_name: "z".into(),
                    input: serde_json::json!({}),
                },
                "mcp_tool_use",
            ),
            (
                ContentBlock::McpToolResult {
                    tool_use_id: "x".into(),
                    is_error: false,
                    content: serde_json::json!({}),
                },
                "mcp_tool_result",
            ),
        ];

        for (block, expected_type) in cases {
            let json = serde_json::to_value(&block).expect("serialize");
            let actual_type = json.get("type").and_then(|v| v.as_str()).unwrap();
            assert_eq!(
                actual_type, expected_type,
                "wrong type tag for {:?}",
                block
            );
        }
    }

    /// Verify that tool_uses() does NOT return ServerToolUse or McpToolUse --
    /// only regular ToolUse blocks should be executed by Phoenix.
    #[test]
    fn tool_uses_excludes_server_blocks() {
        let response = LlmResponse {
            content: vec![
                ContentBlock::Text { text: "hi".into() },
                ContentBlock::ServerToolUse {
                    id: "srvtoolu_1".into(),
                    name: "tool_search".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
                ContentBlock::McpToolUse {
                    id: "mcptoolu_1".into(),
                    name: "slack".into(),
                    server_name: "slack".into(),
                    input: serde_json::json!({}),
                },
            ],
            end_turn: false,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        };

        let uses = response.tool_uses();
        assert_eq!(uses.len(), 1, "should only return regular ToolUse");
        assert_eq!(uses[0].1, "bash");
    }
}
