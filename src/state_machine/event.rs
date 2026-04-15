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
        /// When true, a recovery mechanism (e.g. credential helper) is actively
        /// running and may resolve this error. The transition function uses this
        /// to choose `AwaitingRecovery` vs `Error` (REQ-BED-030).
        recovery_in_progress: bool,
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

    /// Grace turn exhausted -- sub-agent used its extra turn without calling `submit_result`.
    /// The executor extracted the last assistant text (if any) before sending this event.
    GraceTurnExhausted {
        /// The partial result extracted from the last assistant text, or None if no text found.
        result: Option<String>,
    },

    // Recovery events (REQ-BED-030)
    /// Credential helper succeeded — conversations in `AwaitingRecovery` should retry.
    #[allow(dead_code)]
    // Constructed by executor in Phase 2 (credential helper settlement wiring)
    CredentialBecameAvailable,
    /// Credential helper failed — conversations in `AwaitingRecovery` transition to `Error`.
    #[allow(dead_code)]
    // Constructed by executor in Phase 2 (credential helper settlement wiring)
    CredentialHelperFailed {
        message: String,
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

impl Event {
    /// Stable, payload-free name of this event variant. Used by structured
    /// error types (e.g. `TransitionError::InvalidTransition`) and tracing
    /// so they can carry a discriminator without the `Debug` format of the
    /// variant's payloads — task 24682 follow-up. Single source of truth.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Event::UserMessage { .. } => "UserMessage",
            Event::UserCancel { .. } => "UserCancel",
            Event::LlmResponse { .. } => "LlmResponse",
            Event::LlmError { .. } => "LlmError",
            Event::RetryTimeout { .. } => "RetryTimeout",
            Event::ToolComplete { .. } => "ToolComplete",
            Event::ToolAborted { .. } => "ToolAborted",
            Event::SpawnAgentsComplete { .. } => "SpawnAgentsComplete",
            Event::SubAgentResult { .. } => "SubAgentResult",
            Event::ContinuationResponse { .. } => "ContinuationResponse",
            Event::ContinuationFailed { .. } => "ContinuationFailed",
            Event::UserTriggerContinuation => "UserTriggerContinuation",
            Event::TaskApprovalResponse { .. } => "TaskApprovalResponse",
            Event::UserQuestionResponse { .. } => "UserQuestionResponse",
            Event::GraceTurnExhausted { .. } => "GraceTurnExhausted",
            Event::CredentialBecameAvailable => "CredentialBecameAvailable",
            Event::CredentialHelperFailed { .. } => "CredentialHelperFailed",
            Event::TaskResolved { .. } => "TaskResolved",
        }
    }
}
