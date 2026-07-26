//! Events that can occur in a conversation

use crate::domain::db_schema::{ErrorKind, FileAttachment, ImageData, ToolResult};
use crate::domain::skill_invocation::SkillInvocation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnWorkflowId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnTurnId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnEffectId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnAttemptId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnWorkflowVersion(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnProcessIncarnation(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectTurnAttemptAuthority {
    pub workflow_id: DirectTurnWorkflowId,
    pub turn_id: DirectTurnTurnId,
    pub effect_id: DirectTurnEffectId,
    pub attempt_id: DirectTurnAttemptId,
    pub declared_workflow_version: DirectTurnWorkflowVersion,
    pub generation: DirectTurnGeneration,
    pub process_incarnation: DirectTurnProcessIncarnation,
}

impl DirectTurnAttemptAuthority {
    #[must_use]
    pub const fn new(
        workflow_id: u64,
        turn_id: u64,
        effect_id: u64,
        attempt_id: u64,
        declared_workflow_version: u64,
        generation: u64,
        process_incarnation: u64,
    ) -> Self {
        Self {
            workflow_id: DirectTurnWorkflowId(workflow_id),
            turn_id: DirectTurnTurnId(turn_id),
            effect_id: DirectTurnEffectId(effect_id),
            attempt_id: DirectTurnAttemptId(attempt_id),
            declared_workflow_version: DirectTurnWorkflowVersion(declared_workflow_version),
            generation: DirectTurnGeneration(generation),
            process_incarnation: DirectTurnProcessIncarnation(process_incarnation),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmittedDirectTurnExpansionPolicy {
    ExpandReferences,
    LiteralText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedDirectTurnFileAttachment {
    pub original_name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub stored_path: String,
}

impl From<FileAttachment> for SubmittedDirectTurnFileAttachment {
    fn from(value: FileAttachment) -> Self {
        Self {
            original_name: value.original_name,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
            stored_path: value.stored_path,
        }
    }
}

impl From<SubmittedDirectTurnFileAttachment> for FileAttachment {
    fn from(value: SubmittedDirectTurnFileAttachment) -> Self {
        Self {
            original_name: value.original_name,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
            stored_path: value.stored_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedDirectTurnIdentity {
    pub text: String,
    pub images: Vec<ImageData>,
    pub files: Vec<SubmittedDirectTurnFileAttachment>,
    pub message_id: String,
    pub user_agent: Option<String>,
    pub skill_invocation: Option<SkillInvocation>,
    pub expansion_policy: SubmittedDirectTurnExpansionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedDirectTurnDelivery {
    pub text: String,
    pub llm_text: Option<String>,
    pub images: Vec<ImageData>,
    pub files: Vec<FileAttachment>,
    pub user_agent: Option<String>,
    pub skill_invocation: Option<crate::domain::skill_invocation::SkillInvocation>,
}

/// Serializable, lossless direct-turn payload prepared by the authoritative
/// transport before it is delivered to the reducer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedDirectTurnPayload {
    pub v: u32,
    pub submitted: SubmittedDirectTurnIdentity,
    pub delivery: PreparedDirectTurnDelivery,
}

#[derive(Debug, thiserror::Error)]
pub enum PreparedDirectTurnPayloadCodecError {
    #[error("encode direct-turn prepared payload: {0}")]
    Encode(serde_json::Error),
    #[error("decode direct-turn prepared payload: {0}")]
    Decode(serde_json::Error),
    #[error("unsupported direct-turn prepared payload version {actual}; expected {expected}")]
    UnsupportedVersion { actual: u32, expected: u32 },
}

impl PreparedDirectTurnPayload {
    pub const VERSION: u32 = 1;
    fn normalized_value_without_attachments(
        &self,
    ) -> Result<serde_json::Value, PreparedDirectTurnPayloadCodecError> {
        let mut value =
            serde_json::to_value(self).map_err(PreparedDirectTurnPayloadCodecError::Encode)?;
        if let Some(submitted) = value.get_mut("submitted") {
            if let Some(obj) = submitted.as_object_mut() {
                obj.remove("images");
                obj.remove("files");
            }
        }
        if let Some(delivery) = value.get_mut("delivery") {
            if let Some(obj) = delivery.as_object_mut() {
                obj.remove("images");
                obj.remove("files");
            }
        }
        Ok(value)
    }

    /// # Errors
    /// Returns an error when the normalized payload cannot be serialized.
    pub fn to_normalized_bytes_without_attachments(
        &self,
    ) -> Result<Vec<u8>, PreparedDirectTurnPayloadCodecError> {
        serde_json::to_vec(&self.normalized_value_without_attachments()?)
            .map_err(PreparedDirectTurnPayloadCodecError::Encode)
    }

    /// # Errors
    /// Returns an error when the normalized payload cannot be decoded.
    pub fn rehydrate_from_normalized_bytes(
        bytes: &[u8],
        submitted_images: Vec<ImageData>,
        submitted_files: Vec<SubmittedDirectTurnFileAttachment>,
        delivery_images: Vec<ImageData>,
        delivery_files: Vec<FileAttachment>,
    ) -> Result<Self, PreparedDirectTurnPayloadCodecError> {
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(PreparedDirectTurnPayloadCodecError::Decode)?;
        let Some(root) = value.as_object_mut() else {
            return Err(PreparedDirectTurnPayloadCodecError::Decode(
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "prepared payload root must be object",
                )),
            ));
        };
        let Some(submitted) = root.get_mut("submitted") else {
            return Err(PreparedDirectTurnPayloadCodecError::Decode(
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "prepared payload missing submitted",
                )),
            ));
        };
        let Some(submitted_obj) = submitted.as_object_mut() else {
            return Err(PreparedDirectTurnPayloadCodecError::Decode(
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "prepared payload submitted must be object",
                )),
            ));
        };
        submitted_obj.insert(
            "images".to_string(),
            serde_json::to_value(submitted_images)
                .map_err(PreparedDirectTurnPayloadCodecError::Encode)?,
        );
        submitted_obj.insert(
            "files".to_string(),
            serde_json::to_value(submitted_files)
                .map_err(PreparedDirectTurnPayloadCodecError::Encode)?,
        );
        let Some(delivery) = root.get_mut("delivery") else {
            return Err(PreparedDirectTurnPayloadCodecError::Decode(
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "prepared payload missing delivery",
                )),
            ));
        };
        let Some(delivery_obj) = delivery.as_object_mut() else {
            return Err(PreparedDirectTurnPayloadCodecError::Decode(
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "prepared payload delivery must be object",
                )),
            ));
        };
        delivery_obj.insert(
            "images".to_string(),
            serde_json::to_value(delivery_images)
                .map_err(PreparedDirectTurnPayloadCodecError::Encode)?,
        );
        delivery_obj.insert(
            "files".to_string(),
            serde_json::to_value(delivery_files)
                .map_err(PreparedDirectTurnPayloadCodecError::Encode)?,
        );
        let payload: Self =
            serde_json::from_value(value).map_err(PreparedDirectTurnPayloadCodecError::Decode)?;
        if payload.v != Self::VERSION {
            return Err(PreparedDirectTurnPayloadCodecError::UnsupportedVersion {
                actual: payload.v,
                expected: Self::VERSION,
            });
        }
        Ok(payload)
    }

    #[must_use]
    pub const fn from_parts(
        submitted: SubmittedDirectTurnIdentity,
        delivery: PreparedDirectTurnDelivery,
    ) -> Self {
        Self {
            v: Self::VERSION,
            submitted,
            delivery,
        }
    }

    #[must_use]
    pub fn message_id(&self) -> &str {
        &self.submitted.message_id
    }

    #[must_use]
    pub fn submitted_identity_matches(&self, other: &SubmittedDirectTurnIdentity) -> bool {
        &self.submitted == other
    }

    /// Encodes the complete versioned envelope.
    ///
    /// # Errors
    /// Returns [`PreparedDirectTurnPayloadCodecError::Encode`] if JSON encoding fails.
    pub fn to_exact_bytes(&self) -> Result<Vec<u8>, PreparedDirectTurnPayloadCodecError> {
        serde_json::to_vec(self).map_err(PreparedDirectTurnPayloadCodecError::Encode)
    }

    /// Decodes and version-checks a complete envelope.
    ///
    /// # Errors
    /// Returns a decode error for invalid JSON or `UnsupportedVersion` for an
    /// envelope whose codec version is not supported.
    pub fn from_exact_bytes(bytes: &[u8]) -> Result<Self, PreparedDirectTurnPayloadCodecError> {
        let payload: Self =
            serde_json::from_slice(bytes).map_err(PreparedDirectTurnPayloadCodecError::Decode)?;
        if payload.v != Self::VERSION {
            return Err(PreparedDirectTurnPayloadCodecError::UnsupportedVersion {
                actual: payload.v,
                expected: Self::VERSION,
            });
        }
        Ok(payload)
    }

    /// Hashes the exact encoded envelope bytes.
    ///
    /// # Errors
    /// Returns [`PreparedDirectTurnPayloadCodecError::Encode`] if encoding fails.
    pub fn exact_fingerprint(&self) -> Result<String, PreparedDirectTurnPayloadCodecError> {
        Ok(exact_payload_fingerprint(&self.to_exact_bytes()?))
    }

    #[must_use]
    pub fn message_content_and_display_data(
        &self,
    ) -> (
        crate::domain::db_schema::MessageContent,
        Option<serde_json::Value>,
    ) {
        let content = if let Some(invocation) = &self.delivery.skill_invocation {
            crate::domain::db_schema::MessageContent::Skill(
                crate::domain::db_schema::SkillContent {
                    name: invocation.name.clone(),
                    body: invocation.body.clone(),
                    trigger: self.delivery.text.clone(),
                    files: self.delivery.files.clone(),
                },
            )
        } else {
            match &self.delivery.llm_text {
                Some(expanded) => crate::domain::db_schema::MessageContent::User(
                    crate::domain::db_schema::UserContent::with_expansion(
                        self.delivery.text.clone(),
                        expanded.clone(),
                        self.delivery.images.clone(),
                        self.delivery.files.clone(),
                    ),
                ),
                None => {
                    if self.delivery.images.is_empty() && self.delivery.files.is_empty() {
                        crate::domain::db_schema::MessageContent::user(self.delivery.text.clone())
                    } else {
                        crate::domain::db_schema::MessageContent::user_with_attachments(
                            self.delivery.text.clone(),
                            self.delivery.images.clone(),
                            self.delivery.files.clone(),
                        )
                    }
                }
            }
        };
        let display_data = self
            .delivery
            .user_agent
            .as_ref()
            .map(|ua| serde_json::json!({ "user_agent": ua }));
        (content, display_data)
    }
}

