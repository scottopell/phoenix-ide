//! Runtime for executing conversations
//!
//! REQ-BED-007: State Persistence
//! REQ-BED-010: Fixed Working Directory
//! REQ-BED-011: Real-time Event Streaming

#![allow(dead_code)] // browser_sessions() will be used when browser cleanup is wired up
//! REQ-BED-012: Context Window Tracking
//! REQ-BED-008: Sub-Agent Spawning
//! REQ-BED-009: Sub-Agent Isolation

pub(crate) mod executor;
mod recovery;
pub mod traits;
pub mod user_facing_error;

#[cfg(test)]
pub mod testing;

pub use executor::ConversationRuntime;
pub use traits::*;

use crate::platform::PlatformCapability;
use crate::state_machine::state::{ModeKind, SubAgentMode, SubAgentOutcome, SubAgentSpec};
use crate::tools::{BashHandleRegistry, BrowserSessionManager, ToolRegistry};

/// Type alias for production runtime with concrete implementations
pub type ProductionRuntime =
    ConversationRuntime<DatabaseStorage, RegistryLlmClient, ToolRegistryExecutor>;

use crate::db::{ConvMode, Database};
use crate::llm::ModelRegistry;
use crate::state_machine::{ConvContext, ConvState, Event};
use crate::system_prompt::ModeContext;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

/// Request to spawn a sub-agent
#[derive(Debug)]
pub struct SubAgentSpawnRequest {
    pub spec: SubAgentSpec,
    pub parent_conversation_id: String,
    pub parent_event_tx: mpsc::Sender<Event>,
}

/// Request to cancel sub-agents
#[derive(Debug)]
pub struct SubAgentCancelRequest {
    pub ids: Vec<String>,
    #[allow(dead_code)] // Used for logging/debugging
    pub parent_conversation_id: String,
    pub parent_event_tx: mpsc::Sender<Event>,
}

/// Manager for all conversation runtimes
pub struct RuntimeManager {
    db: Database,
    llm_registry: Arc<ModelRegistry>,
    platform: PlatformCapability,
    browser_sessions: Arc<BrowserSessionManager>,
    /// Per-process bash handle registry. Shared by every conversation's
    /// `ToolContext`; each conversation gets its own `ConversationHandles`
    /// table inside (REQ-BASH-014).
    bash_handles: Arc<BashHandleRegistry>,
    mcp_manager: Arc<crate::tools::mcp::McpClientManager>,
    /// Active PTY terminal sessions — threaded into `ToolContext` for `read_terminal`.
    pub terminals: crate::terminal::ActiveTerminals,
    runtimes: RwLock<HashMap<String, ConversationHandle>>,
    /// Channel for sub-agent spawn requests
    spawn_tx: mpsc::Sender<SubAgentSpawnRequest>,
    spawn_rx: RwLock<Option<mpsc::Receiver<SubAgentSpawnRequest>>>,
    /// Channel for sub-agent cancel requests
    cancel_tx: mpsc::Sender<SubAgentCancelRequest>,
    cancel_rx: RwLock<Option<mpsc::Receiver<SubAgentCancelRequest>>>,
    /// Credential helper for recovery settlement (REQ-BED-030).
    credential_helper: Option<Arc<crate::llm::CredentialHelper>>,
}

/// Handle to interact with a running conversation
pub struct ConversationHandle {
    pub event_tx: mpsc::Sender<Event>,
    /// SSE broadcaster. Owns the per-conversation monotonic `sequence_id` counter
    /// that every emitted [`SseEvent`] must consume (task 02675). Callers never
    /// hand-craft a `sequence_id` — they either go through [`SseBroadcaster::send_seq`]
    /// (which allocates the next id from the counter) or [`SseBroadcaster::send_message`]
    /// (which passes through the DB-allocated message id and advances the counter past
    /// it). This makes the "every SSE event carries a monotonic `sequence_id`" contract
    /// structurally enforceable rather than a matter of caller discipline.
    pub broadcast_tx: SseBroadcaster,
}

/// Capacity of the per-conversation SSE broadcast channel.
///
/// Sized to cover a realistic worst-case stall of the slowest receiver
/// (a background tab, a sleeping laptop resume, a long GC pause) during
/// active LLM streaming. At ~50 tokens/sec this buys ~80 seconds of headroom.
///
/// When the channel overflows, `BroadcastStreamRecvError::Lagged` fires on
/// the receive side. We handle that in `api::sse::sse_stream` by closing the
/// stream — the client reconnects, `init` replays current state, and no
/// silent gap results. Increasing this value reduces how often that resync
/// dance happens; it does not change correctness.
pub const SSE_BROADCAST_CAPACITY: usize = 4096;

