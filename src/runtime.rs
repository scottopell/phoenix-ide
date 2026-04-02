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

#[cfg(test)]
pub mod testing;

pub use executor::ConversationRuntime;
pub use traits::*;

use crate::platform::PlatformCapability;
use crate::state_machine::state::{SubAgentOutcome, SubAgentSpec};
use crate::tools::{BrowserSessionManager, ToolRegistry};

/// Type alias for production runtime with concrete implementations
pub type ProductionRuntime =
    ConversationRuntime<DatabaseStorage, RegistryLlmClient, ToolRegistryExecutor>;

use crate::db::Database;
use crate::llm::ModelRegistry;
use crate::state_machine::{ConvContext, ConvState, Event};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

/// Request to spawn a sub-agent
#[derive(Debug)]
pub struct SubAgentSpawnRequest {
    pub spec: SubAgentSpec,
    pub parent_conversation_id: String,
    pub parent_event_tx: mpsc::Sender<Event>,
    pub model_id: String,
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
    mcp_manager: Arc<crate::tools::mcp::McpClientManager>,
    runtimes: RwLock<HashMap<String, ConversationHandle>>,
    /// Channel for sub-agent spawn requests
    spawn_tx: mpsc::Sender<SubAgentSpawnRequest>,
    spawn_rx: RwLock<Option<mpsc::Receiver<SubAgentSpawnRequest>>>,
    /// Channel for sub-agent cancel requests
    cancel_tx: mpsc::Sender<SubAgentCancelRequest>,
    cancel_rx: RwLock<Option<mpsc::Receiver<SubAgentCancelRequest>>>,
}

/// Handle to interact with a running conversation
pub struct ConversationHandle {
    pub event_tx: mpsc::Sender<Event>,
    pub broadcast_tx: broadcast::Sender<SseEvent>,
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
}

/// Breadcrumb entry for showing LLM thought-process trail in the UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SseBreadcrumb {
    #[serde(rename = "type")]
    pub crumb_type: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Events sent to SSE clients
#[derive(Debug, Clone)]
pub enum SseEvent {
    Init {
        conversation: Box<EnrichedConversation>,
        messages: Vec<crate::db::Message>,
        agent_working: bool,
        /// Semantic state category for UI display (idle/working/error/terminal)
        display_state: String,
        last_sequence_id: i64,
        /// Current context window usage in tokens
        context_window_size: u64,
        /// Model's maximum context window in tokens (for calculating percentage)
        model_context_window: usize,
        breadcrumbs: Vec<SseBreadcrumb>,
        /// How many commits the base branch is ahead of this conversation's task branch.
        /// Only populated for Work-mode conversations. 0 means up-to-date or not applicable.
        commits_behind: u32,
    },
    Message {
        message: crate::db::Message,
    },
    StateChange {
        /// Full typed conversation state
        state: ConvState,
        /// Semantic state category for UI display (idle/working/error/terminal)
        display_state: String,
    },
    /// Ephemeral streaming token — not persisted, no `sequence_id` (REQ-BED-025)
    Token {
        text: String,
        request_id: String,
    },
    AgentDone,
    /// Pushed when conversation metadata changes mid-session (e.g., cwd/mode after approval).
    /// Typed struct instead of `Value` — the executor knows exactly which fields changed.
    ConversationUpdate {
        update: ConversationMetadataUpdate,
    },
    Error {
        message: String,
    },
}

