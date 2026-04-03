//! Conversation state types

use crate::db::{ErrorKind, ToolResult, UsageData};
use crate::llm::ContentBlock;
use crate::tools::patch::types::PatchInput;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

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

/// Task specification for `spawn_agents` tool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubAgentTask {
    pub task: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Input for the `spawn_agents` tool (parent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgentsInput {
    pub tasks: Vec<SubAgentTask>,
}

/// Input for the `submit_result` tool (sub-agent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitResultInput {
    pub result: String,
}

/// Input for the `submit_error` tool (sub-agent only)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitErrorInput {
    pub error: String,
}

/// Input for the `propose_task` tool (task approval workflow)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposeTaskInput {
    pub title: String,
    pub priority: String,
    pub plan: String,
}

/// A single question presented to the user (REQ-AUQ-001)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// An option within a user question
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Annotations the user can attach to an answer
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnnotation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Input for the `ask_user_question` tool (REQ-AUQ-001)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    pub questions: Vec<UserQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<QuestionMetadata>,
}

/// Optional metadata for an `ask_user_question` invocation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
    ProposeTask(ProposeTaskInput),
    AskUserQuestion(AskUserQuestionInput),
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
            ToolInput::ProposeTask(_) => "propose_task",
            ToolInput::AskUserQuestion(_) => "ask_user_question",
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
            ToolInput::ProposeTask(input) => serde_json::to_value(input).unwrap_or(Value::Null),
            ToolInput::AskUserQuestion(input) => serde_json::to_value(input).unwrap_or(Value::Null),
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
            "propose_task" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::ProposeTask,
            ),
            "ask_user_question" => serde_json::from_value(value.clone()).map_or_else(
                |_| ToolInput::Unknown {
                    name: name.to_string(),
                    input: value,
                },
                ToolInput::AskUserQuestion,
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
// Assistant Message — bundled representation for atomic persistence
// ============================================================================

/// An LLM assistant message held in state until persistence.
/// Bundles content, display metadata, usage stats, and message ID so they
/// cannot be partially threaded or forgotten.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub message_id: String,
    pub content: Vec<ContentBlock>,
    pub usage: Option<UsageData>,
    pub display_data: Option<Value>,
}

impl AssistantMessage {
    pub fn new(
        content: Vec<ContentBlock>,
        usage: Option<UsageData>,
        display_data: Option<Value>,
    ) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().to_string(),
            content,
            usage,
            display_data,
        }
    }

    /// Returns references to the `ToolUse` blocks in content.
    /// Used by `CheckpointData::tool_round()` to enforce the matching-count invariant.
    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .collect()
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

    /// Executing tools serially.
    /// The assistant message is held here (NOT yet persisted) — persistence is atomic
    /// at the end of the tool round via `CheckpointData::ToolRound` (REQ-BED-007).
    ToolExecuting {
        /// The current tool being executed
        current_tool: ToolCall,
        /// Remaining tools to execute after current completes
        remaining_tools: Vec<ToolCall>,
        /// Completed tool results — single source of truth (FM-4 Prevention).
        /// No parallel `persisted_tool_ids` tracking set.
        #[serde(default)]
        completed_results: Vec<ToolResult>,
        /// Sub-agents spawned during this tool execution phase
        #[serde(default)]
        pending_sub_agents: Vec<PendingSubAgent>,
        /// Assistant message held until all tools complete (not yet persisted)
        #[serde(default)]
        assistant_message: AssistantMessage,
    },

    /// User requested cancellation of tool execution, waiting for abort confirmation.
    /// Carries the assistant message and completed results so the checkpoint can
    /// be persisted atomically on abort.
    CancellingTool {
        /// The tool being aborted
        tool_use_id: String,
        /// Tools that were skipped
        skipped_tools: Vec<ToolCall>,
        /// Tool results completed before cancellation
        completed_results: Vec<ToolResult>,
        /// Assistant message held for atomic persistence
        assistant_message: AssistantMessage,
    },

    /// Waiting for sub-agents to complete
    AwaitingSubAgents {
        /// Sub-agents still running (id + task co-located)
        pending: Vec<PendingSubAgent>,
        #[serde(default)]
        completed_results: Vec<SubAgentResult>,
        /// `tool_use_id` of the `spawn_agents` call (to update `display_data` when done)
        #[serde(default)]
        spawn_tool_id: Option<String>,
    },

    /// User requested cancellation while waiting for sub-agents
    CancellingSubAgents {
        /// Sub-agents still running (id + task co-located)
        pending: Vec<PendingSubAgent>,
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

    /// Awaiting continuation summary from LLM (tool-less request in flight)
    AwaitingContinuation {
        /// Tool calls that were requested but not executed
        rejected_tool_calls: Vec<ToolCall>,
        /// Retry attempt for the continuation request
        attempt: u32,
    },

    /// Awaiting user approval of a proposed task plan (REQ-BED-028)
    AwaitingTaskApproval {
        title: String,
        priority: String,
        plan: String,
    },

    /// Awaiting user answers to clarifying questions (REQ-AUQ-001).
    /// `ask_user_question` must be the sole tool in a response, so there are
    /// no remaining tools or persisted tool IDs to carry.
    AwaitingUserResponse {
        questions: Vec<UserQuestion>,
        tool_use_id: String,
    },

    /// Context window exhausted - conversation is read-only
    ContextExhausted {
        /// The continuation summary
        summary: String,
    },

    /// Task lifecycle completed or abandoned — conversation is permanently read-only.
    /// Rejects all events. Preserved on server restart (not reset to Idle).
    Terminal,
}

/// Outcome of user's decision on a proposed task plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskApprovalOutcome {
    Approved,
    Rejected,
    FeedbackProvided { annotations: String },
}

