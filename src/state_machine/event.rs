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

// ============================================================================
// Split Event Types — CoreEvent, ParentOnlyEvent, SubAgentOnlyEvent
// ============================================================================

/// Events handled by the core transition function (shared by both parent and sub-agent).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum CoreEvent {
    UserMessage {
        text: String,
        llm_text: Option<String>,
        images: Vec<ImageData>,
        message_id: String,
        user_agent: Option<String>,
        skill_invocation: Option<crate::skills::SkillInvocation>,
    },
    UserCancel {
        reason: Option<String>,
    },
    LlmResponse {
        content: Vec<ContentBlock>,
        tool_calls: Vec<ToolCall>,
        end_turn: bool,
        usage: Usage,
    },
    LlmError {
        message: String,
        error_kind: ErrorKind,
        attempt: u32,
        recovery_in_progress: bool,
    },
    RetryTimeout {
        attempt: u32,
    },
    ToolComplete {
        tool_use_id: String,
        result: ToolResult,
    },
    ToolAborted {
        tool_use_id: String,
    },
    SpawnAgentsComplete {
        tool_use_id: String,
        result: ToolResult,
        spawned: Vec<PendingSubAgent>,
    },
    SubAgentResult {
        agent_id: String,
        outcome: SubAgentOutcome,
    },
    ContinuationResponse {
        summary: String,
    },
    ContinuationFailed {
        error: String,
    },
    UserTriggerContinuation,
}

/// Events only valid for parent conversations.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum ParentOnlyEvent {
    TaskApprovalResponse {
        outcome: TaskApprovalOutcome,
    },
    UserQuestionResponse {
        answers: HashMap<String, String>,
        annotations: Option<HashMap<String, QuestionAnnotation>>,
    },
    CredentialBecameAvailable,
    CredentialHelperFailed {
        message: String,
    },
    TaskResolved {
        system_message: String,
        repo_root: String,
    },
}

/// Events only valid for sub-agent conversations.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum SubAgentOnlyEvent {
    GraceTurnExhausted { result: Option<String> },
}

/// Combined event type for parent conversations.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum ParentEvent {
    Core(CoreEvent),
    Parent(ParentOnlyEvent),
}

/// Combined event type for sub-agent conversations.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum SubAgentEvent {
    Core(CoreEvent),
    SubAgent(SubAgentOnlyEvent),
}

// ============================================================================
// From Event -> ParentEvent / SubAgentEvent (for compatibility wrapper)
// ============================================================================

/// Error returned when an `Event` cannot be converted to the requested split type.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Used by TryFrom impls
pub struct EventConversionError {
    pub event_variant: &'static str,
    pub target_type: &'static str,
}

impl std::fmt::Display for EventConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot convert Event::{} to {}",
            self.event_variant, self.target_type
        )
    }
}

impl std::error::Error for EventConversionError {}

impl TryFrom<Event> for ParentEvent {
    type Error = EventConversionError;

    #[allow(clippy::too_many_lines)]
    fn try_from(event: Event) -> Result<Self, Self::Error> {
        match event {
            // Core events
            Event::UserMessage {
                text,
                llm_text,
                images,
                message_id,
                user_agent,
                skill_invocation,
            } => Ok(ParentEvent::Core(CoreEvent::UserMessage {
                text,
                llm_text,
                images,
                message_id,
                user_agent,
                skill_invocation,
            })),
            Event::UserCancel { reason } => Ok(ParentEvent::Core(CoreEvent::UserCancel { reason })),
            Event::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
            } => Ok(ParentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
            })),
            Event::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
            } => Ok(ParentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
            })),
            Event::RetryTimeout { attempt } => {
                Ok(ParentEvent::Core(CoreEvent::RetryTimeout { attempt }))
            }
            Event::ToolComplete {
                tool_use_id,
                result,
            } => Ok(ParentEvent::Core(CoreEvent::ToolComplete {
                tool_use_id,
                result,
            })),
            Event::ToolAborted { tool_use_id } => {
                Ok(ParentEvent::Core(CoreEvent::ToolAborted { tool_use_id }))
            }
            Event::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            } => Ok(ParentEvent::Core(CoreEvent::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            })),
            Event::SubAgentResult { agent_id, outcome } => {
                Ok(ParentEvent::Core(CoreEvent::SubAgentResult {
                    agent_id,
                    outcome,
                }))
            }
            Event::ContinuationResponse { summary } => {
                Ok(ParentEvent::Core(CoreEvent::ContinuationResponse {
                    summary,
                }))
            }
            Event::ContinuationFailed { error } => {
                Ok(ParentEvent::Core(CoreEvent::ContinuationFailed { error }))
            }
            Event::UserTriggerContinuation => {
                Ok(ParentEvent::Core(CoreEvent::UserTriggerContinuation))
            }
            // Parent-only events
            Event::TaskApprovalResponse { outcome } => {
                Ok(ParentEvent::Parent(ParentOnlyEvent::TaskApprovalResponse {
                    outcome,
                }))
            }
            Event::UserQuestionResponse {
                answers,
                annotations,
            } => Ok(ParentEvent::Parent(ParentOnlyEvent::UserQuestionResponse {
                answers,
                annotations,
            })),
            Event::CredentialBecameAvailable => Ok(ParentEvent::Parent(
                ParentOnlyEvent::CredentialBecameAvailable,
            )),
            Event::CredentialHelperFailed { message } => Ok(ParentEvent::Parent(
                ParentOnlyEvent::CredentialHelperFailed { message },
            )),
            Event::TaskResolved {
                system_message,
                repo_root,
            } => Ok(ParentEvent::Parent(ParentOnlyEvent::TaskResolved {
                system_message,
                repo_root,
            })),
            // Sub-agent-only events are invalid for parent
            Event::GraceTurnExhausted { .. } => Err(EventConversionError {
                event_variant: event.variant_name(),
                target_type: "ParentEvent",
            }),
        }
    }
}

