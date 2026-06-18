//! Events that can occur in a conversation

use crate::domain::db_schema::{ErrorKind, FileAttachment, ImageData, ToolResult};
use serde::{Deserialize, Serialize};

/// A steering message queued for delivery when the conversation next reaches
/// `Idle`. The in-memory form of a pending steer; persisted across the
/// normalized `steering_messages` (+ attachment) tables by the DB layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerEntry {
    pub text: String,
    pub llm_text: Option<String>,
    pub images: Vec<ImageData>,
    #[serde(default)]
    pub files: Vec<FileAttachment>,
    pub message_id: String,
    pub user_agent: Option<String>,
    pub skill_invocation: Option<crate::domain::skill_invocation::SkillInvocation>,
}

use crate::domain::llm_types::{ContentBlock, Usage};
use crate::domain::sm_state::{
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
        files: Vec<FileAttachment>,
        /// Client-generated UUID - the canonical identifier for this message
        message_id: String,
        /// Browser user agent for display (e.g., show iPhone icon in UI)
        user_agent: Option<String>,
        /// If this message triggered a skill invocation, the details are here.
        /// When present, the message is persisted as `MessageContent::Skill`.
        skill_invocation: Option<crate::domain::skill_invocation::SkillInvocation>,
    },
    UserCancel {
        /// Why the cancel was issued. `None` means user-initiated or parent-propagated.
        reason: Option<String>,
        /// Whether this cancel was human-requested or forced by a timeout (task 61004).
        cause: CancelCause,
    },

    // LLM events
    LlmResponse {
        content: Vec<ContentBlock>,
        /// Tool calls extracted from the content
        tool_calls: Vec<ToolCall>,
        #[allow(dead_code)] // Reserved for conversation flow control
        end_turn: bool,
        usage: Usage,
        /// Server-generated request id, threaded through from `LlmOutcome::Response`.
        /// Used as the eventual `AssistantMessage.message_id`.
        request_id: String,
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
        /// Upstream quota window reset time, when known. Populated only for
        /// rate-limit errors whose `LlmError.quota` carried a `resets_at`
        /// value (see `llm/rate_limit.rs::QuotaDetails`). Threaded onto
        /// `Effect::ScheduleRetry` and out to `SseEvent::LlmAttempt` so
        /// the client can surface "(retry K/N after rate limit, resets at
        /// HH:MM)" — specs/llm-retry-visibility/ REQ-LRV-001. `None` for
        /// network/server-error retries and for rate-limit errors whose
        /// upstream response didn't include the reset timestamp.
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
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
    /// User responded to a proposed task plan.
    ///
    /// Matches the `TaskApprovalDecided(conversation, decision)` trigger in
    /// `specs/bedrock/bedrock.allium` (the surface's `provides:` block plus
    /// the four `UserApprovesTaskCurrentConversation` /
    /// `UserApprovesTaskFreshWorkConversation` / `UserProvidesFeedback` /
    /// `UserRejectsTask` rules). The HTTP wire type `TaskApprovalResponse`
    /// in `api/types.rs` is unrelated — it's the response body, not the
    /// lifecycle event.
    TaskApprovalDecided {
        outcome: TaskApprovalOutcome,
    },
    /// Internal completion event emitted after fresh task approval creates the
    /// successor Work conversation.
    TaskHandoffComplete {
        successor_conv_id: String,
    },

    // Ask user question events (REQ-AUQ-001)
    /// User answered the pending questions (POST /api/conversations/{id}/respond)
    UserQuestionResponse {
        answers: HashMap<String, String>,
        annotations: Option<HashMap<String, QuestionAnnotation>>,
    },
    /// User dismissed the structured question UI without answering it.
    UserQuestionDismissed,

    /// User dismissed a persisted `Error` state, returning the conversation to
    /// `Idle`. Server-authoritative: the UI does not fake the idle phase
    /// locally — it sends this event so the displayed state and the server
    /// state cannot diverge (a divergence makes server-gated actions like
    /// mark-merged reject while the UI claims the conversation is idle).
    DismissError,

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

    /// A steering message queued while the conversation was busy.
    /// Intercepted by the executor before reaching the state machine —
    /// pushed onto `ConversationRuntime::steering_queue` and delivered as
    /// `UserMessage` when the conversation next enters `Idle`.
    SteerMessage {
        /// Display text — stored in DB and shown in history.
        text: String,
        /// Expanded text delivered to the LLM when `@` references are present.
        llm_text: Option<String>,
        images: Vec<ImageData>,
        files: Vec<FileAttachment>,
        /// Client-generated UUID — canonical identifier for this message.
        message_id: String,
        user_agent: Option<String>,
        skill_invocation: Option<crate::domain::skill_invocation::SkillInvocation>,
    },
    /// Removes a steering entry from the executor's in-memory queue.
    /// Intercepted by the executor before reaching the state machine.
    /// The DB is updated by the cancel handler before this event is sent.
    CancelSteerMessage {
        message_id: String,
    },

    /// Drained steering entries delivered to bedrock for persistence as
    /// `UserMessage`s. Fired by the executor at steering-drain hook points:
    /// turn-end (entering `Idle`) or mid-turn (entering `LlmRequesting` from
    /// a tool round). Parent conversations only.
    SteerDrainedUserMessages {
        entries: Vec<SteerEntry>,
    },

    /// Sent by `RuntimeManager::evict_runtime` (e.g. after a model upgrade)
    /// to cleanly terminate a running runtime that is being replaced.
    /// The executor returns from `run()` immediately on receipt, which drops
    /// the broadcaster and allows connected SSE clients to detect the closed
    /// stream and reconnect to the new runtime.
    Shutdown,
}

