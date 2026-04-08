//! Events that can occur in a conversation

use crate::db::{ErrorKind, ImageData, ToolResult};
use crate::llm::{ContentBlock, Usage};
use crate::state_machine::state::{
    PendingSubAgent, QuestionAnnotation, SubAgentOutcome, TaskApprovalOutcome, ToolCall,
};
use std::collections::HashMap;

/// Events that trigger state transitions
#[derive(Debug, Clone)]
pub enum Event {
    // User events
    UserMessage {
        /// Display text — stored in DB and shown in history (REQ-IR-006).
        text: String,
        /// Expanded text delivered to the LLM when `@` references are present (REQ-IR-001).
        /// `None` means no expansion — `text` is used verbatim.
        llm_text: Option<String>,
        images: Vec<ImageData>,
        /// Client-generated UUID - the canonical identifier for this message
        message_id: String,
        /// Browser user agent for display (e.g., show iPhone icon in UI)
        user_agent: Option<String>,
        /// If this message triggered a skill invocation, the details are here.
        /// When present, the message is persisted as `MessageContent::Skill`.
        skill_invocation: Option<crate::skills::SkillInvocation>,
    },
    UserCancel {
        /// Why the cancel was issued. `None` means user-initiated or parent-propagated.
        reason: Option<String>,
    },

    // LLM events
    LlmResponse {
        content: Vec<ContentBlock>,
        /// Tool calls extracted from the content
        tool_calls: Vec<ToolCall>,
        #[allow(dead_code)] // Reserved for conversation flow control
        end_turn: bool,
        usage: Usage,
    },
    LlmError {
        message: String,
        error_kind: ErrorKind,
        #[allow(dead_code)] // Reserved for retry tracking
        attempt: u32,
    },
    RetryTimeout {
        attempt: u32,
    },

    // Tool events
    ToolComplete {
        tool_use_id: String,
        result: ToolResult,
    },
    /// Tool was aborted due to cancellation
    ToolAborted {
        tool_use_id: String,
    },

    // Sub-agent events
    /// `spawn_agents` tool completed, sub-agents are now running
    SpawnAgentsComplete {
        tool_use_id: String,
        /// Normal tool result for LLM context
        result: ToolResult,
        /// Spawned sub-agents with their tasks
        spawned: Vec<PendingSubAgent>,
    },
    /// A sub-agent has completed (success or failure)
    SubAgentResult {
        agent_id: String,
        outcome: SubAgentOutcome,
    },

    // Context continuation events (REQ-BED-019 through REQ-BED-024)
    /// Continuation summary received from LLM
    ContinuationResponse {
        summary: String,
    },
    /// Continuation request failed after retries
    ContinuationFailed {
        error: String,
    },
    /// User manually triggered continuation (REQ-BED-023)
    UserTriggerContinuation,

    // Task approval events (REQ-BED-028)
    /// User responded to a proposed task plan
    TaskApprovalResponse {
        outcome: TaskApprovalOutcome,
    },

    // Ask user question events (REQ-AUQ-001)
    /// User answered the pending questions (POST /api/conversations/{id}/respond)
    UserQuestionResponse {
        answers: HashMap<String, String>,
        annotations: Option<HashMap<String, QuestionAnnotation>>,
    },

    // Task resolution events (REQ-BED-029)
    /// Task completed or abandoned — transitions conversation to Terminal.
    /// Sent by the API handler after git operations succeed.
    TaskResolved {
        /// System message describing the outcome (e.g., "Task completed. Squash merged...")
        system_message: String,
        /// The repo root path to restore as cwd
        repo_root: String,
    },
}