/// Per-conversation SSE broadcaster with monotonic `sequence_id` allocation.
///
/// Every [`SseEvent`] emitted for a conversation carries a `sequence_id` drawn
/// from a single per-conversation counter. This broadcaster is the sole
/// gateway: callers cannot construct a [`SseEvent`] and broadcast it without
/// first obtaining a `sequence_id` from here, which means the total-order
/// invariant is enforced by the type rather than by caller discipline.
///
/// Two broadcast paths exist:
///
/// 1. **Ephemeral/derived events** (`Token`, `StateChange`, `MessageUpdated`, …) —
///    allocate a fresh id via [`SseBroadcaster::next_seq`] or use
///    [`SseBroadcaster::send_seq`], which hands the id to a construction
///    closure so the caller cannot forget to insert it.
///
/// 2. **Persisted `Message` events** already carry a `message.sequence_id`
///    allocated by `add_message` in the DB layer. Use
///    [`SseBroadcaster::send_message`], which reuses that id and atomically
///    advances the broadcaster's counter past it so ephemeral events emitted
///    afterwards are ordered strictly after the message.
#[derive(Clone)]
pub struct SseBroadcaster {
    tx: broadcast::Sender<SseEvent>,
    /// Highest `sequence_id` emitted so far for this conversation.
    /// `next_seq()` returns `fetch_add(1)` + 1 atomically; `observe_seq(s)`
    /// bumps this value up to at least `s` so message-originated ids integrate
    /// into the same total order.
    last_seq: Arc<AtomicI64>,
}

impl SseBroadcaster {
    /// Build a broadcaster from an existing `broadcast::Sender`.
    ///
    /// `initial_last_seq` is the highest `sequence_id` the client can already
    /// have observed (typically `db.get_last_sequence_id(conversation_id)`).
    /// The next allocated id will be `initial_last_seq + 1`.
    pub fn from_sender(tx: broadcast::Sender<SseEvent>, initial_last_seq: i64) -> Self {
        Self {
            tx,
            last_seq: Arc::new(AtomicI64::new(initial_last_seq)),
        }
    }

    /// Construct a broadcaster with a fresh broadcast channel.
    /// Convenience for call sites that also need the underlying channel's
    /// capacity configured.
    pub fn new(channel_capacity: usize, initial_last_seq: i64) -> Self {
        let (tx, _rx) = broadcast::channel(channel_capacity);
        Self::from_sender(tx, initial_last_seq)
    }

    /// Atomically allocate the next `sequence_id` and return it.
    pub fn next_seq(&self) -> i64 {
        self.last_seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Bump the internal counter so subsequent `next_seq()` values are strictly
    /// greater than `seq`. No-op if the counter is already past `seq`.
    ///
    /// Called for `Message` events whose `sequence_id` is allocated by the DB
    /// layer — we have to fold those into the same total order without
    /// double-allocating.
    pub fn observe_seq(&self, seq: i64) {
        self.last_seq.fetch_max(seq, Ordering::AcqRel);
    }

    /// Highest `sequence_id` emitted so far. Used to seed `SseEvent::Init`'s
    /// `last_sequence_id` so the client's `applyIfNewer` guard starts at the
    /// correct floor.
    pub fn current_seq(&self) -> i64 {
        self.last_seq.load(Ordering::Acquire)
    }

    /// Subscribe to the SSE broadcast stream.
    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        self.tx.subscribe()
    }

    /// Send an event that has already been stamped with a `sequence_id`.
    /// Private on purpose — callers must go through [`SseBroadcaster::send_seq`]
    /// or [`SseBroadcaster::send_message`] so the stamping is done at the
    /// broadcaster and forgetting a `sequence_id` is a compile error.
    ///
    /// Returns `Ok(receiver_count)` on success, `Err(())` when the channel has
    /// no active receivers. The error payload is discarded on purpose —
    /// `broadcast::error::SendError<SseEvent>` is ~320 bytes, which triggers
    /// clippy's `result_large_err` lint, and every call site here only ever
    /// reads `.is_err()`.
    fn send(&self, event: SseEvent) -> Result<usize, ()> {
        self.tx.send(event).map_err(|_| ())
    }

    /// Allocate the next `sequence_id`, pass it to `build`, and broadcast the
    /// resulting event. The closure's signature forces the caller to place the
    /// id on the event — forgetting is a compile error.
    pub fn send_seq(&self, build: impl FnOnce(i64) -> SseEvent) -> Result<usize, ()> {
        let seq = self.next_seq();
        self.send(build(seq))
    }

    /// Broadcast a `Message` event using the DB-allocated `message.sequence_id`,
    /// then advance the broadcaster's counter past it.
    pub fn send_message(&self, message: crate::db::Message) -> Result<usize, ()> {
        self.observe_seq(message.sequence_id);
        self.send(SseEvent::Message { message })
    }
}

/// Typed update for conversation metadata pushed mid-session.
/// Each field is `Option` — only populated fields are serialized to the client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConversationMetadataUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conv_mode_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_behind: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_ahead: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_title: Option<String>,
}