impl Event {
    /// Stable, payload-free name of this event variant. Used by structured
    /// error types (e.g. `TransitionError::InvalidTransition`) and tracing
    /// so they can carry a discriminator without the `Debug` format of the
    /// variant's payloads — task 24682 follow-up. Single source of truth.
    #[must_use]
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
            Event::TaskApprovalDecided { .. } => "TaskApprovalDecided",
            Event::TaskHandoffComplete { .. } => "TaskHandoffComplete",
            Event::UserQuestionResponse { .. } => "UserQuestionResponse",
            Event::UserQuestionDismissed => "UserQuestionDismissed",
            Event::DismissError => "DismissError",
            Event::GraceTurnExhausted { .. } => "GraceTurnExhausted",
            Event::CredentialBecameAvailable => "CredentialBecameAvailable",
            Event::CredentialHelperFailed { .. } => "CredentialHelperFailed",
            Event::TaskResolved { .. } => "TaskResolved",
            Event::SteerMessage { .. } => "SteerMessage",
            Event::CancelSteerMessage { .. } => "CancelSteerMessage",
            Event::SteerDrainedUserMessages { .. } => "SteerDrainedUserMessages",
            Event::Shutdown => "Shutdown",
        }
    }
}

// ============================================================================
// Split Event Types — CoreEvent, ParentOnlyEvent, SubAgentOnlyEvent
// ============================================================================

/// Why a `UserCancel` was issued — drives the recorded sub-agent outcome on
/// forced teardown (task 61004). `UserRequested` is a human-initiated or
/// parent-propagated cancel; `Timeout` is the parent's sub-agent completion
/// timeout forcing teardown.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelCause {
    UserRequested,
    Timeout,
}