impl TryFrom<Event> for SubAgentEvent {
    type Error = EventConversionError;

    #[allow(clippy::too_many_lines)]
    fn try_from(event: Event) -> Result<Self, Self::Error> {
        match event {
            // Core events
            Event::UserMessage {
                text,
                llm_text,
                images,
                message_id,
                user_agent,
                skill_invocation,
            } => Ok(SubAgentEvent::Core(CoreEvent::UserMessage {
                text,
                llm_text,
                images,
                message_id,
                user_agent,
                skill_invocation,
            })),
            Event::UserCancel { reason } => {
                Ok(SubAgentEvent::Core(CoreEvent::UserCancel { reason }))
            }
            Event::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
            } => Ok(SubAgentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
            })),
            Event::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
            } => Ok(SubAgentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
            })),
            Event::RetryTimeout { attempt } => {
                Ok(SubAgentEvent::Core(CoreEvent::RetryTimeout { attempt }))
            }
            Event::ToolComplete {
                tool_use_id,
                result,
            } => Ok(SubAgentEvent::Core(CoreEvent::ToolComplete {
                tool_use_id,
                result,
            })),
            Event::ToolAborted { tool_use_id } => {
                Ok(SubAgentEvent::Core(CoreEvent::ToolAborted { tool_use_id }))
            }
            Event::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            } => Ok(SubAgentEvent::Core(CoreEvent::SpawnAgentsComplete {
                tool_use_id,
                result,
                spawned,
            })),
            Event::SubAgentResult { agent_id, outcome } => {
                Ok(SubAgentEvent::Core(CoreEvent::SubAgentResult {
                    agent_id,
                    outcome,
                }))
            }
            Event::ContinuationResponse { summary } => {
                Ok(SubAgentEvent::Core(CoreEvent::ContinuationResponse {
                    summary,
                }))
            }
            Event::ContinuationFailed { error } => {
                Ok(SubAgentEvent::Core(CoreEvent::ContinuationFailed { error }))
            }
            Event::UserTriggerContinuation => {
                Ok(SubAgentEvent::Core(CoreEvent::UserTriggerContinuation))
            }
            // Sub-agent-only events
            Event::GraceTurnExhausted { result } => Ok(SubAgentEvent::SubAgent(
                SubAgentOnlyEvent::GraceTurnExhausted { result },
            )),
            // Parent-only events are invalid for sub-agent
            Event::TaskApprovalResponse { .. }
            | Event::UserQuestionResponse { .. }
            | Event::CredentialBecameAvailable
            | Event::CredentialHelperFailed { .. }
            | Event::TaskResolved { .. } => Err(EventConversionError {
                event_variant: event.variant_name(),
                target_type: "SubAgentEvent",
            }),
        }
    }
}

impl CoreEvent {
    /// Stable variant name for error reporting
    pub fn variant_name(&self) -> &'static str {
        match self {
            CoreEvent::UserMessage { .. } => "UserMessage",
            CoreEvent::UserCancel { .. } => "UserCancel",
            CoreEvent::LlmResponse { .. } => "LlmResponse",
            CoreEvent::LlmError { .. } => "LlmError",
            CoreEvent::RetryTimeout { .. } => "RetryTimeout",
            CoreEvent::ToolComplete { .. } => "ToolComplete",
            CoreEvent::ToolAborted { .. } => "ToolAborted",
            CoreEvent::SpawnAgentsComplete { .. } => "SpawnAgentsComplete",
            CoreEvent::SubAgentResult { .. } => "SubAgentResult",
            CoreEvent::ContinuationResponse { .. } => "ContinuationResponse",
            CoreEvent::ContinuationFailed { .. } => "ContinuationFailed",
            CoreEvent::UserTriggerContinuation => "UserTriggerContinuation",
        }
    }
}

impl ParentEvent {
    /// Stable variant name for error reporting
    pub fn variant_name(&self) -> &'static str {
        match self {
            ParentEvent::Core(e) => e.variant_name(),
            ParentEvent::Parent(e) => match e {
                ParentOnlyEvent::TaskApprovalResponse { .. } => "TaskApprovalResponse",
                ParentOnlyEvent::UserQuestionResponse { .. } => "UserQuestionResponse",
                ParentOnlyEvent::CredentialBecameAvailable => "CredentialBecameAvailable",
                ParentOnlyEvent::CredentialHelperFailed { .. } => "CredentialHelperFailed",
                ParentOnlyEvent::TaskResolved { .. } => "TaskResolved",
            },
        }
    }
}

impl SubAgentEvent {
    /// Stable variant name for error reporting
    #[allow(dead_code)] // Will be used when callers migrate from Event
    pub fn variant_name(&self) -> &'static str {
        match self {
            SubAgentEvent::Core(e) => e.variant_name(),
            SubAgentEvent::SubAgent(e) => match e {
                SubAgentOnlyEvent::GraceTurnExhausted { .. } => "GraceTurnExhausted",
            },
        }
    }
}