/// A conversation enriched with derived display fields for the API layer.
///
/// Produces the same JSON shape as the old `conversation_to_json()` `Value`:
/// all `Conversation` fields at the top level (via `#[serde(flatten)]`) plus
/// the extra display fields.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnrichedConversation {
    #[serde(flatten)]
    pub inner: crate::db::Conversation,
    pub conv_mode_label: String,
    pub branch_name: Option<String>,
    pub worktree_path: Option<String>,
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_title: Option<String>,
    /// The server-user's `$SHELL` (REQ-TERM-002), used by the frontend to
    /// tailor the OSC 133 enablement snippet (REQ-TERM-017).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// The server-user's `$HOME` (REQ-SEED-*), used by the frontend to spawn
    /// seeded conversations scoped to the user's home directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_dir: Option<String>,
    /// Slug of the seed parent conversation, resolved for the UI breadcrumb
    /// (REQ-SEED-003). `None` if `inner.seed_parent_id` is `None` or the
    /// parent has been deleted — the UI renders unlinked text in the latter
    /// case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed_parent_slug: Option<String>,
}

/// Breadcrumb entry for showing LLM thought-process trail in the UI.
///
/// `Option<T>` fields use `skip_serializing_if = "Option::is_none"`, which
/// means `None` is absent from the wire JSON rather than emitted as `null`.
/// `#[ts(optional)]` tells ts-rs to mirror that by generating `field?: T`
/// (undefined when absent) rather than the default `field: T | null`.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SseBreadcrumb {
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub crumb_type: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sequence_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub preview: Option<String>,
}

/// Events sent to SSE clients.
///
/// Every variant carries a `sequence_id` drawn from the conversation's single
/// monotonic counter (task 02675). The client's `applyIfNewer` guard relies on
/// this total order to dedup reconnect replays. Allocation is the
/// responsibility of [`SseBroadcaster`] — do not hand-craft `sequence_id`
/// values at call sites.
#[derive(Debug, Clone)]
pub enum SseEvent {
    Init {
        /// Snapshot's own place in the total order. On init this equals
        /// `last_sequence_id` — the snapshot is itself an event.
        sequence_id: i64,
        conversation: Box<EnrichedConversation>,
        messages: Vec<crate::db::Message>,
        agent_working: bool,
        /// Semantic state category for UI display (idle/working/error/terminal)
        display_state: String,
        /// Highest `sequence_id` ever emitted for this conversation — what the
        /// client seeds `atom.lastSequenceId` with so subsequent
        /// `applyIfNewer` checks start at the right floor.
        last_sequence_id: i64,
        /// Current context window usage in tokens
        context_window_size: u64,
        breadcrumbs: Vec<SseBreadcrumb>,
        /// How many commits the base branch is ahead of this conversation's task branch.
        /// Only populated for Work-mode conversations. 0 means up-to-date or not applicable.
        commits_behind: u32,
        /// How many commits the task branch is ahead of the base branch.
        commits_ahead: u32,
        /// Human-readable project name derived from the repo root directory name.
        project_name: Option<String>,
    },
    /// A newly-persisted message joins the conversation. Uses `message.sequence_id`
    /// as its envelope `sequence_id` — no separate field needed because
    /// `message.sequence_id` is already the DB-allocated id and, thanks to
    /// [`SseBroadcaster::send_message`], folds into the broadcaster's counter.
    Message {
        message: crate::db::Message,
    },
    /// An existing message's mutable fields changed. Carries only the delta —
    /// `message_id` is the target; `sequence_id` is the envelope id used by
    /// the client reducer for dedup (task 02675). The message's persistent
    /// `sequence_id` is immutable and not repeated here.
    MessageUpdated {
        sequence_id: i64,
        message_id: String,
        display_data: Option<serde_json::Value>,
        content: Option<crate::db::MessageContent>,
        /// Typed tool-execution duration in milliseconds, emitted alongside
        /// the tool-result `Message` event so the client can display elapsed
        /// time without an opaque `display_data` parse. `None` when emitting
        /// from non-tool-result paths (e.g. sub-agent summary).
        duration_ms: Option<u64>,
    },
    StateChange {
        sequence_id: i64,
        /// Full typed conversation state
        state: ConvState,
        /// Semantic state category for UI display (idle/working/error/terminal)
        display_state: String,
    },
    /// Ephemeral streaming token. Not persisted, but still carries a
    /// `sequence_id` from the same counter so reconnects don't strand tokens
    /// behind a per-connection closure counter (task 02675 fixes the
    /// `lastSequence` leapfrog stall).
    Token {
        sequence_id: i64,
        text: String,
        request_id: String,
    },
    AgentDone {
        sequence_id: i64,
    },
    /// Emitted once when a conversation's `is_terminal()` first becomes true.
    /// Consumed by the terminal subsystem to tear down any active PTY session.
    ConversationBecameTerminal {
        sequence_id: i64,
    },
    /// Pushed when conversation metadata changes mid-session (e.g., cwd/mode after approval).
    /// Typed struct instead of `Value` — the executor knows exactly which fields changed.
    ConversationUpdate {
        sequence_id: i64,
        update: ConversationMetadataUpdate,
    },
    /// User-facing error for the SSE `error` channel. Carries a typed
    /// payload (task 24682) so internal `Debug`-format strings cannot
    /// accidentally leak — every construction goes through
    /// `runtime::user_facing_error`.
    Error {
        sequence_id: i64,
        error: user_facing_error::UserFacingError,
    },
}