impl RuntimeManager {
    pub fn new(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        platform: PlatformCapability,
        mcp_manager: Arc<crate::tools::mcp::McpClientManager>,
    ) -> Self {
        let (spawn_tx, spawn_rx) = mpsc::channel(32);
        let (cancel_tx, cancel_rx) = mpsc::channel(32);
        Self {
            db,
            llm_registry,
            platform,
            browser_sessions: Arc::new(BrowserSessionManager::default()),
            mcp_manager,
            runtimes: RwLock::new(HashMap::new()),
            spawn_tx,
            spawn_rx: RwLock::new(Some(spawn_rx)),
            cancel_tx,
            cancel_rx: RwLock::new(Some(cancel_rx)),
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
            model_id,
        } = req;

        tracing::info!(
            agent_id = %spec.agent_id,
            parent_id = %parent_conversation_id,
            task = %spec.task,
            "Spawning sub-agent"
        );

        // 1. Create conversation in DB
        let slug = format!("sub-{}", spec.agent_id.get(..8).unwrap_or(&spec.agent_id));
        let conv = match self
            .db
            .create_conversation(
                &spec.agent_id,
                &slug,
                &spec.cwd,
                false, // user_initiated = false
                Some(&parent_conversation_id),
                Some(&model_id), // inherit parent's model
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

        // 3. Create sub-agent context
        let context_window = self.llm_registry.context_window(&model_id);
        let conv_context = ConvContext::sub_agent(
            &conv.id,
            PathBuf::from(&conv.cwd),
            &model_id,
            context_window,
        );

        // 4. Create channels for the sub-agent runtime
        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, _) = broadcast::channel(128);

        // 5. Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(self.llm_registry.clone(), model_id);
        // Sub-agents use the standard sub-agent tool set for now.
        // Mode inheritance (REQ-BED-018) will be refined in M2.
        let tool_executor = ToolRegistryExecutor::builtin_only(ToolRegistry::for_subagent());

        // 6. Create runtime with parent notification
        let runtime: ProductionRuntime = ConversationRuntime::new(
            conv_context,
            ConvState::Idle,
            storage,
            llm_client,
            tool_executor,
            self.browser_sessions.clone(),
            self.llm_registry.clone(),
            event_rx,
            event_tx.clone(),
            broadcast_tx.clone(),
        )
        .with_parent(parent_event_tx.clone())
        .with_spawn_channels(self.spawn_tx.clone(), self.cancel_tx.clone());

        // 7. Store handle
        self.runtimes.write().await.insert(
            conv.id.clone(),
            ConversationHandle {
                event_tx: event_tx.clone(),
                broadcast_tx: broadcast_tx.clone(),
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
                let _ = event_tx.send(Event::UserCancel).await;
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
                let _ = handle.event_tx.send(Event::UserCancel).await;
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
        let context = if is_sub_agent {
            // Sub-agent being resumed - we don't have the original task
            // This is an edge case that shouldn't happen in normal operation
            ConvContext::sub_agent(
                &conv.id,
                PathBuf::from(&conv.cwd),
                &model_id,
                context_window,
            )
        } else {
            ConvContext::new(
                &conv.id,
                PathBuf::from(&conv.cwd),
                &model_id,
                context_window,
            )
        };

        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, _) = broadcast::channel(128);

        // Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(self.llm_registry.clone(), model_id);

        // Use appropriate tool registry based on sub-agent status and conversation mode.
        // Sub-agents get a restricted tool set (no MCP, no spawn_agents) -- they only
        // have SubmitResult/SubmitError for completion signaling.
        let tool_executor = if is_sub_agent {
            ToolRegistryExecutor::builtin_only(ToolRegistry::for_subagent())
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
                ConvMode::Standalone => {
                    // Full tool suite for non-git directories
                    ToolRegistry::standalone()
                }
                ConvMode::Work { .. } => {
                    // Full tool suite for Work mode (same as standalone for now)
                    ToolRegistry::standalone()
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
            self.llm_registry.clone(),
            event_rx,
            event_tx.clone(),
            broadcast_tx.clone(),
        )
        .with_spawn_channels(self.spawn_tx.clone(), self.cancel_tx.clone());

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
            broadcast_tx: broadcast_tx.clone(),
        };

        // Store handle
        self.runtimes.write().await.insert(
            conversation_id.to_string(),
            ConversationHandle {
                event_tx,
                broadcast_tx,
            },
        );

        Ok(handle)
    }

    /// Send an event to a conversation
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
