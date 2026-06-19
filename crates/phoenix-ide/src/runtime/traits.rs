//! Trait abstractions for runtime I/O
//!
//! These traits enable testing the executor with mock implementations.

use crate::db::{ConvMode, Message, MessageContent, UsageData};
use crate::llm::{LlmError, LlmRequest, LlmResponse};
use crate::state_machine::ConvState;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use serde_json::Value;

/// Storage for conversation messages
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Add a message to the conversation
    ///
    /// `message_id` is the canonical identifier for this message. For user messages,
    /// this is client-generated (enabling idempotent retries). For agent/tool messages,
    /// this is server-generated.
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

    /// Get a single message by ID
    #[allow(dead_code)]
    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String>;

    /// Returns true if a message with the given `message_id` already exists.
    /// Used by `PersistMessage` to make persistence idempotent across crash
    /// recovery (re-drain after partial steering-queue drain).
    async fn message_exists(&self, message_id: &str) -> Result<bool, String>;

    /// Update `display_data` for an existing message
    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<(), String>;

    /// Update the `content` text inside a tool result message's JSON.
    /// Used to write sub-agent outcomes into the `spawn_agents` tool result before
    /// the LLM is called, so the results appear in the conversation history.
    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<(), String>;

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
}

/// Storage for conversation state
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Update the conversation state (full state as JSON). `state_updated_at`
    /// is the runtime's authoritative phase-entry timestamp; persisting it
    /// (rather than a fresh `now()` at the storage layer) keeps the DB row and
    /// the `StateChange` SSE on one value (REQ-WPV-001).
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String>;

    /// Get the current conversation state
    #[allow(dead_code)] // API completeness
    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String>;

    /// Update the conversation mode (e.g., Explore -> Work on task approval)
    async fn update_conversation_mode(&self, conv_id: &str, mode: &ConvMode) -> Result<(), String>;

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

    /// Record token usage for one LLM turn. Fire-and-forget; errors are logged
    /// by the caller and do not affect the conversation.
    async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        usage: &crate::llm::Usage,
    ) -> Result<(), String>;

    /// Update the steering queue for a conversation. Persists the FIFO queue
    /// of pending steering messages to the DB.
    async fn update_steering_queue(
        &self,
        conv_id: &str,
        queue: &[crate::state_machine::event::SteerEntry],
    ) -> Result<(), String>;

    /// Remove specific drained entries from the persisted steering queue,
    /// preserving any concurrently-enqueued entries. Implementations must be
    /// atomic re: `enqueue_steer_message`'s read-modify-write to avoid losing
    /// a steer queued during the drain window.
    async fn remove_steering_entries(
        &self,
        conv_id: &str,
        message_ids: &[String],
    ) -> Result<(), String>;
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
        chunk_tx: &tokio::sync::broadcast::Sender<crate::llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        let _ = chunk_tx;
        self.complete(request).await
    }

    /// Get the model ID
    #[allow(dead_code)] // API completeness
    fn model_id(&self) -> &str;
}

use crate::runtime::deny_gate::CheckedToolCall;
use crate::tools::ToolContext;

/// Executor for tools
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a gate-cleared tool call. Accepting only a [`CheckedToolCall`]
    /// — whose sole non-test mint is `DenyGate::check` — makes an ungated tool
    /// call unrepresentable (specs/permissions REQ-PERM-001).
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput>;

    /// Get tool definitions for LLM (phoenix-native).
    async fn definitions(&self) -> Vec<crate::llm::ToolDefinition>;

    /// Get tool definitions in the requested LLM language. Default impl
    /// delegates to `definitions()` (phoenix-native); production overrides
    /// translate tool descriptions per-language.
    async fn definitions_for_language(
        &self,
        _language: crate::llm_language::LlmLanguage,
    ) -> Vec<crate::llm::ToolDefinition> {
        self.definitions().await
    }

    /// Names of tools whose stale results may be cleared from the model-bound
    /// history (specs/stale-tool-results). Default empty so a test double opts
    /// out of clearing unless it overrides; the production registry executor
    /// derives the set from `Tool::clearable()`.
    fn clearable_tool_names(&self) -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
    }

    /// Replace the tool set (e.g., Explore -> Work mode transition).
    /// Default is a no-op for test doubles that don't need dynamic swapping.
    fn upgrade_to_work_mode(&self) {
        // No-op by default
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

    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String> {
        (**self).get_message_by_id(message_id).await
    }

    async fn message_exists(&self, message_id: &str) -> Result<bool, String> {
        (**self).message_exists(message_id).await
    }

    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<(), String> {
        (**self)
            .update_message_display_data(message_id, display_data)
            .await
    }

    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<(), String> {
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
}

#[async_trait]
impl<T: StateStore + ?Sized> StateStore for Arc<T> {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String> {
        (**self)
            .update_state(conv_id, state, state_updated_at)
            .await
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        (**self).get_state(conv_id).await
    }

    async fn update_conversation_mode(&self, conv_id: &str, mode: &ConvMode) -> Result<(), String> {
        (**self).update_conversation_mode(conv_id, mode).await
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

    async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        usage: &crate::llm::Usage,
    ) -> Result<(), String> {
        (**self)
            .insert_turn_usage(conversation_id, root_conversation_id, model, usage)
            .await
    }

    async fn update_steering_queue(
        &self,
        conv_id: &str,
        queue: &[crate::state_machine::event::SteerEntry],
    ) -> Result<(), String> {
        (**self).update_steering_queue(conv_id, queue).await
    }

    async fn remove_steering_entries(
        &self,
        conv_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        (**self).remove_steering_entries(conv_id, message_ids).await
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
        chunk_tx: &tokio::sync::broadcast::Sender<crate::llm::TokenChunk>,
    ) -> Result<LlmResponse, LlmError> {
        (**self).complete_streaming(request, chunk_tx).await
    }

    fn model_id(&self) -> &str {
        (**self).model_id()
    }
}

#[async_trait]
impl<T: ToolExecutor + ?Sized> ToolExecutor for Arc<T> {
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput> {
        (**self).execute(call, ctx).await
    }

    async fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        (**self).definitions().await
    }

    async fn definitions_for_language(
        &self,
        language: crate::llm_language::LlmLanguage,
    ) -> Vec<crate::llm::ToolDefinition> {
        (**self).definitions_for_language(language).await
    }

    fn upgrade_to_work_mode(&self) {
        (**self).upgrade_to_work_mode();
    }
}

