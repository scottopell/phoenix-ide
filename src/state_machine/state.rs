//! Conversation state types

use crate::db::{ErrorKind, ToolResult};
use std::time::Duration;
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

/// Task specification for spawn_agents tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentTask {
    pub task: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Input for the spawn_agents tool (parent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgentsInput {
    pub tasks: Vec<SubAgentTask>,
}

/// Input for the submit_result tool (sub-agent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitResultInput {
    pub result: String,
}

/// Input for the submit_error tool (sub-agent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitErrorInput {
    pub error: String,
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
    SpawnAgents(SpawnAgentsInput),
    SubmitResult(SubmitResultInput),
    SubmitError(SubmitErrorInput),
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
            ToolInput::SpawnAgents(_) => "spawn_agents",
            ToolInput::SubmitResult(_) => "submit_result",
            ToolInput::SubmitError(_) => "submit_error",
            ToolInput::Unknown { name, .. } => name,
        }
    }

    /// Check if this is a sub-agent terminal tool
    pub fn is_terminal_tool(&self) -> bool {
        matches!(self, ToolInput::SubmitResult(_) | ToolInput::SubmitError(_))
    }

    /// Convert to JSON Value for tool execution
    pub fn to_value(&self) -> Value {
        match self {
            ToolInput::Bash(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Think(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::Patch(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::KeywordSearch(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::ReadImage(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::SpawnAgents(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::SubmitResult(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::SubmitError(input) => serde_json::to_value(input).unwrap_or(Value::Null),
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
            "spawn_agents" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::SpawnAgents,
            ),
            "submit_result" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::SubmitResult,
            ),
            "submit_error" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::SubmitError,
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
        /// Sub-agents spawned during this tool execution phase
        #[serde(default)]
        pending_sub_agents: Vec<String>,
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

    /// User requested cancellation while waiting for sub-agents
    CancellingSubAgents {
        pending_ids: Vec<String>,
        #[serde(default)]
        completed_results: Vec<SubAgentResult>,
    },

    /// Sub-agent completed successfully (terminal state, sub-agent only)
    Completed { result: String },

    /// Sub-agent failed (terminal state, sub-agent only)
    Failed {
        error: String,
        error_kind: ErrorKind,
    },

    /// Error occurred - UI displays this state directly
    Error {
        message: String,
        error_kind: ErrorKind,
    },
}

impl ConvState {
    /// Check if this is a terminal state (sub-agent only - cannot transition out)
    pub fn is_terminal(&self) -> bool {
        matches!(self, ConvState::Completed { .. } | ConvState::Failed { .. })
    }

    /// Check if agent is currently working
    #[allow(dead_code, clippy::unused_self)] // State query utility
    pub fn is_working(&self) -> bool {
        !matches!(self, ConvState::Idle | ConvState::Error { .. })
    }

}

// ============================================================================
// Sub-Agent Types
// ============================================================================

/// Outcome of a sub-agent execution - pit of success design
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubAgentOutcome {
    Success { result: String },
    Failure { error: String, error_kind: ErrorKind },
}

/// Result from a completed sub-agent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub agent_id: String,
    pub task: String,
    pub outcome: SubAgentOutcome,
}

/// Specification for spawning a sub-agent (used in effects)
#[derive(Debug, Clone, PartialEq)]
pub struct SubAgentSpec {
    pub agent_id: String,
    pub task: String,
    pub cwd: String,
    pub timeout: Option<Duration>,
}

/// Context for a conversation (immutable configuration)
#[derive(Debug, Clone)]
pub struct ConvContext {
    pub conversation_id: String,
    pub working_dir: PathBuf,
    #[allow(dead_code)] // Used by LLM client selection
    pub model_id: String,
    /// Whether this is a sub-agent conversation
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

    /// Create a sub-agent context
    pub fn sub_agent(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: true,
        }
    }
}