/// Semantic state category for UI display.
///
/// Single source of truth for how conversation states map to visual indicators.
/// The API serializes this so the UI never re-derives state categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayState {
    /// Ready for user input (green dot, static)
    Idle,
    /// Agent is processing (yellow dot, pulsing)
    Working,
    /// Retryable error occurred (red dot)
    Error,
    /// Conversation cannot continue — context exhausted, completed, or failed (gray dot, static)
    Terminal,
    /// Awaiting user action on a proposed task plan (REQ-BED-028)
    AwaitingApproval,
}

impl DisplayState {
    pub fn as_str(self) -> &'static str {
        match self {
            DisplayState::Idle => "idle",
            DisplayState::Working => "working",
            DisplayState::Error => "error",
            DisplayState::Terminal => "terminal",
            DisplayState::AwaitingApproval => "awaiting_approval",
        }
    }
}

/// Executor lifecycle signal — forces explicit handling of terminal states (FM-5 prevention).
///
/// The executor loop checks this after every transition. `Terminal` means the loop
/// must exit — no reliance on channel-drop semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum StepResult {
    Continue,
    Terminal(TerminalOutcome),
}

/// Why the executor is exiting.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutcome {
    /// Sub-agent completed successfully
    Completed(String),
    /// Sub-agent or conversation failed
    Failed(String, ErrorKind),
    /// Context window exhausted — conversation is read-only
    ContextExhausted { summary: String },
    /// Task lifecycle ended (complete or abandon) — conversation is permanently read-only
    TaskResolved,
}

impl ConvState {
    /// Check if this is a terminal state — cannot transition out.
    /// `Completed`/`Failed` are sub-agent specific; `Terminal` is the
    /// user-facing lifecycle end state (complete/abandon).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ConvState::Completed { .. } | ConvState::Failed { .. } | ConvState::Terminal
        )
    }

    /// Structural terminal-state check for the executor loop.
    ///
    /// Returns `StepResult::Terminal` for states that cannot produce further transitions,
    /// forcing the executor to exit explicitly rather than relying on channel lifecycle.
    pub fn step_result(&self) -> StepResult {
        match self {
            ConvState::Completed { result } => {
                StepResult::Terminal(TerminalOutcome::Completed(result.clone()))
            }
            ConvState::Failed { error, error_kind } => {
                StepResult::Terminal(TerminalOutcome::Failed(error.clone(), error_kind.clone()))
            }
            ConvState::ContextExhausted { summary, .. } => {
                StepResult::Terminal(TerminalOutcome::ContextExhausted {
                    summary: summary.clone(),
                })
            }
            ConvState::Terminal => StepResult::Terminal(TerminalOutcome::TaskResolved),
            ConvState::Idle
            | ConvState::AwaitingLlm
            | ConvState::LlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::Error { .. }
            | ConvState::AwaitingContinuation { .. }
            | ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. } => StepResult::Continue,
        }
    }

    /// Semantic category for UI display. This is the single source of truth
    /// for mapping raw conversation states to visual indicators.
    pub fn display_state(&self) -> DisplayState {
        match self {
            ConvState::Idle => DisplayState::Idle,
            ConvState::Error { .. } => DisplayState::Error,
            ConvState::AwaitingTaskApproval { .. } | ConvState::AwaitingUserResponse { .. } => {
                DisplayState::AwaitingApproval
            }
            ConvState::ContextExhausted { .. }
            | ConvState::Completed { .. }
            | ConvState::Failed { .. }
            | ConvState::Terminal => DisplayState::Terminal,
            ConvState::AwaitingLlm
            | ConvState::LlmRequesting { .. }
            | ConvState::ToolExecuting { .. }
            | ConvState::CancellingTool { .. }
            | ConvState::AwaitingSubAgents { .. }
            | ConvState::CancellingSubAgents { .. }
            | ConvState::AwaitingContinuation { .. } => DisplayState::Working,
        }
    }
}

// ============================================================================
// Sub-Agent Types
// ============================================================================

/// Outcome of a sub-agent execution - pit of success design
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubAgentOutcome {
    Success {
        result: String,
    },
    Failure {
        error: String,
        error_kind: ErrorKind,
    },
    /// Sub-agent exceeded its time limit (REQ-SA-006)
    TimedOut,
}

/// A sub-agent that is still running
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingSubAgent {
    pub agent_id: String,
    pub task: String,
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
    /// Mandatory timeout — caller must make a conscious decision (REQ-SA-006)
    pub timeout: Duration,
}

/// How a conversation handles approaching context limits
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextExhaustionBehavior {
    /// Normal conversations: trigger continuation at 90% threshold
    #[default]
    ThresholdBasedContinuation,
    /// Sub-agents: fail immediately (no continuation flow)
    IntentionallyUnhandled,
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
    /// Model's context window size in tokens
    pub context_window: usize,
    /// How this conversation handles context exhaustion
    pub context_exhaustion_behavior: ContextExhaustionBehavior,
    /// Conversation mode context for system prompt (stable per mode, updated on Explore->Work)
    pub mode_context: Option<crate::system_prompt::ModeContext>,
}

/// Default context window for unknown models (conservative)
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

impl ConvContext {
    pub fn new(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
        context_window: usize,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: false,
            context_window,
            context_exhaustion_behavior: ContextExhaustionBehavior::ThresholdBasedContinuation,
            mode_context: None,
        }
    }

    /// Create a sub-agent context
    pub fn sub_agent(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
        context_window: usize,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: true,
            context_window,
            context_exhaustion_behavior: ContextExhaustionBehavior::IntentionallyUnhandled,
            mode_context: None,
        }
    }
}