#[must_use]
pub fn exact_payload_fingerprint(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

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

impl From<PreparedDirectTurnPayload> for SteerEntry {
    fn from(value: PreparedDirectTurnPayload) -> Self {
        Self {
            text: value.delivery.text,
            llm_text: value.delivery.llm_text,
            images: value.delivery.images,
            files: value.delivery.files,
            message_id: value.submitted.message_id,
            user_agent: value.delivery.user_agent,
            skill_invocation: value.delivery.skill_invocation,
        }
    }
}

use crate::domain::llm_types::{ContentBlock, Usage};
use crate::domain::sm_state::{
    CommissionReviewApprovalOutcome, PendingSubAgent, QuestionAnnotation, SubAgentOutcome,
    TaskApprovalOutcome, ToolCall,
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
    AuthoritativeUserMessage {
        payload: PreparedDirectTurnPayload,
        authority: DirectTurnAttemptAuthority,
    },
    /// Internal first-turn event accepted only while the shell is provisioning.
    CreationProvisioned {
        initial_message: SteerEntry,
        job_id: String,
        claim: super::creation_protocol::CreationClaim,
    },
    /// Internal crash-recovery event for an initial request persisted before dispatch.
    CreationRequestResume {
        job_id: String,
        claim: super::creation_protocol::CreationClaim,
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
    CommissionReviewApprovalDecided {
        outcome: CommissionReviewApprovalOutcome,
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

    /// Internal notification that durable wake messages and the matching
    /// `LlmRequesting` state were adopted atomically in `SQLite`. A live idle
    /// executor uses this edge to mirror the persisted state and start the
    /// already-adopted turn without persisting another message or state row.
    WakeBatchAdopted,

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
            Event::AuthoritativeUserMessage { .. } => "AuthoritativeUserMessage",
            Event::CreationProvisioned { .. } => "CreationProvisioned",
            Event::CreationRequestResume { .. } => "CreationRequestResume",
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
            Event::CommissionReviewApprovalDecided { .. } => "CommissionReviewApprovalDecided",
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
            Event::WakeBatchAdopted => "WakeBatchAdopted",
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
    AuthoritativeUserMessage {
        payload: Box<PreparedDirectTurnPayload>,
        authority: DirectTurnAttemptAuthority,
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
    CommissionReviewApprovalDecided {
        outcome: CommissionReviewApprovalOutcome,
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
#[allow(clippy::large_enum_variant)]
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
            Event::AuthoritativeUserMessage { payload, authority } => {
                Ok(ParentEvent::Core(CoreEvent::AuthoritativeUserMessage {
                    payload: Box::new(payload),
                    authority,
                }))
            }
            Event::CreationProvisioned { .. } => Err(EventConversionError {
                event_variant: "CreationProvisioned",
                target_type: "ParentEvent",
            }),
            Event::CreationRequestResume { .. } => Err(EventConversionError {
                event_variant: "CreationRequestResume",
                target_type: "ParentEvent",
            }),
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
            Event::CommissionReviewApprovalDecided { outcome } => Ok(ParentEvent::Parent(
                ParentOnlyEvent::CommissionReviewApprovalDecided { outcome },
            )),
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
            | Event::WakeBatchAdopted
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
            Event::AuthoritativeUserMessage { payload, authority } => {
                Ok(SubAgentEvent::Core(CoreEvent::AuthoritativeUserMessage {
                    payload: Box::new(payload),
                    authority,
                }))
            }
            Event::CreationProvisioned { .. } => Err(EventConversionError {
                event_variant: "CreationProvisioned",
                target_type: "SubAgentEvent",
            }),
            Event::CreationRequestResume { .. } => Err(EventConversionError {
                event_variant: "CreationRequestResume",
                target_type: "SubAgentEvent",
            }),
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
            | Event::CommissionReviewApprovalDecided { .. }
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
            | Event::WakeBatchAdopted
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
            CoreEvent::AuthoritativeUserMessage { .. } => "AuthoritativeUserMessage",
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
                ParentOnlyEvent::CommissionReviewApprovalDecided { .. } => {
                    "CommissionReviewApprovalDecided"
                }
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

#[cfg(test)]
mod direct_turn_payload_tests {
    use super::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, PreparedDirectTurnPayloadCodecError,
        SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
    };

    fn submitted(
        message_id: &str,
        policy: SubmittedDirectTurnExpansionPolicy,
    ) -> SubmittedDirectTurnIdentity {
        SubmittedDirectTurnIdentity {
            text: "display @file".to_string(),
            images: Vec::new(),
            files: Vec::new(),
            message_id: message_id.to_string(),
            user_agent: Some("agent/test".to_string()),
            skill_invocation: None,
            expansion_policy: policy,
        }
    }

    fn delivery(text: &str, llm_text: Option<&str>) -> PreparedDirectTurnDelivery {
        PreparedDirectTurnDelivery {
            text: text.to_string(),
            llm_text: llm_text.map(str::to_string),
            images: Vec::new(),
            files: Vec::new(),
            user_agent: Some("agent/test".to_string()),
            skill_invocation: None,
        }
    }

    #[test]
    fn prepared_payload_exact_bytes_roundtrip_preserves_submitted_and_delivery() {
        let payload = PreparedDirectTurnPayload::from_parts(
            submitted(
                "msg-1",
                SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            ),
            delivery("display @file", Some("display <file>expanded</file>")),
        );
        let bytes = payload.to_exact_bytes().unwrap();
        assert_eq!(
            PreparedDirectTurnPayload::from_exact_bytes(&bytes).unwrap(),
            payload
        );
        assert_eq!(
            payload.exact_fingerprint().unwrap(),
            super::exact_payload_fingerprint(&bytes)
        );
    }

    #[test]
    fn prepared_payload_rejects_unsupported_version() {
        let payload = PreparedDirectTurnPayload::from_parts(
            submitted(
                "msg-1",
                SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            ),
            delivery("display @file", Some("display <file>expanded</file>")),
        );
        let mut value = serde_json::to_value(payload).unwrap();
        value["v"] = serde_json::json!(PreparedDirectTurnPayload::VERSION + 1);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            PreparedDirectTurnPayload::from_exact_bytes(&bytes),
            Err(PreparedDirectTurnPayloadCodecError::UnsupportedVersion { actual, expected })
                if actual == PreparedDirectTurnPayload::VERSION + 1
                    && expected == PreparedDirectTurnPayload::VERSION
        ));
    }

    #[test]
    fn delivery_conversion_uses_resolved_delivery_not_submitted_identity() {
        let payload = PreparedDirectTurnPayload::from_parts(
            submitted(
                "msg-1",
                SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            ),
            delivery("display @file", Some("display <file>expanded</file>")),
        );
        let (content, _) = payload.message_content_and_display_data();
        let crate::domain::db_schema::MessageContent::User(user) = content else {
            panic!("expected user content");
        };
        assert_eq!(user.text, "display @file");
        assert_eq!(
            user.llm_text.as_deref(),
            Some("display <file>expanded</file>")
        );
    }

    #[test]
    fn same_submitted_identity_can_have_changed_delivery() {
        let submitted = submitted(
            "msg-1",
            SubmittedDirectTurnExpansionPolicy::ExpandReferences,
        );
        let original = PreparedDirectTurnPayload::from_parts(
            submitted.clone(),
            delivery("display @file", Some("first expansion")),
        );
        let changed = PreparedDirectTurnPayload::from_parts(
            submitted.clone(),
            delivery("display @file", Some("second expansion")),
        );
        assert!(original.submitted_identity_matches(&submitted));
        assert!(changed.submitted_identity_matches(&submitted));
        assert_ne!(original, changed);
    }
}