impl RuntimeManager {
    pub fn new(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        platform: PlatformCapability,
        mcp_manager: Arc<crate::tools::mcp::McpClientManager>,
        credential_helper: Option<Arc<crate::llm::CredentialHelper>>,
    ) -> Self {
        let (spawn_tx, spawn_rx) = mpsc::channel(32);
        let (cancel_tx, cancel_rx) = mpsc::channel(32);
        Self {
            db,
            llm_registry,
            platform,
            browser_sessions: Arc::new(BrowserSessionManager::default()),
            bash_handles: Arc::new(BashHandleRegistry::new()),
            mcp_manager,
            terminals: crate::terminal::ActiveTerminals::new(),
            runtimes: RwLock::new(HashMap::new()),
            spawn_tx,
            spawn_rx: RwLock::new(Some(spawn_rx)),
            cancel_tx,
            cancel_rx: RwLock::new(Some(cancel_rx)),
            credential_helper,
        }
    }

    /// Get the detected platform capability
    pub fn platform(&self) -> PlatformCapability {
        self.platform
    }

    /// Get the browser session manager
    pub fn browser_sessions(&self) -> &Arc<BrowserSessionManager> {
        &self.browser_sessions
    }

    /// Get the bash handle registry (REQ-BASH-007 shutdown kill-tree,
    /// REQ-BASH-006 hard-delete cascade).
    pub fn bash_handles(&self) -> &Arc<BashHandleRegistry> {
        &self.bash_handles
    }

    /// Get the spawn channel sender (cloned for each runtime)
    #[allow(dead_code)] // Used internally by get_or_create
    fn spawn_tx(&self) -> mpsc::Sender<SubAgentSpawnRequest> {
        self.spawn_tx.clone()
    }

    /// Get the cancel channel sender (cloned for each runtime)
    #[allow(dead_code)] // Used internally by get_or_create
    fn cancel_tx(&self) -> mpsc::Sender<SubAgentCancelRequest> {
        self.cancel_tx.clone()
    }