/// Events handled by the core transition function (shared by both parent and sub-agent).
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum CoreEvent {
    UserMessage {
        text: String,
        llm_text: Option<String>,
        images: Vec<ImageData>,
        files: Vec<FileAttachment>,
        message_id: String,
        user_agent: Option<String>,
        skill_invocation: Option<crate::domain::skill_invocation::SkillInvocation>,
    },
    UserCancel {
        reason: Option<String>,
        cause: CancelCause,
    },
    LlmResponse {
        content: Vec<ContentBlock>,
        tool_calls: Vec<ToolCall>,
        end_turn: bool,
        usage: Usage,
        request_id: String,
    },
    LlmError {
        message: String,
        error_kind: ErrorKind,
        attempt: u32,
        recovery_in_progress: bool,
        /// Quota reset timestamp; see `Event::LlmError::resets_at`.
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
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
    /// Drained steering entries to be persisted as `UserMessage`s mid-conversation.
    /// Fired by the executor when transitioning into a state that is about to ask
    /// the LLM (entering `Idle`, or entering `LlmRequesting` from a tool round).
    /// Bedrock persists each entry as a User message and may transition state.
    /// See `specs/steering-messages/` for the queue mechanism.
    SteerDrainedUserMessages {
        entries: Vec<SteerEntry>,
    },
}

/// Events only valid for parent conversations.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants used by split transition functions
pub enum ParentOnlyEvent {
    TaskApprovalDecided {
        outcome: TaskApprovalOutcome,
    },
    TaskHandoffComplete {
        successor_conv_id: String,
    },
    UserQuestionResponse {
        answers: HashMap<String, String>,
        annotations: Option<HashMap<String, QuestionAnnotation>>,
    },
    UserQuestionDismissed,
    DismissError,
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
                files,
                message_id,
                user_agent,
                skill_invocation,
            } => Ok(ParentEvent::Core(CoreEvent::UserMessage {
                text,
                llm_text,
                images,
                files,
                message_id,
                user_agent,
                skill_invocation,
            })),
            Event::UserCancel { reason, cause } => {
                Ok(ParentEvent::Core(CoreEvent::UserCancel { reason, cause }))
            }
            Event::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
                request_id,
            } => Ok(ParentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
                request_id,
            })),
            Event::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
                resets_at,
            } => Ok(ParentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
                resets_at,
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
            Event::SteerDrainedUserMessages { entries } => {
                Ok(ParentEvent::Core(CoreEvent::SteerDrainedUserMessages {
                    entries,
                }))
            }
            // Parent-only events
            Event::TaskApprovalDecided { outcome } => {
                Ok(ParentEvent::Parent(ParentOnlyEvent::TaskApprovalDecided {
                    outcome,
                }))
            }
            Event::TaskHandoffComplete { successor_conv_id } => {
                Ok(ParentEvent::Parent(ParentOnlyEvent::TaskHandoffComplete {
                    successor_conv_id,
                }))
            }
            Event::UserQuestionResponse {
                answers,
                annotations,
            } => Ok(ParentEvent::Parent(ParentOnlyEvent::UserQuestionResponse {
                answers,
                annotations,
            })),
            Event::UserQuestionDismissed => {
                Ok(ParentEvent::Parent(ParentOnlyEvent::UserQuestionDismissed))
            }
            Event::DismissError => Ok(ParentEvent::Parent(ParentOnlyEvent::DismissError)),
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
            // Sub-agent-only events are invalid for parent;
            // SteerMessage, CancelSteerMessage, and Shutdown are intercepted by
            // the executor before reaching the state-machine conversion.
            Event::GraceTurnExhausted { .. }
            | Event::SteerMessage { .. }
            | Event::CancelSteerMessage { .. }
            | Event::Shutdown => Err(EventConversionError {
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
                files,
                message_id,
                user_agent,
                skill_invocation,
            } => Ok(SubAgentEvent::Core(CoreEvent::UserMessage {
                text,
                llm_text,
                images,
                files,
                message_id,
                user_agent,
                skill_invocation,
            })),
            Event::UserCancel { reason, cause } => {
                Ok(SubAgentEvent::Core(CoreEvent::UserCancel { reason, cause }))
            }
            Event::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
                request_id,
            } => Ok(SubAgentEvent::Core(CoreEvent::LlmResponse {
                content,
                tool_calls,
                end_turn,
                usage,
                request_id,
            })),
            Event::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
                resets_at,
            } => Ok(SubAgentEvent::Core(CoreEvent::LlmError {
                message,
                error_kind,
                attempt,
                recovery_in_progress,
                resets_at,
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
            // Parent-only events are invalid for sub-agent;
            // SteerMessage, CancelSteerMessage, and Shutdown are intercepted by
            // the executor before reaching the state-machine conversion.
            // SteerDrainedUserMessages is parent-only: steering is a parent-
            // conversation feature, and the executor's drain detector guards
            // against firing for sub-agents.
            Event::TaskApprovalDecided { .. }
            | Event::TaskHandoffComplete { .. }
            | Event::UserQuestionResponse { .. }
            | Event::UserQuestionDismissed
            | Event::DismissError
            | Event::CredentialBecameAvailable
            | Event::CredentialHelperFailed { .. }
            | Event::TaskResolved { .. }
            | Event::SteerMessage { .. }
            | Event::CancelSteerMessage { .. }
            | Event::SteerDrainedUserMessages { .. }
            | Event::Shutdown => Err(EventConversionError {
                event_variant: event.variant_name(),
                target_type: "SubAgentEvent",
            }),
        }
    }
}

