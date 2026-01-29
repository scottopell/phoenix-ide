//! Conversation state types

use crate::db::{ErrorKind, SubAgentResult, ToolResult};
use crate::tools::patch::types::PatchInput;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

// ============================================================================
// Tool Input Types - Strongly typed inputs for each tool
// ============================================================================

/// Execution mode for bash commands
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BashMode {
    #[default]
    Default,
    Slow,
    Background,
}

/// Input for the bash tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashInput {
    pub command: String,
    #[serde(default)]
    pub mode: BashMode,
}

/// Input for the think tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkInput {
    pub thoughts: String,
}

/// Input for the `keyword_search` tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeywordSearchInput {
    pub query: String,
    pub search_terms: Vec<String>,
}

/// Input for the `read_image` tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadImageInput {
    pub path: String,
}

/// Strongly typed tool input enum
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_tool", rename_all = "snake_case")]
pub enum ToolInput {
    Bash(BashInput),
    Think(ThinkInput),
    Patch(PatchInput),
    KeywordSearch(KeywordSearchInput),
    ReadImage(ReadImageInput),
    /// Fallback for unknown tools or parsing failures
    Unknown {
        name: String,
        input: Value,
    },
}

impl ToolInput {
    /// Get the tool name
    pub fn tool_name(&self) -> &str {
        match self {
            ToolInput::Bash(_) => "bash",
            ToolInput::Think(_) => "think",
            ToolInput::Patch(_) => "patch",
            ToolInput::KeywordSearch(_) => "keyword_search",
            ToolInput::ReadImage(_) => "read_image",
            ToolInput::Unknown { name, .. } => name,
        }
    }

    /// Convert to JSON Value for tool execution
    pub fn to_value(&self) -> Value {
        match self {
            ToolInput::Bash(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Think(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Patch(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::KeywordSearch(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::ReadImage(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Unknown { input, .. } => input.clone(),
        }
    }

    /// Parse from tool name and JSON value
    pub fn from_name_and_value(name: &str, value: Value) -> Self {
        match name {
            "bash" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::Bash,
            ),
            "think" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::Think,
            ),
            "patch" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::Patch,
            ),
            "keyword_search" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::KeywordSearch,
            ),
            "read_image" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::ReadImage,
            ),
            _ => ToolInput::Unknown {
                name: name.to_string(),
                input: value,
            },
        }
    }
}

// ============================================================================
// Tool Call - A tool invocation with ID and typed input
// ============================================================================

/// A tool call from the LLM with typed input
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub input: ToolInput,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, input: ToolInput) -> Self {
        Self {
            id: id.into(),
            input,
        }
    }

    /// Get the tool name
    pub fn name(&self) -> &str {
        self.input.tool_name()
    }
}

// ============================================================================
// Conversation State
// ============================================================================

/// Conversation state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum ConvState {
    /// Ready for user input, no pending operations
    #[default]
    Idle,

    /// User message received, preparing LLM request
    AwaitingLlm,

    /// LLM request in flight, with retry tracking
    LlmRequesting { attempt: u32 },

    /// Executing tools serially
    ToolExecuting {
        /// The current tool being executed
        current_tool: ToolCall,
        /// Remaining tools to execute after current completes
        remaining_tools: Vec<ToolCall>,
        /// Results from completed tools
        #[serde(default)]
        completed_results: Vec<ToolResult>,
    },

    /// User requested cancellation of LLM request, waiting for response to discard
    CancellingLlm,

    /// User requested cancellation of tool execution, waiting for abort confirmation
    CancellingTool {
        /// The tool being aborted
        tool_use_id: String,
        /// Tools that were skipped
        skipped_tools: Vec<ToolCall>,
        /// Results from tools that completed before cancel
        completed_results: Vec<ToolResult>,
    },

    /// Waiting for sub-agents to complete
    AwaitingSubAgents {
        pending_ids: Vec<String>,
        #[serde(default)]
        completed_results: Vec<SubAgentResult>,
    },

    /// Error occurred - UI displays this state directly
    Error {
        message: String,
        error_kind: ErrorKind,
    },
}

impl ConvState {
    /// Check if this is a terminal state (conversation should stop processing)
    #[allow(dead_code, clippy::unused_self)] // State query utility
    pub fn is_terminal(&self) -> bool {
        false // Conversations can always be continued from any state
    }

    /// Check if agent is currently working
    #[allow(dead_code, clippy::unused_self)] // State query utility
    pub fn is_working(&self) -> bool {
        !matches!(self, ConvState::Idle | ConvState::Error { .. })
    }

    /// Convert to database state string
    pub fn to_db_state(&self) -> &'static str {
        match self {
            ConvState::Idle => "idle",
            ConvState::AwaitingLlm => "awaiting_llm",
            ConvState::LlmRequesting { .. } => "llm_requesting",
            ConvState::ToolExecuting { .. } => "tool_executing",
            ConvState::CancellingLlm => "cancelling",
            ConvState::CancellingTool { .. } => "cancelling",
            ConvState::AwaitingSubAgents { .. } => "awaiting_sub_agents",
            ConvState::Error { .. } => "error",
        }
    }
}

/// Context for a conversation (immutable configuration)
#[derive(Debug, Clone)]
pub struct ConvContext {
    pub conversation_id: String,
    pub working_dir: PathBuf,
    #[allow(dead_code)] // Used by LLM client selection
    pub model_id: String,
    #[allow(dead_code)] // Reserved for sub-agent feature
    pub is_sub_agent: bool,
}

impl ConvContext {
    pub fn new(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: false,
        }
    }

    #[allow(dead_code)] // Reserved for sub-agent feature
    pub fn into_sub_agent(mut self) -> Self {
        self.is_sub_agent = true;
        self
    }
}