    /// Start the background task that handles sub-agent spawn/cancel requests
    /// Must be called once after creating the `RuntimeManager`
    pub async fn start_sub_agent_handler(self: &Arc<Self>) {
        let manager = Arc::clone(self);

        // Take the receivers (can only be done once)
        let spawn_rx = self.spawn_rx.write().await.take();
        let cancel_rx = self.cancel_rx.write().await.take();

        if let (Some(mut spawn_rx), Some(mut cancel_rx)) = (spawn_rx, cancel_rx) {
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(req) = spawn_rx.recv() => {
                            manager.handle_spawn_request(req).await;
                        }
                        Some(req) = cancel_rx.recv() => {
                            manager.handle_cancel_request(req).await;
                        }
                        else => break,
                    }
                }
                tracing::info!("Sub-agent handler stopped");
            });
        }
    }

    /// Handle a sub-agent spawn request
    #[allow(clippy::too_many_lines)]
    async fn handle_spawn_request(self: &Arc<Self>, req: SubAgentSpawnRequest) {
        let SubAgentSpawnRequest {
            spec,
            parent_conversation_id,
            parent_event_tx,
        } = req;

        tracing::info!(
            agent_id = %spec.agent_id,
            parent_id = %parent_conversation_id,
            task = %spec.task,
            "Spawning sub-agent"
        );

        // 1. Look up parent conversation to inherit its conv_mode
        let parent_conv = match self.db.get_conversation(&parent_conversation_id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "Failed to look up parent conversation");
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id: spec.agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: format!("Failed to look up parent conversation: {e}"),
                            error_kind: crate::db::ErrorKind::SubAgentError,
                        },
                    })
                    .await;
                return;
            }
        };

        // Derive sub-agent conv_mode from spec.mode + parent's mode.
        // Explore sub-agents are always Explore. Work sub-agents inherit
        // the parent's Work mode (branch, base_branch, worktree_path).
        let sub_conv_mode = match spec.mode {
            SubAgentMode::Explore => ConvMode::Explore,
            SubAgentMode::Work => parent_conv.conv_mode.clone(),
        };

        // 2. Create conversation in DB with correct conv_mode
        let slug = format!("sub-{}", spec.agent_id.get(..8).unwrap_or(&spec.agent_id));
        let conv = match self
            .db
            .create_conversation_with_project(
                &spec.agent_id,
                &slug,
                &spec.cwd,
                false, // user_initiated = false
                Some(&parent_conversation_id),
                Some(&spec.model_id), // inherit parent's model
                None,                 // project_id
                &sub_conv_mode,
                None, // desired_base_branch
                None, // seed_parent_id (sub-agents use `parent_conversation_id` above)
                None, // seed_label
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create sub-agent conversation");
                // Notify parent of failure
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id: spec.agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: format!("Failed to create conversation: {e}"),
                            error_kind: crate::db::ErrorKind::SubAgentError,
                        },
                    })
                    .await;
                return;
            }
        };

        // 2. Insert initial task as synthetic user message
        let message_id = uuid::Uuid::new_v4().to_string();
        let content = crate::db::MessageContent::user(&spec.task);
        if let Err(e) = self
            .db
            .add_message(&message_id, &conv.id, &content, None, None)
            .await
        {
            tracing::error!(error = %e, "Failed to add initial message");
            let _ = parent_event_tx
                .send(Event::SubAgentResult {
                    agent_id: spec.agent_id,
                    outcome: SubAgentOutcome::Failure {
                        error: format!("Failed to add initial message: {e}"),
                        error_kind: crate::db::ErrorKind::SubAgentError,
                    },
                })
                .await;
            return;
        }

        // 3. Create sub-agent context with max_turns from spec (REQ-PROJ-008)
        let root_conversation_id =
            find_root_conversation_id(&self.db, &parent_conversation_id).await;
        let context_window = self.llm_registry.context_window(&spec.model_id);
        let mut conv_context = ConvContext::sub_agent(
            &conv.id,
            PathBuf::from(&conv.cwd),
            &spec.model_id,
            context_window,
            root_conversation_id,
        );
        conv_context.max_turns = spec.max_turns;
        conv_context.mode_context = Some(conv_mode_to_context(&sub_conv_mode));
        conv_context.mode = match &sub_conv_mode {
            ConvMode::Direct => ModeKind::Direct,
            ConvMode::Explore | ConvMode::Work { .. } => ModeKind::Managed,
            ConvMode::Branch { .. } => ModeKind::Branch,
        };

        // 4. Create channels for the sub-agent runtime. The broadcaster
        // seeds its counter from the message we just inserted (sequence_id=1)
        // so the first non-message event is ordered strictly after it.
        let (event_tx, event_rx) = mpsc::channel(32);
        let broadcaster = SseBroadcaster::new(SSE_BROADCAST_CAPACITY, 1);

        // 5. Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(self.llm_registry.clone(), spec.model_id.clone());
        // Select tool registry based on sub-agent mode (REQ-PROJ-008).
        // Sub-agents get MCP access via the parent's MCP manager.
        let registry = match spec.mode {
            SubAgentMode::Explore => ToolRegistry::for_subagent_explore(),
            SubAgentMode::Work => ToolRegistry::for_subagent_work(),
        };
        let tool_executor = ToolRegistryExecutor::with_mcp(registry, self.mcp_manager.clone());

        // 6. Create runtime with parent notification
        let runtime: ProductionRuntime = ConversationRuntime::new(
            conv_context,
            ConvState::Idle,
            storage,
            llm_client,
            tool_executor,
            self.browser_sessions.clone(),
            self.bash_handles.clone(),
            self.llm_registry.clone(),
            self.terminals.clone(),
            event_rx,
            event_tx.clone(),
            broadcaster.clone(),
        )
        .with_parent(parent_event_tx.clone())
        .with_spawn_channels(self.spawn_tx.clone(), self.cancel_tx.clone())
        .with_credential_helper(self.credential_helper.clone());

        // 7. Store handle
        self.runtimes.write().await.insert(
            conv.id.clone(),
            ConversationHandle {
                event_tx: event_tx.clone(),
                broadcast_tx: broadcaster.clone(),
            },
        );

        // 8. Set up per-agent timeout — sends UserCancel if sub-agent exceeds its limit.
        // This is a safety net; the parent's AwaitingSubAgents deadline is the primary
        // enforcement (REQ-SA-006). Both fire independently.
        let timeout_duration = spec.timeout;
        let timeout_task = {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout_duration).await;
                tracing::info!("Sub-agent timeout reached, sending cancel");
                let _ = event_tx
                    .send(Event::UserCancel {
                        reason: Some("Sub-agent timed out".to_string()),
                    })
                    .await;
            })
        };

        // 9. Start runtime task
        let conv_id = conv.id.clone();
        let task_text = spec.task.clone();
        let manager_for_cleanup = Arc::clone(self);
        tokio::spawn(async move {
            // Send initial UserMessage event to start the conversation
            // Sub-agents generate their own message_id since they don't have a client
            let _ = event_tx
                .send(Event::UserMessage {
                    text: task_text,
                    llm_text: None, // Sub-agent tasks are already fully specified
                    images: vec![],
                    message_id: uuid::Uuid::new_v4().to_string(),
                    user_agent: Some("Phoenix Sub-Agent".to_string()),
                    skill_invocation: None,
                })
                .await;

            runtime.run().await;

            // Cancel timeout — sub-agent finished before its limit
            timeout_task.abort();

            // Remove the handle from runtimes so its event_tx sender is dropped.
            // Without this the channel never closes and any other executor holding
            // only its own internal sender would loop forever waiting for recv().
            manager_for_cleanup.runtimes.write().await.remove(&conv_id);

            tracing::info!(conv_id = %conv_id, "Sub-agent runtime finished and cleaned up");
        });
    }

    /// Handle a sub-agent cancel request
    async fn handle_cancel_request(&self, req: SubAgentCancelRequest) {
        let SubAgentCancelRequest {
            ids,
            parent_conversation_id: _,
            parent_event_tx,
        } = req;

        let runtimes = self.runtimes.read().await;

        for agent_id in ids {
            if let Some(handle) = runtimes.get(&agent_id) {
                tracing::info!(agent_id = %agent_id, "Sending cancel to sub-agent");
                let _ = handle
                    .event_tx
                    .send(Event::UserCancel { reason: None })
                    .await;
            } else {
                // Runtime not found - synthesize failure result
                tracing::warn!(agent_id = %agent_id, "Sub-agent runtime not found, synthesizing failure");
                let _ = parent_event_tx
                    .send(Event::SubAgentResult {
                        agent_id,
                        outcome: SubAgentOutcome::Failure {
                            error: "Sub-agent runtime not found".to_string(),
                            error_kind: crate::db::ErrorKind::Cancelled,
                        },
                    })
                    .await;
            }
        }
    }

    /// Get or create a runtime for a conversation
    #[allow(clippy::too_many_lines)]
    pub async fn get_or_create(
        self: &Arc<Self>,
        conversation_id: &str,
    ) -> Result<ConversationHandle, String> {
        // Check if already running
        {
            let runtimes = self.runtimes.read().await;
            if let Some(handle) = runtimes.get(conversation_id) {
                return Ok(ConversationHandle {
                    event_tx: handle.event_tx.clone(),
                    broadcast_tx: handle.broadcast_tx.clone(),
                });
            }
        }

        // Need to start a new runtime
        let conv = self
            .db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        // Check if this is a sub-agent being resumed (shouldn't happen normally)
        let is_sub_agent = conv.parent_conversation_id.is_some();

        // Resolve model once: use conversation's stored model, or fall back to registry default
        let model_id = conv
            .model
            .clone()
            .unwrap_or_else(|| self.llm_registry.default_model_id().to_string());
        let context_window = self.llm_registry.context_window(&model_id);
        let mode_context = conv_mode_to_context(&conv.conv_mode);
        let mut context = if is_sub_agent {
            let root_id = find_root_conversation_id(&self.db, conversation_id).await;
            ConvContext::sub_agent(
                &conv.id,
                PathBuf::from(&conv.cwd),
                &model_id,
                context_window,
                root_id,
            )
        } else {
            ConvContext::new(
                &conv.id,
                PathBuf::from(&conv.cwd),
                &model_id,
                context_window,
            )
        };
        context.mode_context = Some(mode_context);
        context.desired_base_branch = conv.desired_base_branch.clone();
        context.mode = match &conv.conv_mode {
            ConvMode::Direct => ModeKind::Direct,
            ConvMode::Explore | ConvMode::Work { .. } => ModeKind::Managed,
            ConvMode::Branch { .. } => ModeKind::Branch,
        };

        let (event_tx, event_rx) = mpsc::channel(32);
        // Seed the broadcaster's sequence_id counter from the highest seq
        // already persisted for this conversation. Without this, a resumed
        // conversation would allocate sequence_ids starting from 1 and collide
        // with ids the client may have already observed in a previous session.
        let initial_last_seq = self
            .db
            .get_last_sequence_id(conversation_id)
            .await
            .unwrap_or(0);
        let broadcaster = SseBroadcaster::new(SSE_BROADCAST_CAPACITY, initial_last_seq);

        // Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(self.llm_registry.clone(), model_id);

        // Use appropriate tool registry based on sub-agent status and conversation mode.
        // Sub-agents get a restricted tool set (no MCP, no spawn_agents) -- they only
        // have SubmitResult/SubmitError for completion signaling.
        let tool_executor = if is_sub_agent {
            // Resumed sub-agents default to Explore registry (mode not persisted).
            // This is a rare path -- sub-agents don't normally survive restarts.
            ToolRegistryExecutor::with_mcp(
                ToolRegistry::for_subagent_explore(),
                self.mcp_manager.clone(),
            )
        } else {
            use crate::db::ConvMode;
            let registry = match conv.conv_mode {
                ConvMode::Explore => {
                    if self.platform.has_sandbox() {
                        ToolRegistry::explore_with_sandbox()
                    } else {
                        ToolRegistry::explore_no_sandbox()
                    }
                }
                ConvMode::Direct => {
                    // Full tool suite for Direct mode
                    ToolRegistry::direct()
                }
                ConvMode::Work { .. } | ConvMode::Branch { .. } => {
                    // Full tool suite for Work/Branch mode (same as Direct)
                    ToolRegistry::direct()
                }
            };
            // MCP tools resolved live from the manager on every definitions()
            // call -- enable/disable and reload take effect immediately.
            ToolRegistryExecutor::with_mcp(registry, self.mcp_manager.clone())
        };

        // Determine initial state: check if conversation needs auto-continuation
        // REQ-BED-007 says resume from idle, but we need to handle interrupted turns
        let (initial_state, needs_auto_continue) =
            self.determine_resume_state(conversation_id).await?;

        let runtime: ProductionRuntime = ConversationRuntime::new(
            context,
            initial_state,
            storage,
            llm_client,
            tool_executor,
            self.browser_sessions.clone(),
            self.bash_handles.clone(),
            self.llm_registry.clone(),
            self.terminals.clone(),
            event_rx,
            event_tx.clone(),
            broadcaster.clone(),
        )
        .with_spawn_channels(self.spawn_tx.clone(), self.cancel_tx.clone())
        .with_credential_helper(self.credential_helper.clone());

        // If auto-continuing, inject a system message so the LLM knows a restart
        // happened. This also serves as the restart loop counter — recovery.rs
        // counts consecutive restart system messages at the tail of the history.
        if needs_auto_continue {
            use crate::db::SystemContent;
            use crate::runtime::recovery::RESTART_SYSTEM_MESSAGE_MARKER;

            let restart_msg = format!(
                "{RESTART_SYSTEM_MESSAGE_MARKER} This conversation was interrupted \
                 by a server restart. The last tool execution may have caused the \
                 restart. Review the tool results above before deciding what to do \
                 next. Do NOT re-execute the same command that was just running."
            );
            let msg_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = self
                .db
                .add_message(
                    &msg_id,
                    conversation_id,
                    &crate::db::MessageContent::System(SystemContent { text: restart_msg }),
                    None,
                    None,
                )
                .await
            {
                tracing::warn!(conv_id = %conversation_id, error = %e,
                    "Failed to inject restart system message");
            }
            tracing::info!(conv_id = %conversation_id, "Will auto-continue interrupted conversation");
        }

        // Start runtime in background
        let conv_id = conversation_id.to_string();
        let manager_for_cleanup = Arc::clone(self);
        tokio::spawn(async move {
            runtime.run().await;

            // Remove the handle so its event_tx sender is dropped.
            // Without this, the channel stays open and the handle persists after
            // the executor exits (FM-5). A new runtime will be created by
            // get_or_create if the conversation is resumed.
            manager_for_cleanup.runtimes.write().await.remove(&conv_id);

            tracing::info!(conv_id = %conv_id, "Conversation runtime finished and cleaned up");
        });

        let handle = ConversationHandle {
            event_tx: event_tx.clone(),
            broadcast_tx: broadcaster.clone(),
        };

        // Store handle
        self.runtimes.write().await.insert(
            conversation_id.to_string(),
            ConversationHandle {
                event_tx,
                broadcast_tx: broadcaster,
            },
        );

        Ok(handle)
    }

    /// Send an event to a conversation
    /// Evict an active runtime so it gets recreated with fresh config on next access.
    /// Used after model upgrades to pick up the new model and context window.
    pub async fn evict_runtime(&self, conversation_id: &str) {
        self.runtimes.write().await.remove(conversation_id);
    }

    pub async fn send_event(
        self: &Arc<Self>,
        conversation_id: &str,
        event: Event,
    ) -> Result<(), String> {
        let handle = self.get_or_create(conversation_id).await?;
        handle
            .event_tx
            .send(event)
            .await
            .map_err(|e| format!("Failed to send event: {e}"))
    }

    /// Subscribe to conversation updates
    pub async fn subscribe(
        self: &Arc<Self>,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<SseEvent>, String> {
        let handle = self.get_or_create(conversation_id).await?;
        Ok(handle.broadcast_tx.subscribe())
    }

    /// Determine the resume state for a conversation.
    ///
    /// Delegates to `recovery::should_auto_continue` for the actual logic.
    /// See that module for comprehensive tests.
    async fn determine_resume_state(
        &self,
        conversation_id: &str,
    ) -> Result<(ConvState, bool), String> {
        // States that survive restart (preserved by reset_all_to_idle) must be
        // restored from the DB, not derived from message history. The recovery
        // heuristic only applies to transient states that were reset to Idle.
        let conv = self
            .db
            .get_conversation(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        match &conv.state {
            ConvState::AwaitingTaskApproval { .. }
            | ConvState::AwaitingUserResponse { .. }
            | ConvState::ContextExhausted { .. }
            | ConvState::Terminal => {
                tracing::debug!(
                    conv_id = %conversation_id,
                    state = ?std::mem::discriminant(&conv.state),
                    "Restoring persisted state (survives restart)"
                );
                return Ok((conv.state, false));
            }
            _ => {}
        }

        let messages = self
            .db
            .get_messages(conversation_id)
            .await
            .map_err(|e| e.to_string())?;

        let decision = recovery::should_auto_continue(&messages);

        tracing::debug!(
            conv_id = %conversation_id,
            msg_count = messages.len(),
            reason = ?decision.reason,
            needs_auto_continue = decision.needs_auto_continue,
            "determine_resume_state"
        );

        if decision.needs_auto_continue {
            tracing::info!(
                conv_id = %conversation_id,
                "Detected interrupted conversation - will auto-continue"
            );
        }

        Ok((decision.state, decision.needs_auto_continue))
    }

    /// Get the database handle
    pub fn db(&self) -> &Database {
        &self.db
    }

    pub fn model_registry(&self) -> &ModelRegistry {
        &self.llm_registry
    }

    /// Get the LLM registry
    #[allow(dead_code)] // For future API use
    pub fn llm_registry(&self) -> &Arc<ModelRegistry> {
        &self.llm_registry
    }
}

/// Walk up the parent chain to find the root (top-level) conversation id.
///
/// For a root conversation the function returns immediately. For deeply nested
/// sub-agents it follows `parent_conversation_id` links until it reaches a
/// conversation with no parent, or until the 10-iteration guard fires on
/// corrupt data.
async fn find_root_conversation_id(db: &Database, conversation_id: &str) -> String {
    let mut current_id = conversation_id.to_string();
    for _ in 0..10 {
        match db.get_conversation(&current_id).await {
            Ok(conv) => match conv.parent_conversation_id {
                None => return current_id,
                Some(parent_id) => current_id = parent_id,
            },
            Err(_) => return current_id,
        }
    }
    current_id
}

/// Convert a database `ConvMode` into a `ModeContext` for the system prompt.
fn conv_mode_to_context(mode: &ConvMode) -> ModeContext {
    match mode {
        ConvMode::Explore => ModeContext::Explore,
        ConvMode::Work {
            branch_name,
            base_branch,
            worktree_path,
            ..
        } => ModeContext::Work {
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            worktree_path: worktree_path.to_string(),
        },
        ConvMode::Branch {
            branch_name,
            base_branch,
            worktree_path,
        } => ModeContext::Branch {
            branch_name: branch_name.to_string(),
            base_branch: base_branch.to_string(),
            worktree_path: worktree_path.to_string(),
        },
        ConvMode::Direct => ModeContext::Direct,
    }
}

#[cfg(test)]
mod broadcaster_tests {
    use super::*;

    /// Regression for task 02679: when a caller pre-allocates a message's
    /// `sequence_id` from the broadcaster *before* writing to the DB, the
    /// message's seq is strictly greater than any ephemeral event emitted
    /// earlier on the same broadcaster. A concurrent reader (the client's
    /// `applyIfNewer` guard) will accept the message rather than dropping
    /// it as stale.
    ///
    /// The failure shape this guards against:
    /// - pre-fix, `add_message` allocated its own seq via `SELECT MAX+1`.
    /// - After several ephemeral events (tokens advance `SseBroadcaster`'s
    ///   counter to N ≫ DB message count), an assistant message persists
    ///   with DB seq = (count+1) ≪ N.
    /// - `send_message` broadcasts it with that stale seq; the client's
    ///   `lastSequenceId ≥ N` causes `applyIfNewer` to drop it; the
    ///   assistant's response visibly disappears.
    ///
    /// See `specs/sse_wire/sse_wire.allium`, invariant `PersistBeforeBroadcast`.
    #[test]
    fn next_seq_after_ephemeral_events_exceeds_prior_events() {
        let b = SseBroadcaster::new(16, 0);

        // Simulate many ephemeral events (token stream, state changes)
        // each consuming one seq from the counter.
        let mut last_ephemeral = 0;
        for _ in 0..50 {
            last_ephemeral = b.next_seq();
        }

        // Pre-allocate a seq for a message that's about to be persisted.
        let message_seq = b.next_seq();

        // The message seq must be strictly greater than every ephemeral
        // event emitted before it. This is the structural property the
        // client's applyIfNewer guard relies on.
        assert!(
            message_seq > last_ephemeral,
            "pre-allocated message seq ({message_seq}) must exceed all prior \
             ephemeral seqs ({last_ephemeral})"
        );
        assert_eq!(message_seq, last_ephemeral + 1);
    }

    /// `observe_seq` is idempotent when the broadcaster's counter is
    /// already past the supplied seq — this is the normal path once
    /// `send_message` runs with a pre-allocated seq (broadcaster counter
    /// already = seq after `next_seq()`).
    #[test]
    fn observe_seq_is_idempotent_when_counter_already_past() {
        let b = SseBroadcaster::new(16, 0);
        let seq = b.next_seq();
        b.observe_seq(seq);
        assert_eq!(
            b.current_seq(),
            seq,
            "observe_seq must not bump the counter past seq when already at seq"
        );

        // A subsequent next_seq advances by exactly one.
        let next = b.next_seq();
        assert_eq!(next, seq + 1);
    }

    /// `observe_seq` still catches up when a DB-allocated message seq
    /// leapfrogs the broadcaster — the pre-fix path. Kept as a belt-and-
    /// braces check: non-broadcasting paths (sub-agent bootstrap, crash
    /// recovery) still use `add_message`, and the first `send_message` on
    /// a restarted conversation must fold their DB seqs back in.
    #[test]
    fn observe_seq_catches_up_when_db_seq_leapfrogs() {
        let b = SseBroadcaster::new(16, 0);
        // Simulate: two direct-DB writes (bootstrap + restart marker)
        // occurred before the broadcaster emitted anything; broadcaster
        // is at 0, DB MAX is 2.
        b.observe_seq(2);
        let next = b.next_seq();
        assert_eq!(next, 3, "broadcaster must allocate past the DB watermark");
    }
}