impl CoreEvent {
    /// Stable variant name for error reporting
    #[must_use]
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
            CoreEvent::SteerDrainedUserMessages { .. } => "SteerDrainedUserMessages",
        }
    }
}

impl ParentEvent {
    /// Stable variant name for error reporting
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            ParentEvent::Core(e) => e.variant_name(),
            ParentEvent::Parent(e) => match e {
                ParentOnlyEvent::TaskApprovalDecided { .. } => "TaskApprovalDecided",
                ParentOnlyEvent::TaskHandoffComplete { .. } => "TaskHandoffComplete",
                ParentOnlyEvent::UserQuestionResponse { .. } => "UserQuestionResponse",
                ParentOnlyEvent::UserQuestionDismissed => "UserQuestionDismissed",
                ParentOnlyEvent::DismissError => "DismissError",
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
    #[must_use]
    pub fn variant_name(&self) -> &'static str {
        match self {
            SubAgentEvent::Core(e) => e.variant_name(),
            SubAgentEvent::SubAgent(e) => match e {
                SubAgentOnlyEvent::GraceTurnExhausted { .. } => "GraceTurnExhausted",
            },
        }
    }
}

#[cfg(test)]
mod spec_runtime_name_alignment_tests {
    use super::*;
    use crate::domain::sm_state::TaskApprovalOutcome;

    /// Lock the event variant name to the spec trigger name. The four
    /// `UserApprovesTaskCurrentConversation` / `UserApprovesTaskFreshWorkConversation`
    /// / `UserProvidesFeedback` / `UserRejectsTask` rules in
    /// `specs/bedrock/bedrock.allium` subscribe to
    /// `TaskApprovalDecided(conversation, decision)`; the bedrock + auth
    /// surfaces declare it under `provides:`. The Rust event name and that
    /// spec name must match, or the audit-class drift task 02684 caught
    /// returns. Renaming one without the other regresses; this test fails
    /// at compile time (variant) and runtime (string) if so.
    #[test]
    fn task_approval_event_name_matches_spec_trigger() {
        let event = Event::TaskApprovalDecided {
            outcome: TaskApprovalOutcome::Rejected,
        };
        assert_eq!(event.variant_name(), "TaskApprovalDecided");

        let parent_only = ParentOnlyEvent::TaskApprovalDecided {
            outcome: TaskApprovalOutcome::Rejected,
        };
        let parent_event = ParentEvent::Parent(parent_only);
        assert_eq!(parent_event.variant_name(), "TaskApprovalDecided");
    }
}