// ============================================================================
// Production Adapters
// ============================================================================

use crate::db::Database;
use crate::llm::ModelRegistry;
use crate::tools::ToolRegistry;
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

#[async_trait]
impl MessageStore for DatabaseStorage {
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

    async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &Value,
    ) -> Result<(), String> {
        self.db
            .update_message_display_data(message_id, display_data)
            .await
            .map_err(|e| e.to_string())
    }

    async fn update_tool_message_content(
        &self,
        message_id: &str,
        content: &str,
    ) -> Result<(), String> {
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
}

#[async_trait]
impl StateStore for DatabaseStorage {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
        state_updated_at: chrono::DateTime<chrono::Utc>,
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

    async fn update_conversation_mode(&self, conv_id: &str, mode: &ConvMode) -> Result<(), String> {
        self.db
            .update_conversation_mode(conv_id, mode)
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

    async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        usage: &crate::llm::Usage,
    ) -> Result<(), String> {
        self.db
            .insert_turn_usage(conversation_id, root_conversation_id, model, usage)
            .await
            .map_err(|e| e.to_string())
    }

    async fn update_steering_queue(
        &self,
        conv_id: &str,
        queue: &[crate::state_machine::event::SteerEntry],
    ) -> Result<(), String> {
        self.db
            .update_steering_queue(conv_id, queue)
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_steering_entries(
        &self,
        conv_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        self.db
            .remove_steering_entries(conv_id, message_ids)
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
        chunk_tx: &tokio::sync::broadcast::Sender<crate::llm::TokenChunk>,
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
}

/// Adapter to use `ToolRegistry` as `ToolExecutor`
///
/// Uses `RwLock` for interior mutability so the registry can be swapped
/// at runtime (e.g., Explore -> Work mode transition after task approval).
pub struct ToolRegistryExecutor {
    registry: std::sync::RwLock<ToolRegistry>,
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
}

impl ToolRegistryExecutor {
    /// Create an executor with built-in tools only (no MCP).
    /// Used for sub-agents which have a restricted tool set.
    #[allow(dead_code)]
    pub fn builtin_only(
        registry: ToolRegistry,
        agent_catalog: Arc<[phoenix_agents::AgentDefinition]>,
    ) -> Self {
        Self {
            registry: std::sync::RwLock::new(registry),
            mcp_manager: None,
            agent_catalog,
        }
    }

    /// Create an executor with built-in tools + live MCP tool resolution.
    /// MCP tools are resolved from the manager on every `definitions()` and
    /// `execute()` call, so enable/disable and reload take effect immediately.
    pub fn with_mcp(
        registry: ToolRegistry,
        manager: Arc<crate::tools::mcp::McpClientManager>,
        agent_catalog: Arc<[phoenix_agents::AgentDefinition]>,
    ) -> Self {
        Self {
            registry: std::sync::RwLock::new(registry),
            mcp_manager: Some(manager),
            agent_catalog,
        }
    }

    /// Replace the inner `ToolRegistry` (e.g., after Explore -> Work mode transition).
    pub fn swap_registry(&self, new_registry: ToolRegistry) {
        let mut guard = self
            .registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = new_registry;
    }
}

#[async_trait]
impl ToolExecutor for ToolRegistryExecutor {
    async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput> {
        let (name, input) = call.into_parts();
        // Look up the tool while holding the read lock, then drop the guard
        // before the async .run() call (RwLockReadGuard is !Send).
        let tool = {
            let registry = self
                .registry
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.find_tool(&name)
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

    async fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        self.definitions_for_language(crate::llm_language::LlmLanguage::default())
            .await
    }

    fn clearable_tool_names(&self) -> std::collections::HashSet<String> {
        self.registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clearable_tool_names()
    }

    async fn definitions_for_language(
        &self,
        language: crate::llm_language::LlmLanguage,
    ) -> Vec<crate::llm::ToolDefinition> {
        let mut defs = {
            let registry = self
                .registry
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.definitions_for_language(language)
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
                defs.push(crate::llm::ToolDefinition {
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

    fn upgrade_to_work_mode(&self) {
        // Reuse the frozen catalog so the upgraded registry advertises the same
        // agent_type enum the executor resolves against (REQ-AG-008).
        self.swap_registry(ToolRegistry::direct(self.agent_catalog.to_vec()));
        tracing::info!("Tool registry upgraded to Work mode (full tool suite)");
    }
}
