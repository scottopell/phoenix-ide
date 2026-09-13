//! Trait abstractions for runtime I/O
//!
//! These traits enable testing the executor with mock implementations.

use crate::db::{ConvMode, Message, MessageContent, UsageData};
use crate::state_machine::ConvState;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use phoenix_llm::{LlmError, LlmRequest, LlmResponse};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveDirectTurn {
    pub turn_id: phoenix_workflow::TurnAuthorityId,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadedActiveDirectTurn {
    Unmaterialized {
        active: ActiveDirectTurn,
    },
    Materialized {
        active: ActiveDirectTurn,
        canonical_message_id: String,
    },
}

impl LoadedActiveDirectTurn {
    pub const fn active(&self) -> &ActiveDirectTurn {
        match self {
            Self::Unmaterialized { active } | Self::Materialized { active, .. } => active,
        }
    }

    pub fn into_active(self) -> ActiveDirectTurn {
        match self {
            Self::Unmaterialized { active } | Self::Materialized { active, .. } => active,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AuthoritativeUserMessageMaterialization {
    Materialized {
        message: Box<Message>,
        active: ActiveDirectTurn,
    },
    ClassifiedCommitted {
        message: Box<Message>,
        active: ActiveDirectTurn,
    },
    ExactReplay,
    NotCommitted,
    StaleAuthority,
    CommandRejected,
    DurableFactUnclassified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveDirectTurnTerminal {
    Completed,
    Cancelled,
    Failed { reason: String },
}

impl ActiveDirectTurnTerminal {
    pub(crate) const fn variant_name(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed { .. } => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveDirectTurnSettlement {
    pub turn: ActiveDirectTurn,
    pub conversation_id: String,
    pub terminal: ActiveDirectTurnTerminal,
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ContinuationDirectTurnSettlement {
    pub turn: ActiveDirectTurn,
    pub terminal: ActiveDirectTurnTerminal,
    pub operation_id: String,
    pub message: Message,
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ContinuationStartRecoverySettlement {
    pub turn: Option<ActiveDirectTurn>,
    pub terminal: Option<ActiveDirectTurnTerminal>,
    pub operation_id: String,
    pub message: Message,
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AuthoritativeUserMessageAdoptionInput {
    pub authority: phoenix_core::domain::sm_event::DirectTurnAttemptAuthority,
    pub payload: phoenix_core::domain::sm_event::PreparedDirectTurnPayload,
    pub sequence_id: i64,
    pub created_at: phoenix_workflow::Timestamp,
    pub accepted_state: ConvState,
    pub state_updated_at: DateTime<Utc>,
    pub now: phoenix_workflow::Timestamp,
}

/// Storage for conversation messages
#[async_trait]
pub trait MessageStore: Send + Sync {
    async fn accepted_continuation_handoff_message_id(
        &self,
        conv_id: &str,
    ) -> Result<Option<String>, String>;

    /// Add a message to the conversation
    ///
    /// `message_id` is the canonical identifier for this message. For user messages,
    /// this is client-generated (enabling idempotent retries). For agent/tool messages,
    /// this is server-generated.
    #[allow(dead_code)]
    async fn add_message(
        &self,
        message_id: &str,
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String>;

    /// Add a message with a pre-allocated `sequence_id`.
    ///
    /// Used by code paths that also broadcast the message over SSE: the
    /// seq is taken from `SseBroadcaster::next_seq()` before this call, so
    /// the message's own seq is strictly greater than any ephemeral event
    /// (token, `state_change`, error) broadcast earlier on the same
    /// conversation. This is what enforces `PersistBeforeBroadcast`
    /// (`specs/sse_wire/sse_wire.allium`) at the sequence-allocation level
    /// and prevents the "message seq < client `lastSequenceId` → dropped by
    /// `applyIfNewer`" failure from task 02679.
    ///
    /// Callers that do NOT broadcast (e.g. sub-agent bootstrap user
    /// message, crash-recovery system marker) may still use
    /// [`MessageStore::add_message`]; their seq allocation race is benign
    /// because no client ever sees a stale seq for them.
    async fn add_message_with_seq(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String>;

    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_and_terminal_obligation(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalEvidenceEstablishment;

    /// Like `add_message_with_seq`, but writes a caller-supplied
    /// `created_at` instead of `Utc::now()`. Used by `persist_checkpoint`
    /// to align the durable DB row's timestamp with the eager-broadcast
    /// `AssistantMessage` timestamp atomically (single INSERT), so there
    /// is no window where a concurrent reconnect's init read could see
    /// a transient `Utc::now()` value before the alignment write lands.
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_at(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Message, String>;

    /// Get all messages for a conversation
    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String>;

    /// Load one transactionally consistent, fully hydrated durable prompt
    /// snapshot for a runtime-owned projection.
    async fn load_hydrated_prompt_snapshot(
        &self,
        conv_id: &str,
    ) -> Result<crate::db::HydratedPromptSnapshot, String>;

    /// Load the fully hydrated durable tail after a generation-fenced prompt
    /// position.
    async fn load_hydrated_prompt_tail(
        &self,
        conv_id: &str,
        after: crate::db::GenerationFencedPromptPosition,
    ) -> Result<crate::db::HydratedPromptTail, String>;

    /// Get a single message by ID
    #[allow(dead_code)]
    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String>;

    /// Returns true if a message with the given `message_id` already exists.
    /// Used by `PersistMessage` to make persistence idempotent across crash
    /// recovery (re-drain after partial steering-queue drain).
    async fn message_exists(&self, message_id: &str) -> Result<bool, String>;

    /// Atomically persist the initial message, complete its creation job, and
    /// commit the dispatchable runtime state under one current claim.
    #[allow(clippy::too_many_arguments)]
    async fn materialize_creation_runtime(
        &self,
        _job_id: &str,
        _claim: &phoenix_core::domain::creation_protocol::CreationClaim,
        _conversation_id: &str,
        _allocate_sequence: &mut (dyn FnMut(i64) -> i64 + Send),
        _content: &MessageContent,
        _display_data: Option<&Value>,
        _usage_data: Option<&UsageData>,
        _message_id: &str,
        _state: &ConvState,
        _state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::db::CreationRuntimeMaterialization, String> {
        Ok(crate::db::CreationRuntimeMaterialization::ClaimLost)
    }

    /// Atomically complete an async creation job and commit its runtime state.
    async fn settle_creation_runtime(
        &self,
        _job_id: &str,
        _claim: &phoenix_core::domain::creation_protocol::CreationClaim,
        _conversation_id: &str,
        _state: &ConvState,
        _state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::db::CreationCasOutcome, String> {
        Ok(crate::db::CreationCasOutcome::ClaimLost)
    }

    async fn preflight_authoritative_user_message(
        &self,
        _authority: &phoenix_core::domain::sm_event::DirectTurnAttemptAuthority,
        _payload: &phoenix_core::domain::sm_event::PreparedDirectTurnPayload,
        _now: phoenix_workflow::Timestamp,
    ) -> Result<phoenix_db::workflow::DirectTurnMaterializationEligibility, String> {
        Ok(phoenix_db::workflow::DirectTurnMaterializationEligibility::StaleAuthority)
    }

    async fn materialize_authoritative_user_message(
        &self,
        _input: &AuthoritativeUserMessageAdoptionInput,
    ) -> Result<AuthoritativeUserMessageMaterialization, String> {
        Ok(AuthoritativeUserMessageMaterialization::StaleAuthority)
    }

    async fn load_active_direct_turn(
        &self,
        conversation_id: &str,
    ) -> Result<Option<LoadedActiveDirectTurn>, String>;

    async fn settle_active_direct_turn(
        &self,
        settlement: &ActiveDirectTurnSettlement,
    ) -> Result<(), String>;

    async fn persist_active_direct_turn_terminal_obligation(
        &self,
        settlement: &ActiveDirectTurnSettlement,
        response_message_id: Option<&str>,
    ) -> TerminalMutationEstablishment;

    async fn settle_continuation_direct_turn(
        &self,
        settlement: &ContinuationDirectTurnSettlement,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        let _ = settlement;
        Ok(crate::db::ContinuationCommitOutcome::Stale)
    }

    /// Update `display_data` for an existing message
    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<i64, String>;

    /// Update the `content` text inside a tool result message's JSON.
    /// Used to write sub-agent outcomes into the `spawn_agents` tool result before
    /// the LLM is called, so the results appear in the conversation history.
    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<i64, String>;

    /// Atomically persist a fork proposal together with the originating turn's
    /// tool round (REQ-PROJ-033): the assistant message, each synthetic
    /// tool-result message, and the `fork_proposals` row commit in a single
    /// transaction. The caller pre-builds the message rows (seq-allocated and
    /// content-mapped) so the broadcast seq ordering matches the persisted
    /// rows.
    async fn persist_fork_proposal_with_tool_round(
        &self,
        origin_conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
        proposal: &crate::db::ForkProposal,
    ) -> Result<(), String>;

    /// Atomically persist a completed tool round (REQ-BED-007): the assistant
    /// message and every paired tool-result message commit in a single
    /// transaction, or none do. The caller pre-builds the message rows
    /// (seq-allocated and content-mapped) so the broadcast seq ordering matches
    /// the persisted rows. A partial write would leave an unpaired `tool_use`
    /// that 400s every later LLM request.
    async fn persist_tool_round(
        &self,
        conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
    ) -> Result<(), String>;

    async fn persist_tool_round_with_terminal_obligation(
        &self,
        conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalMutationEstablishment;

    async fn persist_sub_agent_results_with_terminal_obligation(
        &self,
        evidence: &TerminalSubAgentEvidence,
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalMutationEstablishment;
}

#[derive(Clone, Debug)]
pub enum TerminalEvidenceEstablishment {
    Established(Box<Message>),
    Retired,
    KnownNotCommitted(String),
    Unclassifiable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalMutationEstablishment {
    Established { transcript_generation: Option<i64> },
    Retired,
    KnownNotCommitted(String),
    Unclassifiable(String),
}

#[derive(Clone, Debug)]
pub enum TerminalSubAgentEvidence {
    Update {
        conversation_id: String,
        message_id: String,
        content: MessageContent,
        display_data: Value,
    },
    Insert(Message),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersistedStateSnapshot {
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
}

/// Storage for conversation state
#[async_trait]
pub trait StateStore: Send + Sync {
    async fn establish_parent_reconcile_action(
        &self,
        _conversation_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Update the conversation state (full state as JSON). `state_updated_at`
    /// is the runtime's authoritative phase-entry timestamp; persisting it
    /// (rather than a fresh `now()` at the storage layer) keeps the DB row and
    /// the `StateChange` SSE on one value (REQ-WPV-001).
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<(), String>;

    /// Get the current conversation state
    #[allow(dead_code)] // API completeness
    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String>;

    async fn get_state_snapshot(&self, conv_id: &str) -> Result<PersistedStateSnapshot, String>;

    async fn begin_continuation(
        &self,
        conv_id: &str,
        operation_id: &str,
        message: &crate::db::Message,
        awaiting_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<crate::db::ContinuationCommitOutcome, String>;

    async fn recover_continuation_start(
        &self,
        settlement: &ContinuationStartRecoverySettlement,
    ) -> Result<crate::db::ContinuationCommitOutcome, String>;

    async fn commit_continuation(
        &self,
        conv_id: &str,
        operation_id: &str,
        message: &crate::db::Message,
        completed_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<crate::db::ContinuationCommitOutcome, String>;

    async fn persist_approved_task_authority(
        &self,
        conv_id: &str,
        approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
    ) -> Result<(), String>;

    /// Get the current conversation mode (used by effect handlers that need
    /// worktree path / branch name, since `ConvContext.mode` only carries the
    /// `ModeKind` discriminant, not the concrete paths).
    #[allow(dead_code)]
    async fn get_conversation_mode(&self, conv_id: &str) -> Result<ConvMode, String>;

    /// Update the conversation working directory. Conversation cwd is
    /// immutable post-creation; the only legitimate callers are
    /// recovery/teardown fallbacks (task 13012). The `_recovery_only`
    /// suffix keeps this off the casual-mutation path.
    async fn update_conversation_cwd_recovery_only(
        &self,
        conv_id: &str,
        cwd: &str,
    ) -> Result<(), String>;

    /// Read the conversation's clear watermark for stale tool-result clearing
    /// (specs/stale-tool-results). Returns 0 when nothing has been cleared yet.
    async fn get_clear_watermark(&self, conv_id: &str) -> Result<i64, String>;

    /// Advance the conversation's clear watermark. The write is structurally
    /// monotonic — a value below the persisted watermark is ignored, never
    /// regressing it (specs/stale-tool-results, REQ-STR-007).
    async fn set_clear_watermark(&self, conv_id: &str, watermark: i64) -> Result<(), String>;

    /// The total prompt size of the most recent turn (input + cache-read +
    /// cache-creation tokens), or `None` if the conversation has no turns yet.
    /// The clearing pressure signal (specs/stale-tool-results, REQ-STR-001).
    async fn get_last_turn_prompt_tokens(&self, conv_id: &str) -> Result<Option<i64>, String>;

    /// Record token usage for one LLM turn. Fire-and-forget; errors are logged
    /// by the caller and do not affect the conversation.
    async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
        usage: &phoenix_llm::Usage,
        first_byte_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), String>;

    /// Record one content-free provider attempt for TTFT and stall analytics.
    async fn upsert_llm_request_metrics(
        &self,
        metrics: &phoenix_llm::LlmAttemptMetrics,
    ) -> Result<(), String>;

    /// Load the current durable steering queue in FIFO order.
    async fn load_steering_entries(
        &self,
        conv_id: &str,
    ) -> Result<Vec<phoenix_core::domain::sm_event::SteerEntry>, String>;

    /// Atomically insert the reducer-selected FIFO batch, persist its supplied
    /// next state, and remove exactly its queue identities.
    async fn commit_steering_drain(
        &self,
        conv_id: &str,
        messages: &[crate::db::Message],
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<Vec<crate::db::SteeringDrainMessageStatus>, String>;
}

/// Client for making LLM requests
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Complete an LLM request (non-streaming)
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Streaming completion — emits `TokenChunk::Text` events via `chunk_tx` as tokens
    /// arrive, then returns the fully assembled `LlmResponse`.
    /// Default implementation calls `complete()` with no streaming.
    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &tokio::sync::mpsc::Sender<phoenix_llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let _ = chunk_tx;
        self.complete(request).await
    }

    /// Get the model ID
    #[allow(dead_code)] // API completeness
    fn model_id(&self) -> &str;

    /// Typed provider/route limits for continuation-summary requests.
    fn continuation_request_limits(&self) -> phoenix_llm::ContinuationRequestLimits {
        phoenix_llm::ContinuationRequestLimits::TokenWindowOnly
    }
}

use crate::runtime::deny_gate::CheckedToolCall;
use crate::tools::ToolContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCapabilityGeneration(pub u64);

impl ToolCapabilityGeneration {
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct ToolCapabilitySnapshot {
    pub generation: ToolCapabilityGeneration,
    pub authority: phoenix_core::work_scope::ResourceAuthority,
    pub clearable_names: Arc<std::collections::HashSet<String>>,
}

impl ToolCapabilitySnapshot {
    #[must_use]
    pub fn restricted() -> Self {
        Self {
            generation: ToolCapabilityGeneration::INITIAL,
            authority: phoenix_core::work_scope::ResourceAuthority::Restricted,
            clearable_names: Arc::new(std::collections::HashSet::new()),
        }
    }
}

/// Executor for tools
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a gate-cleared tool call. Accepting only a [`CheckedToolCall`]
    /// — whose sole non-test mint is `DenyGate::check` — makes an ungated tool
    /// call unrepresentable (specs/permissions REQ-PERM-001).
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput>;

    async fn execute_at_generation(
        &self,
        expected: ToolCapabilityGeneration,
        call: CheckedToolCall,
        ctx: ToolContext,
    ) -> Result<Option<ToolOutput>, String> {
        if self.capability_snapshot().generation != expected {
            return Err("stale tool capability generation".to_string());
        }
        Ok(self.execute(call, ctx).await)
    }

    /// Get tool definitions for LLM (phoenix-native).
    async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition>;

    /// Get tool definitions in the requested LLM language. Default impl
    /// delegates to `definitions()` (phoenix-native); production overrides
    /// translate tool descriptions per-language.
    async fn definitions_for_language(
        &self,
        _language: crate::llm_language::LlmLanguage,
    ) -> Vec<phoenix_llm::ToolDefinition> {
        self.definitions().await
    }

    /// Frozen model IDs advertised by the conversation's `spawn_agents`
    /// schema. Spawn-time validation uses this same snapshot so schema and
    /// executor acceptance cannot drift if the live registry changes.
    fn subagent_model_ids(&self) -> Arc<[String]> {
        Arc::from(Vec::new())
    }

    fn capability_snapshot(&self) -> ToolCapabilitySnapshot {
        ToolCapabilitySnapshot::restricted()
    }

    async fn definitions_for_generation(
        &self,
        expected: ToolCapabilityGeneration,
        language: crate::llm_language::LlmLanguage,
    ) -> Result<Vec<phoenix_llm::ToolDefinition>, String> {
        if self.capability_snapshot().generation != expected {
            return Err("stale tool capability generation".to_string());
        }
        let definitions = self.definitions_for_language(language).await;
        if self.capability_snapshot().generation != expected {
            return Err("stale tool capability generation".to_string());
        }
        Ok(definitions)
    }

    /// Publish the Work-authority tool set before post-approval execution resumes.
    /// Executors that cannot perform the authority transition fail closed.
    fn upgrade_to_work_mode(&self) -> Result<ToolCapabilitySnapshot, String> {
        Err("tool executor cannot publish Work authority".to_string())
    }
}

/// Combined storage trait for convenience
pub trait Storage: MessageStore + StateStore {}
impl<T: MessageStore + StateStore> Storage for T {}

// ============================================================================
// Arc implementations for trait objects
// ============================================================================

#[async_trait]
impl<T: MessageStore + ?Sized> MessageStore for Arc<T> {
    async fn accepted_continuation_handoff_message_id(
        &self,
        conv_id: &str,
    ) -> Result<Option<String>, String> {
        (**self)
            .accepted_continuation_handoff_message_id(conv_id)
            .await
    }

    async fn add_message(
        &self,
        message_id: &str,
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        (**self)
            .add_message(message_id, conv_id, content, display_data, usage_data)
            .await
    }

    async fn add_message_with_seq(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        (**self)
            .add_message_with_seq(
                message_id,
                conv_id,
                sequence_id,
                content,
                display_data,
                usage_data,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_and_terminal_obligation(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalEvidenceEstablishment {
        (**self)
            .add_message_with_seq_and_terminal_obligation(
                message_id,
                conv_id,
                sequence_id,
                content,
                display_data,
                usage_data,
                settlement,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_at(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Message, String> {
        (**self)
            .add_message_with_seq_at(
                message_id,
                conv_id,
                sequence_id,
                content,
                display_data,
                usage_data,
                created_at,
            )
            .await
    }

    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String> {
        (**self).get_messages(conv_id).await
    }

    async fn load_hydrated_prompt_snapshot(
        &self,
        conv_id: &str,
    ) -> Result<crate::db::HydratedPromptSnapshot, String> {
        (**self).load_hydrated_prompt_snapshot(conv_id).await
    }

    async fn load_hydrated_prompt_tail(
        &self,
        conv_id: &str,
        after: crate::db::GenerationFencedPromptPosition,
    ) -> Result<crate::db::HydratedPromptTail, String> {
        (**self).load_hydrated_prompt_tail(conv_id, after).await
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String> {
        (**self).get_message_by_id(message_id).await
    }

    async fn message_exists(&self, message_id: &str) -> Result<bool, String> {
        (**self).message_exists(message_id).await
    }

    async fn materialize_creation_runtime(
        &self,
        job_id: &str,
        claim: &phoenix_core::domain::creation_protocol::CreationClaim,
        conversation_id: &str,
        allocate_sequence: &mut (dyn FnMut(i64) -> i64 + Send),
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        message_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::db::CreationRuntimeMaterialization, String> {
        (**self)
            .materialize_creation_runtime(
                job_id,
                claim,
                conversation_id,
                allocate_sequence,
                content,
                display_data,
                usage_data,
                message_id,
                state,
                state_updated_at,
            )
            .await
    }

    async fn settle_creation_runtime(
        &self,
        job_id: &str,
        claim: &phoenix_core::domain::creation_protocol::CreationClaim,
        conversation_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::db::CreationCasOutcome, String> {
        (**self)
            .settle_creation_runtime(job_id, claim, conversation_id, state, state_updated_at)
            .await
    }

    async fn preflight_authoritative_user_message(
        &self,
        authority: &phoenix_core::domain::sm_event::DirectTurnAttemptAuthority,
        payload: &phoenix_core::domain::sm_event::PreparedDirectTurnPayload,
        now: phoenix_workflow::Timestamp,
    ) -> Result<phoenix_db::workflow::DirectTurnMaterializationEligibility, String> {
        (**self)
            .preflight_authoritative_user_message(authority, payload, now)
            .await
    }

    async fn materialize_authoritative_user_message(
        &self,
        input: &AuthoritativeUserMessageAdoptionInput,
    ) -> Result<AuthoritativeUserMessageMaterialization, String> {
        (**self).materialize_authoritative_user_message(input).await
    }

    async fn load_active_direct_turn(
        &self,
        conversation_id: &str,
    ) -> Result<Option<LoadedActiveDirectTurn>, String> {
        (**self).load_active_direct_turn(conversation_id).await
    }

    async fn persist_active_direct_turn_terminal_obligation(
        &self,
        settlement: &ActiveDirectTurnSettlement,
        response_message_id: Option<&str>,
    ) -> TerminalMutationEstablishment {
        (**self)
            .persist_active_direct_turn_terminal_obligation(settlement, response_message_id)
            .await
    }

    async fn settle_active_direct_turn(
        &self,
        settlement: &ActiveDirectTurnSettlement,
    ) -> Result<(), String> {
        (**self).settle_active_direct_turn(settlement).await
    }

    async fn settle_continuation_direct_turn(
        &self,
        settlement: &ContinuationDirectTurnSettlement,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        (**self).settle_continuation_direct_turn(settlement).await
    }

    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<i64, String> {
        (**self)
            .update_message_display_data(message_id, display_data)
            .await
    }

    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<i64, String> {
        (**self)
            .update_tool_message_content(message_id, content)
            .await
    }

    async fn persist_fork_proposal_with_tool_round(
        &self,
        origin_conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
        proposal: &crate::db::ForkProposal,
    ) -> Result<(), String> {
        (**self)
            .persist_fork_proposal_with_tool_round(
                origin_conv_id,
                assistant,
                tool_results,
                proposal,
            )
            .await
    }

    async fn persist_tool_round(
        &self,
        conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
    ) -> Result<(), String> {
        (**self)
            .persist_tool_round(conv_id, assistant, tool_results)
            .await
    }

    async fn persist_tool_round_with_terminal_obligation(
        &self,
        conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalMutationEstablishment {
        (**self)
            .persist_tool_round_with_terminal_obligation(
                conv_id,
                assistant,
                tool_results,
                settlement,
            )
            .await
    }

    async fn persist_sub_agent_results_with_terminal_obligation(
        &self,
        evidence: &TerminalSubAgentEvidence,
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalMutationEstablishment {
        (**self)
            .persist_sub_agent_results_with_terminal_obligation(evidence, settlement)
            .await
    }
}

#[async_trait]
impl<T: StateStore + ?Sized> StateStore for Arc<T> {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<(), String> {
        (**self)
            .update_state(conv_id, state, state_updated_at)
            .await
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        (**self).get_state(conv_id).await
    }

    async fn get_state_snapshot(&self, conv_id: &str) -> Result<PersistedStateSnapshot, String> {
        (**self).get_state_snapshot(conv_id).await
    }

    async fn begin_continuation(
        &self,
        conv_id: &str,
        operation_id: &str,
        message: &crate::db::Message,
        awaiting_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        (**self)
            .begin_continuation(
                conv_id,
                operation_id,
                message,
                awaiting_state,
                state_updated_at,
            )
            .await
    }

    async fn recover_continuation_start(
        &self,
        settlement: &ContinuationStartRecoverySettlement,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        (**self).recover_continuation_start(settlement).await
    }

    async fn commit_continuation(
        &self,
        conv_id: &str,
        operation_id: &str,
        message: &crate::db::Message,
        completed_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        (**self)
            .commit_continuation(
                conv_id,
                operation_id,
                message,
                completed_state,
                state_updated_at,
            )
            .await
    }

    async fn persist_approved_task_authority(
        &self,
        conv_id: &str,
        approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
    ) -> Result<(), String> {
        (**self)
            .persist_approved_task_authority(conv_id, approval)
            .await
    }

    async fn get_conversation_mode(&self, conv_id: &str) -> Result<ConvMode, String> {
        (**self).get_conversation_mode(conv_id).await
    }

    async fn update_conversation_cwd_recovery_only(
        &self,
        conv_id: &str,
        cwd: &str,
    ) -> Result<(), String> {
        (**self)
            .update_conversation_cwd_recovery_only(conv_id, cwd)
            .await
    }

    async fn get_clear_watermark(&self, conv_id: &str) -> Result<i64, String> {
        (**self).get_clear_watermark(conv_id).await
    }

    async fn set_clear_watermark(&self, conv_id: &str, watermark: i64) -> Result<(), String> {
        (**self).set_clear_watermark(conv_id, watermark).await
    }

    async fn get_last_turn_prompt_tokens(&self, conv_id: &str) -> Result<Option<i64>, String> {
        (**self).get_last_turn_prompt_tokens(conv_id).await
    }

    async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
        usage: &phoenix_llm::Usage,
        first_byte_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), String> {
        (**self)
            .insert_turn_usage(
                conversation_id,
                root_conversation_id,
                model,
                effective_effort,
                usage,
                first_byte_at,
            )
            .await
    }

    async fn upsert_llm_request_metrics(
        &self,
        metrics: &phoenix_llm::LlmAttemptMetrics,
    ) -> Result<(), String> {
        (**self).upsert_llm_request_metrics(metrics).await
    }

    async fn load_steering_entries(
        &self,
        conv_id: &str,
    ) -> Result<Vec<phoenix_core::domain::sm_event::SteerEntry>, String> {
        (**self).load_steering_entries(conv_id).await
    }

    async fn commit_steering_drain(
        &self,
        conv_id: &str,
        messages: &[crate::db::Message],
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<Vec<crate::db::SteeringDrainMessageStatus>, String> {
        (**self)
            .commit_steering_drain(conv_id, messages, state, state_updated_at)
            .await
    }
}

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClient for Arc<T> {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        (**self).complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &tokio::sync::mpsc::Sender<phoenix_llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        (**self).complete_streaming(request, chunk_tx).await
    }

    fn model_id(&self) -> &str {
        (**self).model_id()
    }

    fn continuation_request_limits(&self) -> phoenix_llm::ContinuationRequestLimits {
        (**self).continuation_request_limits()
    }
}

#[async_trait]
impl<T: ToolExecutor + ?Sized> ToolExecutor for Arc<T> {
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput> {
        (**self).execute(call, ctx).await
    }

    async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition> {
        (**self).definitions().await
    }

    async fn definitions_for_language(
        &self,
        language: crate::llm_language::LlmLanguage,
    ) -> Vec<phoenix_llm::ToolDefinition> {
        (**self).definitions_for_language(language).await
    }

    fn subagent_model_ids(&self) -> Arc<[String]> {
        (**self).subagent_model_ids()
    }

    fn capability_snapshot(&self) -> ToolCapabilitySnapshot {
        (**self).capability_snapshot()
    }

    async fn definitions_for_generation(
        &self,
        expected: ToolCapabilityGeneration,
        language: crate::llm_language::LlmLanguage,
    ) -> Result<Vec<phoenix_llm::ToolDefinition>, String> {
        (**self)
            .definitions_for_generation(expected, language)
            .await
    }

    async fn execute_at_generation(
        &self,
        expected: ToolCapabilityGeneration,
        call: CheckedToolCall,
        ctx: ToolContext,
    ) -> Result<Option<ToolOutput>, String> {
        (**self).execute_at_generation(expected, call, ctx).await
    }

    fn upgrade_to_work_mode(&self) -> Result<ToolCapabilitySnapshot, String> {
        (**self).upgrade_to_work_mode()
    }
}

// ============================================================================
// Production Adapters
// ============================================================================

use crate::db::Database;
use crate::tools::{ToolRegistry, WritingConversationTools};
use phoenix_llm::ModelRegistry;
use std::sync::Arc;

/// Adapter to use Database as Storage
#[derive(Clone)]
pub struct DatabaseStorage {
    db: Database,
}

impl DatabaseStorage {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    #[allow(dead_code)] // Useful for tests
    pub fn inner(&self) -> &Database {
        &self.db
    }
}

fn direct_turn_terminal_command(
    turn: &ActiveDirectTurn,
    terminal: ActiveDirectTurnTerminal,
) -> phoenix_workflow::TurnCommand {
    match terminal {
        ActiveDirectTurnTerminal::Completed => phoenix_workflow::TurnCommand::Complete {
            turn_id: turn.turn_id,
            expected_generation: turn.generation,
        },
        ActiveDirectTurnTerminal::Cancelled => phoenix_workflow::TurnCommand::Cancel {
            turn_id: turn.turn_id,
            expected_generation: turn.generation,
        },
        ActiveDirectTurnTerminal::Failed { reason } => phoenix_workflow::TurnCommand::Fail {
            turn_id: turn.turn_id,
            expected_generation: turn.generation,
            reason,
        },
    }
}

#[async_trait]
impl MessageStore for DatabaseStorage {
    async fn accepted_continuation_handoff_message_id(
        &self,
        conv_id: &str,
    ) -> Result<Option<String>, String> {
        self.db
            .accepted_continuation_handoff_message_id(conv_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn add_message(
        &self,
        message_id: &str,
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        self.db
            .add_message(message_id, conv_id, content, display_data, usage_data)
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_message_with_seq(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        self.db
            .add_message_with_seq(
                message_id,
                conv_id,
                sequence_id,
                content,
                display_data,
                usage_data,
            )
            .await
            .map_err(|e| e.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_and_terminal_obligation(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalEvidenceEstablishment {
        let terminal = match &settlement.terminal {
            ActiveDirectTurnTerminal::Completed => phoenix_workflow::TurnTerminal::Completed,
            ActiveDirectTurnTerminal::Cancelled => phoenix_workflow::TurnTerminal::Cancelled,
            ActiveDirectTurnTerminal::Failed { reason } => phoenix_workflow::TurnTerminal::Failed {
                reason: reason.clone(),
            },
        };
        let obligation = phoenix_db::workflow::DirectTurnTerminalObligationInput {
            turn_id: settlement.turn.turn_id,
            expected_generation: settlement.turn.generation,
            terminal,
            projection: phoenix_db::workflow::PersistedConversationProjection {
                state: settlement.state.clone(),
                state_updated_at: settlement.state_updated_at,
            },
            response_message_id: Some(message_id.to_string()),
        };
        match self
            .db
            .add_message_with_seq_and_terminal_obligation(
                message_id,
                conv_id,
                sequence_id,
                content,
                display_data,
                usage_data,
                &obligation,
            )
            .await
        {
            Ok(message) => TerminalEvidenceEstablishment::Established(Box::new(message)),
            Err(write_error) => {
                let repo = self.db.workflow_repository();
                match repo
                    .probe_terminal_evidence(conv_id, message_id, &obligation)
                    .await
                {
                    Ok(phoenix_db::workflow::TerminalEvidenceProbe::Established { .. }) => self
                        .db
                        .get_message_by_id_in_conversation(conv_id, message_id)
                        .await
                        .map_or_else(
                            |probe_error| TerminalEvidenceEstablishment::Unclassifiable(format!(
                                "{write_error}; established terminal response retrieval failed: {probe_error}"
                            )),
                            |message| TerminalEvidenceEstablishment::Established(Box::new(message)),
                        ),
                    Ok(phoenix_db::workflow::TerminalEvidenceProbe::Retired) => {
                        TerminalEvidenceEstablishment::Retired
                    }
                    Ok(phoenix_db::workflow::TerminalEvidenceProbe::KnownNotCommitted) => {
                        TerminalEvidenceEstablishment::KnownNotCommitted(write_error.to_string())
                    }
                    Ok(phoenix_db::workflow::TerminalEvidenceProbe::Incomplete) => {
                        TerminalEvidenceEstablishment::Unclassifiable(format!(
                            "{write_error}; terminal evidence is incomplete"
                        ))
                    }
                    Err(probe_error) => TerminalEvidenceEstablishment::Unclassifiable(format!(
                        "{write_error}; exact terminal evidence classification failed: {probe_error}"
                    )),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_message_with_seq_at(
        &self,
        message_id: &str,
        conv_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Message, String> {
        self.db
            .add_message_with_seq_at(
                message_id,
                conv_id,
                sequence_id,
                content,
                display_data,
                usage_data,
                created_at,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String> {
        self.db
            .get_messages(conv_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn load_hydrated_prompt_snapshot(
        &self,
        conv_id: &str,
    ) -> Result<crate::db::HydratedPromptSnapshot, String> {
        self.db
            .load_hydrated_prompt_snapshot(conv_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn load_hydrated_prompt_tail(
        &self,
        conv_id: &str,
        after: crate::db::GenerationFencedPromptPosition,
    ) -> Result<crate::db::HydratedPromptTail, String> {
        self.db
            .load_hydrated_prompt_tail(conv_id, after)
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String> {
        self.db
            .get_message_by_id(message_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn message_exists(&self, message_id: &str) -> Result<bool, String> {
        self.db
            .message_exists(message_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn materialize_creation_runtime(
        &self,
        job_id: &str,
        claim: &phoenix_core::domain::creation_protocol::CreationClaim,
        conversation_id: &str,
        allocate_sequence: &mut (dyn FnMut(i64) -> i64 + Send),
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
        message_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::db::CreationRuntimeMaterialization, String> {
        self.db
            .materialize_conversation_creation_runtime(
                job_id,
                claim,
                conversation_id,
                message_id,
                allocate_sequence,
                content,
                display_data,
                usage_data,
                state,
                state_updated_at,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn settle_creation_runtime(
        &self,
        job_id: &str,
        claim: &phoenix_core::domain::creation_protocol::CreationClaim,
        conversation_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::db::CreationCasOutcome, String> {
        self.db
            .settle_conversation_creation_runtime(
                job_id,
                claim,
                conversation_id,
                state,
                state_updated_at,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn preflight_authoritative_user_message(
        &self,
        authority: &phoenix_core::domain::sm_event::DirectTurnAttemptAuthority,
        payload: &phoenix_core::domain::sm_event::PreparedDirectTurnPayload,
        now: phoenix_workflow::Timestamp,
    ) -> Result<phoenix_db::workflow::DirectTurnMaterializationEligibility, String> {
        use phoenix_db::workflow::PreflightDirectTurnMaterializationInput;
        use phoenix_workflow::TurnAuthorityId;
        let repo = self.db.workflow_repository();
        let local_authority = direct_turn_local_authority(authority);
        repo.preflight_direct_turn_materialization(&PreflightDirectTurnMaterializationInput {
            turn_id: TurnAuthorityId(authority.turn_id.0),
            authority: local_authority,
            prepared: payload.clone(),
            now,
        })
        .await
        .map_err(|error| error.to_string())
    }

    async fn materialize_authoritative_user_message(
        &self,
        input: &AuthoritativeUserMessageAdoptionInput,
    ) -> Result<AuthoritativeUserMessageMaterialization, String> {
        use phoenix_db::workflow::{
            LocalAuthorityResult, MaterializeAuthoritativeTurnInput,
            MaterializeAuthoritativeTurnOutcome,
        };
        use phoenix_workflow::TurnAuthorityId;
        let repo = self.db.workflow_repository();
        let local_authority = direct_turn_local_authority(&input.authority);
        let materialized = repo
            .materialize_authoritative_turn(&MaterializeAuthoritativeTurnInput {
                turn_id: TurnAuthorityId(input.authority.turn_id.0),
                authority: local_authority,
                prepared: input.payload.clone(),
                sequence_id: input.sequence_id,
                created_at: input.created_at,
                accepted_state: input.accepted_state.clone(),
                state_updated_at: input.state_updated_at,
                now: input.now,
            })
            .await;
        Ok(match materialized {
            LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::Materialized(materialization),
            ) => AuthoritativeUserMessageMaterialization::Materialized {
                message: materialization.message,
                active: ActiveDirectTurn {
                    turn_id: materialization.turn_id,
                    generation: materialization.generation,
                },
            },
            LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::ExactReplay(_),
            ) => AuthoritativeUserMessageMaterialization::ExactReplay,
            LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::ClassifiedCommitted(materialization),
            ) => AuthoritativeUserMessageMaterialization::ClassifiedCommitted {
                message: materialization.message,
                active: ActiveDirectTurn {
                    turn_id: materialization.turn_id,
                    generation: materialization.generation,
                },
            },
            LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::NotCommitted,
            ) => AuthoritativeUserMessageMaterialization::NotCommitted,
            LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::StaleAuthority,
            ) => AuthoritativeUserMessageMaterialization::StaleAuthority,
            LocalAuthorityResult::DurableFactEstablished(
                MaterializeAuthoritativeTurnOutcome::CommandRejected(error),
            ) => {
                tracing::warn!(?error, "direct-turn materialization command rejected");
                AuthoritativeUserMessageMaterialization::CommandRejected
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                AuthoritativeUserMessageMaterialization::DurableFactUnclassified
            }
        })
    }

    async fn load_active_direct_turn(
        &self,
        conversation_id: &str,
    ) -> Result<Option<LoadedActiveDirectTurn>, String> {
        let repo = self.db.workflow_repository();
        repo.load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
            conversation_id.to_string(),
        ))
        .await
        .map(|turn| {
            turn.map(|turn| {
                let active = ActiveDirectTurn {
                    turn_id: turn.id,
                    generation: turn.generation,
                };
                match turn.materialization {
                    phoenix_workflow::Materialization::Unmaterialized => {
                        LoadedActiveDirectTurn::Unmaterialized { active }
                    }
                    phoenix_workflow::Materialization::Materialized { message_id } => {
                        LoadedActiveDirectTurn::Materialized {
                            active,
                            canonical_message_id: message_id.0,
                        }
                    }
                }
            })
        })
        .map_err(|error| error.to_string())
    }

    async fn persist_active_direct_turn_terminal_obligation(
        &self,
        settlement: &ActiveDirectTurnSettlement,
        response_message_id: Option<&str>,
    ) -> TerminalMutationEstablishment {
        let obligation =
            terminal_obligation(settlement, response_message_id.map(ToString::to_string));
        let repo = self.db.workflow_repository();
        match repo.persist_terminal_obligation(&obligation).await {
            Ok(()) => TerminalMutationEstablishment::Established {
                transcript_generation: None,
            },
            Err(error) => {
                classify_terminal_mutation(
                    &self.db,
                    &phoenix_db::workflow::TerminalEvidenceExpectation::ObligationOnly {
                        conversation_id: settlement.conversation_id.clone(),
                    },
                    &obligation,
                    error.to_string(),
                    None,
                )
                .await
            }
        }
    }

    async fn settle_active_direct_turn(
        &self,
        settlement: &ActiveDirectTurnSettlement,
    ) -> Result<(), String> {
        let repo = self.db.workflow_repository();
        repo.terminalize_authoritative_turn(
            &phoenix_db::workflow::TerminalizeAuthoritativeTurnInput {
                command: direct_turn_terminal_command(
                    &settlement.turn,
                    settlement.terminal.clone(),
                ),
                projection: Some(phoenix_db::workflow::PersistedConversationProjection {
                    state: settlement.state.clone(),
                    state_updated_at: settlement.state_updated_at,
                }),
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    async fn settle_continuation_direct_turn(
        &self,
        settlement: &ContinuationDirectTurnSettlement,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        let repo = self.db.workflow_repository();
        repo.settle_continuation_direct_turn_atomically(
            &phoenix_db::workflow::AtomicContinuationSettlementInput {
                conversation_id: settlement.message.conversation_id.clone(),
                operation_id: settlement.operation_id.clone(),
                message: settlement.message.clone(),
                completed_state: settlement.state.clone(),
                state_updated_at: settlement.state_updated_at,
                command: direct_turn_terminal_command(
                    &settlement.turn,
                    settlement.terminal.clone(),
                ),
            },
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<i64, String> {
        self.db
            .update_message_display_data(message_id, display_data)
            .await
            .map_err(|e| e.to_string())
    }

    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<i64, String> {
        self.db
            .update_tool_message_content(message_id, content)
            .await
            .map_err(|e| e.to_string())
    }

    async fn persist_fork_proposal_with_tool_round(
        &self,
        origin_conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
        proposal: &crate::db::ForkProposal,
    ) -> Result<(), String> {
        self.db
            .persist_fork_proposal_with_tool_round(
                origin_conv_id,
                assistant,
                tool_results,
                proposal,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn persist_tool_round(
        &self,
        conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
    ) -> Result<(), String> {
        self.db
            .persist_tool_round(conv_id, assistant, tool_results)
            .await
            .map_err(|e| e.to_string())
    }

    async fn persist_tool_round_with_terminal_obligation(
        &self,
        conv_id: &str,
        assistant: &crate::db::Message,
        tool_results: &[crate::db::Message],
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalMutationEstablishment {
        let obligation = terminal_obligation(settlement, None);
        let mut messages = Vec::with_capacity(1 + tool_results.len());
        messages.push(assistant.clone());
        messages.extend_from_slice(tool_results);
        let evidence = phoenix_db::workflow::TerminalEvidenceExpectation::Messages(messages);
        match self
            .db
            .persist_tool_round_with_terminal_obligation(
                conv_id,
                assistant,
                tool_results,
                &obligation,
            )
            .await
        {
            Ok(()) => TerminalMutationEstablishment::Established {
                transcript_generation: None,
            },
            Err(error) => {
                classify_terminal_mutation(
                    &self.db,
                    &evidence,
                    &obligation,
                    error.to_string(),
                    None,
                )
                .await
            }
        }
    }

    async fn persist_sub_agent_results_with_terminal_obligation(
        &self,
        evidence: &TerminalSubAgentEvidence,
        settlement: &ActiveDirectTurnSettlement,
    ) -> TerminalMutationEstablishment {
        let obligation = terminal_obligation(settlement, None);
        let evidence = match evidence {
            TerminalSubAgentEvidence::Update {
                conversation_id,
                message_id,
                content,
                display_data,
            } => phoenix_db::workflow::TerminalEvidenceExpectation::MessageMutation {
                conversation_id: conversation_id.clone(),
                message_id: message_id.clone(),
                content: content.clone(),
                display_data: display_data.clone(),
            },
            TerminalSubAgentEvidence::Insert(message) => {
                phoenix_db::workflow::TerminalEvidenceExpectation::Messages(vec![message.clone()])
            }
        };
        match self
            .db
            .persist_sub_agent_terminal_evidence(&evidence, &obligation)
            .await
        {
            Ok(transcript_generation) => TerminalMutationEstablishment::Established {
                transcript_generation,
            },
            Err(error) => {
                let transcript_generation = if evidence.is_message_mutation() {
                    sqlx::query_scalar(
                        "SELECT transcript_generation FROM conversations WHERE id = ?1",
                    )
                    .bind(evidence.conversation_id())
                    .fetch_optional(self.db.pool())
                    .await
                    .ok()
                    .flatten()
                } else {
                    None
                };
                classify_terminal_mutation(
                    &self.db,
                    &evidence,
                    &obligation,
                    error.to_string(),
                    transcript_generation,
                )
                .await
            }
        }
    }
}

fn terminal_obligation(
    settlement: &ActiveDirectTurnSettlement,
    response_message_id: Option<String>,
) -> phoenix_db::workflow::DirectTurnTerminalObligationInput {
    let terminal = match &settlement.terminal {
        ActiveDirectTurnTerminal::Completed => phoenix_workflow::TurnTerminal::Completed,
        ActiveDirectTurnTerminal::Cancelled => phoenix_workflow::TurnTerminal::Cancelled,
        ActiveDirectTurnTerminal::Failed { reason } => phoenix_workflow::TurnTerminal::Failed {
            reason: reason.clone(),
        },
    };
    phoenix_db::workflow::DirectTurnTerminalObligationInput {
        turn_id: settlement.turn.turn_id,
        expected_generation: settlement.turn.generation,
        terminal,
        projection: phoenix_db::workflow::PersistedConversationProjection {
            state: settlement.state.clone(),
            state_updated_at: settlement.state_updated_at,
        },
        response_message_id,
    }
}

async fn classify_terminal_mutation(
    db: &crate::db::Database,
    evidence: &phoenix_db::workflow::TerminalEvidenceExpectation,
    obligation: &phoenix_db::workflow::DirectTurnTerminalObligationInput,
    command_error: String,
    _transcript_generation: Option<i64>,
) -> TerminalMutationEstablishment {
    let repo = db.workflow_repository();
    match repo
        .probe_exact_terminal_evidence(evidence, obligation)
        .await
    {
        Ok(phoenix_db::workflow::TerminalEvidenceProbe::Established {
            transcript_generation,
        }) => TerminalMutationEstablishment::Established {
            transcript_generation,
        },
        Ok(phoenix_db::workflow::TerminalEvidenceProbe::Retired) => {
            TerminalMutationEstablishment::Retired
        }
        Ok(phoenix_db::workflow::TerminalEvidenceProbe::KnownNotCommitted) => {
            TerminalMutationEstablishment::KnownNotCommitted(command_error)
        }
        Ok(phoenix_db::workflow::TerminalEvidenceProbe::Incomplete) => {
            TerminalMutationEstablishment::Unclassifiable(format!(
                "terminal evidence command failed: {command_error}; exact evidence is incomplete"
            ))
        }
        Err(probe_error) => TerminalMutationEstablishment::Unclassifiable(format!(
            "terminal evidence command failed: {command_error}; exact probe failed: {probe_error}"
        )),
    }
}

fn direct_turn_local_authority(
    authority: &phoenix_core::domain::sm_event::DirectTurnAttemptAuthority,
) -> phoenix_db::LocalAttemptAuthority {
    use phoenix_workflow::{
        AttemptId, EffectId, Generation, ProcessIncarnation, Version, WorkflowId,
    };

    phoenix_db::LocalAttemptAuthority {
        workflow_id: WorkflowId(authority.workflow_id.0),
        declared_workflow_version: Version(authority.declared_workflow_version.0),
        generation: Generation(authority.generation.0),
        effect_id: EffectId(authority.effect_id.0),
        attempt_id: AttemptId(authority.attempt_id.0),
        process_incarnation: ProcessIncarnation(authority.process_incarnation.0),
    }
}

#[async_trait]
impl StateStore for DatabaseStorage {
    async fn establish_parent_reconcile_action(&self, conversation_id: &str) -> Result<(), String> {
        self.db
            .establish_parent_reconcile_action(conversation_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<(), String> {
        self.db
            .update_conversation_state_at(conv_id, state, state_updated_at)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        let conv = self
            .db
            .get_conversation(conv_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(conv.state)
    }

    async fn get_state_snapshot(&self, conv_id: &str) -> Result<PersistedStateSnapshot, String> {
        let conv = self
            .db
            .get_conversation(conv_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(PersistedStateSnapshot {
            state: conv.state,
            state_updated_at: conv.state_updated_at,
        })
    }

    async fn begin_continuation(
        &self,
        conv_id: &str,
        operation_id: &str,
        message: &crate::db::Message,
        awaiting_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        self.db
            .begin_continuation(
                conv_id,
                operation_id,
                message,
                awaiting_state,
                state_updated_at,
            )
            .await
            .map_err(|error| error.to_string())
    }

    async fn recover_continuation_start(
        &self,
        settlement: &ContinuationStartRecoverySettlement,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        if let (Some(turn), Some(terminal)) = (&settlement.turn, &settlement.terminal) {
            let repo = self.db.workflow_repository();
            repo.settle_failed_continuation_start_atomically(
                &phoenix_db::workflow::AtomicContinuationSettlementInput {
                    conversation_id: settlement.message.conversation_id.clone(),
                    operation_id: settlement.operation_id.clone(),
                    message: settlement.message.clone(),
                    completed_state: settlement.state.clone(),
                    state_updated_at: settlement.state_updated_at,
                    command: direct_turn_terminal_command(turn, terminal.clone()),
                },
            )
            .await
            .map_err(|error| error.to_string())
        } else {
            self.db
                .recover_continuation_start(
                    &settlement.message.conversation_id,
                    &settlement.operation_id,
                    &settlement.message,
                    &settlement.state,
                    settlement.state_updated_at,
                )
                .await
                .map_err(|error| error.to_string())
        }
    }

    async fn commit_continuation(
        &self,
        conv_id: &str,
        operation_id: &str,
        message: &crate::db::Message,
        completed_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<crate::db::ContinuationCommitOutcome, String> {
        self.db
            .commit_continuation(
                conv_id,
                operation_id,
                message,
                completed_state,
                state_updated_at,
            )
            .await
            .map_err(|error| error.to_string())
    }

    async fn persist_approved_task_authority(
        &self,
        conv_id: &str,
        approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
    ) -> Result<(), String> {
        self.db
            .persist_approved_task_authority(conv_id, approval)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_conversation_mode(&self, conv_id: &str) -> Result<ConvMode, String> {
        let conv = self
            .db
            .get_conversation(conv_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(conv.conv_mode)
    }

    async fn update_conversation_cwd_recovery_only(
        &self,
        conv_id: &str,
        cwd: &str,
    ) -> Result<(), String> {
        self.db
            .update_conversation_cwd_recovery_only(conv_id, cwd)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_clear_watermark(&self, conv_id: &str) -> Result<i64, String> {
        self.db
            .get_clear_watermark(conv_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn set_clear_watermark(&self, conv_id: &str, watermark: i64) -> Result<(), String> {
        self.db
            .update_clear_watermark(conv_id, watermark)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_last_turn_prompt_tokens(&self, conv_id: &str) -> Result<Option<i64>, String> {
        self.db
            .get_last_turn_prompt_tokens(conv_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        effective_effort: phoenix_core::domain::llm_types::EffectiveEffort,
        usage: &phoenix_llm::Usage,
        first_byte_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), String> {
        self.db
            .insert_turn_usage(
                conversation_id,
                root_conversation_id,
                model,
                effective_effort,
                usage,
                first_byte_at,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn upsert_llm_request_metrics(
        &self,
        metrics: &phoenix_llm::LlmAttemptMetrics,
    ) -> Result<(), String> {
        self.db
            .upsert_llm_request_metrics(metrics)
            .await
            .map_err(|e| e.to_string())
    }

    async fn load_steering_entries(
        &self,
        conv_id: &str,
    ) -> Result<Vec<phoenix_core::domain::sm_event::SteerEntry>, String> {
        self.db
            .get_steering_queue(conv_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn commit_steering_drain(
        &self,
        conv_id: &str,
        messages: &[crate::db::Message],
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> Result<Vec<crate::db::SteeringDrainMessageStatus>, String> {
        self.db
            .commit_steering_drain(conv_id, messages, state, state_updated_at)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Adapter to use `ModelRegistry` as `LlmClient`
pub struct RegistryLlmClient {
    registry: Arc<ModelRegistry>,
    model_id: String,
}

impl RegistryLlmClient {
    pub fn new(registry: Arc<ModelRegistry>, model_id: String) -> Self {
        Self { registry, model_id }
    }
}

#[async_trait]
impl LlmClient for RegistryLlmClient {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let llm = self.registry.get(&self.model_id).ok_or_else(|| {
            LlmError::network(format!(
                "Model '{}' is not available in the registry",
                self.model_id
            ))
        })?;
        llm.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: &LlmRequest,
        chunk_tx: &tokio::sync::mpsc::Sender<phoenix_llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let llm = self.registry.get(&self.model_id).ok_or_else(|| {
            LlmError::network(format!(
                "Model '{}' is not available in the registry",
                self.model_id
            ))
        })?;
        llm.complete_streaming(request, chunk_tx).await
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn continuation_request_limits(&self) -> phoenix_llm::ContinuationRequestLimits {
        self.registry.get(&self.model_id).map_or(
            phoenix_llm::ContinuationRequestLimits::TokenWindowOnly,
            |llm| llm.continuation_request_limits(),
        )
    }
}

struct PublishedToolCapability {
    generation: ToolCapabilityGeneration,
    authority: phoenix_core::work_scope::ResourceAuthority,
    registry: ToolRegistry,
    clearable_names: Arc<std::collections::HashSet<String>>,
}

impl PublishedToolCapability {
    fn new(authority: phoenix_core::work_scope::ResourceAuthority, registry: ToolRegistry) -> Self {
        let clearable_names = Arc::new(registry.clearable_tool_names());
        Self {
            generation: ToolCapabilityGeneration::INITIAL,
            authority,
            registry,
            clearable_names,
        }
    }

    fn snapshot(&self) -> ToolCapabilitySnapshot {
        ToolCapabilitySnapshot {
            generation: self.generation,
            authority: self.authority,
            clearable_names: Arc::clone(&self.clearable_names),
        }
    }
}

/// Adapter to use `ToolRegistry` as `ToolExecutor`
///
/// Uses `RwLock` for interior mutability so the registry can be swapped
/// at runtime (e.g., Explore -> Work mode transition after task approval).
pub struct ToolRegistryExecutor {
    capability: std::sync::RwLock<PublishedToolCapability>,
    /// When set, MCP tools are resolved live from the manager on every
    /// `definitions()` and `execute()` call instead of being snapshotted
    /// into the registry. This means enable/disable and reload take effect
    /// immediately across all conversations.
    mcp_manager: Option<Arc<crate::tools::mcp::McpClientManager>>,
    /// The named-agent catalog frozen at conversation start. Reused when
    /// upgrading Explore → Work so the rebuilt Work registry's `spawn_agents`
    /// tool advertises the *same* `agent_type` enum the executor resolves
    /// against, instead of re-discovering the filesystem (REQ-AG-004/008).
    /// Empty for sub-agents.
    agent_catalog: Arc<[phoenix_agents::AgentDefinition]>,
    model_ids: Arc<[String]>,
    writing_tools: Option<WritingConversationTools>,
}

impl ToolRegistryExecutor {
    /// Create an executor with built-in tools only (no MCP).
    /// Used for sub-agents which have a restricted tool set.
    #[allow(dead_code)]
    pub fn builtin_only(
        authority: phoenix_core::work_scope::ResourceAuthority,
        registry: ToolRegistry,
        agent_catalog: Arc<[phoenix_agents::AgentDefinition]>,
    ) -> Self {
        Self {
            capability: std::sync::RwLock::new(PublishedToolCapability::new(authority, registry)),
            mcp_manager: None,
            agent_catalog,
            model_ids: Arc::from(Vec::new()),
            writing_tools: None,
        }
    }

    /// Create an executor with built-in tools + live MCP tool resolution.
    /// MCP tools are resolved from the manager on every `definitions()` and
    /// `execute()` call, so enable/disable and reload take effect immediately.
    pub fn with_mcp(
        authority: phoenix_core::work_scope::ResourceAuthority,
        registry: ToolRegistry,
        manager: Arc<crate::tools::mcp::McpClientManager>,
        agent_catalog: Arc<[phoenix_agents::AgentDefinition]>,
        model_ids: Arc<[String]>,
    ) -> Self {
        Self {
            capability: std::sync::RwLock::new(PublishedToolCapability::new(authority, registry)),
            mcp_manager: Some(manager),
            agent_catalog,
            model_ids,
            writing_tools: None,
        }
    }

    #[must_use]
    pub fn with_writing_tools(mut self, tools: Option<WritingConversationTools>) -> Self {
        self.writing_tools = tools;
        self
    }

    fn publish_work_registry(&self, registry: ToolRegistry) -> ToolCapabilitySnapshot {
        let clearable_names = Arc::new(registry.clearable_tool_names());
        let mut guard = self
            .capability
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.generation = guard.generation.next();
        guard.authority = phoenix_core::work_scope::ResourceAuthority::Work;
        guard.registry = registry;
        guard.clearable_names = clearable_names;
        guard.snapshot()
    }
}

#[async_trait]
impl ToolExecutor for ToolRegistryExecutor {
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        // Look up the tool while holding the read lock, then drop the guard
        // before the async .run() call (RwLockReadGuard is !Send).
        let tool = {
            let capability = self
                .capability
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            capability.registry.find_tool(&name)
        };
        if let Some(t) = tool {
            return Some(t.run(input, ctx).await);
        }

        // Fall back to live MCP tool resolution.
        if let Some(ref manager) = self.mcp_manager {
            if let Some(mcp_tool) = crate::tools::mcp::create_mcp_tool_by_name(manager, &name).await
            {
                return Some(mcp_tool.run(input, ctx).await);
            }
        }

        None
    }

    async fn definitions(&self) -> Vec<phoenix_llm::ToolDefinition> {
        self.definitions_for_language(crate::llm_language::LlmLanguage::default())
            .await
    }

    async fn definitions_for_language(
        &self,
        language: crate::llm_language::LlmLanguage,
    ) -> Vec<phoenix_llm::ToolDefinition> {
        let mut defs = {
            let capability = self
                .capability
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            capability.registry.definitions_for_language(language)
        };

        // Merge live MCP tool definitions (respects current disabled state).
        // Built-in names are checked to prevent shadowing; MCP full names
        // are also tracked to detect cross-server collisions.
        if let Some(ref manager) = self.mcp_manager {
            let mut seen_names: std::collections::HashSet<String> =
                defs.iter().map(|d| d.name.clone()).collect();

            for (server_name, tool_def) in manager.tool_definitions().await {
                let full_name = format!("{server_name}__{}", tool_def.name);
                if seen_names.contains(&full_name) {
                    tracing::debug!(
                        tool = %full_name,
                        "MCP tool name conflicts with existing tool, skipping"
                    );
                    continue;
                }
                seen_names.insert(full_name.clone());
                defs.push(phoenix_llm::ToolDefinition {
                    name: full_name,
                    description: tool_def.description,
                    input_schema: tool_def.input_schema,
                    defer_loading: true,
                });
            }
        }

        if defs.len() > 50 {
            let deferred = defs.iter().filter(|d| d.defer_loading).count();
            if deferred == 0 {
                tracing::warn!(
                    total = defs.len(),
                    "Tool count exceeds 50 with no deferred tools -- accuracy may degrade. \
                     Consider disabling unused MCP servers or using a model that supports tool search."
                );
            }
        }

        defs
    }

    fn capability_snapshot(&self) -> ToolCapabilitySnapshot {
        self.capability
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot()
    }

    async fn execute_at_generation(
        &self,
        expected: ToolCapabilityGeneration,
        call: CheckedToolCall,
        ctx: ToolContext,
    ) -> Result<Option<ToolOutput>, String> {
        let (name, input) = call.into_parts();
        let tool = {
            let capability = self
                .capability
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if capability.generation != expected {
                return Err("stale tool capability generation".to_string());
            }
            capability.registry.find_tool(&name)
        };
        if self.capability_snapshot().generation != expected {
            return Err("stale tool capability generation".to_string());
        }
        if let Some(tool) = tool {
            return Ok(Some(tool.run(input, ctx).await));
        }
        if let Some(ref manager) = self.mcp_manager {
            if let Some(tool) = crate::tools::mcp::create_mcp_tool_by_name(manager, &name).await {
                if self.capability_snapshot().generation != expected {
                    return Err("stale tool capability generation".to_string());
                }
                return Ok(Some(tool.run(input, ctx).await));
            }
        }
        Ok(None)
    }

    fn subagent_model_ids(&self) -> Arc<[String]> {
        self.model_ids.clone()
    }

    fn upgrade_to_work_mode(&self) -> Result<ToolCapabilitySnapshot, String> {
        let mut registry =
            ToolRegistry::direct(self.agent_catalog.to_vec(), self.model_ids.to_vec());
        if let Some(tools) = self.writing_tools.clone() {
            registry = registry.try_with_writing_conversation_tools(tools)?;
        }
        Ok(self.publish_work_registry(registry))
    }
}

#[cfg(test)]
mod tool_registry_executor_tests {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolOutput};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct NamedMarker(&'static str);

    #[async_trait]
    impl Tool for NamedMarker {
        fn name(&self) -> &'static str {
            self.0
        }

        fn description(&self) -> String {
            "test marker".to_string()
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn run(&self, _input: Value, _ctx: ToolContext) -> ToolOutput {
            ToolOutput::success("ok")
        }
    }

    fn tool_context_for_path(
        path: &std::path::Path,
        authority: phoenix_core::work_scope::ResourceAuthority,
    ) -> ToolContext {
        ToolContext::new_with_resource_access(
            tokio_util::sync::CancellationToken::new(),
            "capability-e2e".to_string(),
            path.to_path_buf(),
            Arc::new(crate::tools::browser::BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            Arc::new(crate::tools::TmuxRegistry::default()),
            Some(path.to_path_buf()),
            crate::work_scope::WorkScopeId::new(),
            authority,
        )
    }

    fn tool_output_text(output: ToolOutput) -> String {
        match output {
            ToolOutput::Success { output, .. } | ToolOutput::Error { output, .. } => output,
        }
    }

    #[tokio::test]
    async fn same_executor_after_approval_can_mutate_all_work_paths() {
        use std::process::Command;

        let repo = tempfile::TempDir::new().unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "capability@test.invalid"],
            vec!["config", "user.name", "Capability Test"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success());
        }
        std::fs::create_dir(repo.path().join("tasks")).unwrap();
        std::fs::write(
            repo.path().join("tasks/10000-p0-ready--fixture.md"),
            "# fixture\n",
        )
        .unwrap();
        std::fs::write(repo.path().join("README.md"), "fixture\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "."])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["commit", "-m", "fixture"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());

        let executor = ToolRegistryExecutor::builtin_only(
            phoenix_core::work_scope::ResourceAuthority::Restricted,
            ToolRegistry::explore(
                "tasks",
                Vec::new(),
                Vec::new(),
                crate::tools::ExploreToolPolicy::from_platform(
                    &crate::platform::PlatformCapability::detect(),
                ),
            ),
            Arc::from(Vec::new()),
        );
        let work = executor.upgrade_to_work_mode().unwrap();
        let command = r#"
            test -z "${PHOENIX_SANDBOX_SCRATCH:-}"
            mv tasks/10000-p0-ready--fixture.md tasks/10000-p0-in-progress--fixture.md
            common=$(git rev-parse --git-common-dir)
            : > "$common/capability-transition.lock"
            rm "$common/capability-transition.lock"
            mkdir -p target/codegen .uv/cache
            printf generated > target/codegen/output
            printf cached > .uv/cache/output
            printf committed > approved.txt
            git add tasks approved.txt
            git commit -m approved-capability
        "#;
        let call = CheckedToolCall::cleared_for_test(
            "bash".to_string(),
            serde_json::json!({"op":"run", "cmd":command, "wait_seconds":30}),
        );
        let output = executor
            .execute_at_generation(
                work.generation,
                call,
                tool_context_for_path(repo.path(), work.authority),
            )
            .await
            .unwrap()
            .unwrap();
        let text = tool_output_text(output);
        assert!(text.contains("\"exit_code\":0"), "{text}");
        assert!(repo
            .path()
            .join("tasks/10000-p0-in-progress--fixture.md")
            .exists());
        assert!(repo.path().join("target/codegen/output").exists());
        assert!(repo.path().join(".uv/cache/output").exists());
        let subject = Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "approved-capability"
        );
    }

    #[tokio::test]
    async fn unapproved_explore_remains_sandboxed() {
        let repo = tempfile::TempDir::new().unwrap();
        let executor = ToolRegistryExecutor::builtin_only(
            phoenix_core::work_scope::ResourceAuthority::Restricted,
            ToolRegistry::explore(
                "tasks",
                Vec::new(),
                Vec::new(),
                crate::tools::ExploreToolPolicy::from_platform(
                    &crate::platform::PlatformCapability::detect(),
                ),
            ),
            Arc::from(Vec::new()),
        );
        let definitions = executor.definitions().await;
        if definitions
            .iter()
            .any(|definition| definition.name == "bash")
        {
            let call = CheckedToolCall::cleared_for_test(
                "bash".to_string(),
                serde_json::json!({
                    "op":"run",
                    "cmd":"test -n \"${PHOENIX_SANDBOX_SCRATCH:-}\" && ! touch source-denied",
                    "wait_seconds":30
                }),
            );
            let snapshot = executor.capability_snapshot();
            let output = executor
                .execute_at_generation(
                    snapshot.generation,
                    call,
                    tool_context_for_path(repo.path(), snapshot.authority),
                )
                .await
                .unwrap()
                .unwrap();
            let _text = tool_output_text(output);
            assert!(
                !repo.path().join("source-denied").exists(),
                "Restricted Explore Bash must not mutate the source tree"
            );
        }
        assert_eq!(
            executor.capability_snapshot().authority,
            phoenix_core::work_scope::ResourceAuthority::Restricted
        );
    }

    #[tokio::test]
    async fn explore_upgrade_preserves_host_bound_writing_tools() {
        let executor = ToolRegistryExecutor::builtin_only(
            phoenix_core::work_scope::ResourceAuthority::Restricted,
            ToolRegistry::explore(
                "tasks",
                Vec::new(),
                Vec::new(),
                crate::tools::ExploreToolPolicy::from_platform(
                    &phoenix_core::platform::PlatformCapability::None {
                        details: "test".to_string(),
                    },
                ),
            ),
            Arc::from(Vec::new()),
        )
        .with_writing_tools(Some(
            WritingConversationTools::new(
                Arc::new(NamedMarker("search_conversations")),
                Arc::new(NamedMarker("read_conversation")),
                Arc::new(NamedMarker("query_database")),
                Arc::new(NamedMarker("send_conversation_message")),
            )
            .unwrap(),
        ));

        assert!(!executor
            .definitions()
            .await
            .iter()
            .any(|definition| definition.name == "search_conversations"));
        let snapshot = executor.upgrade_to_work_mode().unwrap();
        assert_eq!(
            snapshot.authority,
            phoenix_core::work_scope::ResourceAuthority::Work
        );
        assert_eq!(snapshot.generation.value(), 1);
        assert_eq!(
            executor.capability_snapshot().generation,
            snapshot.generation
        );
        assert!(
            executor
                .definitions_for_generation(
                    ToolCapabilityGeneration::INITIAL,
                    crate::llm_language::LlmLanguage::default(),
                )
                .await
                .is_err(),
            "a precreated provider surface must not cross capability publication"
        );
        let stale_call = crate::runtime::deny_gate::CheckedToolCall::cleared_for_test(
            "think".to_string(),
            serde_json::json!({"thoughts": "stale"}),
        );
        let stale_context = crate::runtime::testing::test_tool_context_for_authority(
            phoenix_core::work_scope::ResourceAuthority::Restricted,
        );
        assert!(
            executor
                .execute_at_generation(
                    ToolCapabilityGeneration::INITIAL,
                    stale_call,
                    stale_context,
                )
                .await
                .is_err(),
            "a precreated tool call must not cross capability publication"
        );
        assert!(executor
            .definitions()
            .await
            .iter()
            .any(|definition| definition.name == "search_conversations"));
    }
}
