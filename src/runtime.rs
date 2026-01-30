//! Runtime for executing conversations
//!
//! REQ-BED-007: State Persistence
//! REQ-BED-010: Fixed Working Directory
//! REQ-BED-011: Real-time Event Streaming
//! REQ-BED-012: Context Window Tracking

mod executor;
pub mod traits;

#[cfg(test)]
pub mod testing;

pub use executor::ConversationRuntime;
pub use traits::*;

use crate::tools::ToolRegistry;

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

/// Manager for all conversation runtimes
pub struct RuntimeManager {
    db: Database,
    llm_registry: Arc<ModelRegistry>,
    runtimes: RwLock<HashMap<String, ConversationHandle>>,
}

/// Handle to interact with a running conversation
pub struct ConversationHandle {
    pub event_tx: mpsc::Sender<Event>,
    pub broadcast_tx: broadcast::Sender<SseEvent>,
}

/// Events sent to SSE clients
#[derive(Debug, Clone)]
pub enum SseEvent {
    Init {
        conversation: serde_json::Value,
        messages: Vec<serde_json::Value>,
        agent_working: bool,
        last_sequence_id: i64,
    },
    Message {
        message: serde_json::Value,
    },
    StateChange {
        state: String,
        state_data: serde_json::Value,
    },
    AgentDone,
    Error {
        message: String,
    },
}

impl RuntimeManager {
    pub fn new(db: Database, llm_registry: Arc<ModelRegistry>) -> Self {
        Self {
            db,
            llm_registry,
            runtimes: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create a runtime for a conversation
    pub async fn get_or_create(&self, conversation_id: &str) -> Result<ConversationHandle, String> {
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
            .map_err(|e| e.to_string())?;

        let context = ConvContext::new(
            &conv.id,
            PathBuf::from(&conv.cwd),
            self.llm_registry.default_model_id(),
        );

        let (event_tx, event_rx) = mpsc::channel(32);
        let (broadcast_tx, _) = broadcast::channel(128);

        // Create production adapters
        let storage = DatabaseStorage::new(self.db.clone());
        let llm_client = RegistryLlmClient::new(
            self.llm_registry.clone(),
            self.llm_registry.default_model_id().to_string(),
        );
        let tool_executor = ToolRegistryExecutor::new(ToolRegistry::new(
            context.working_dir.clone(),
            self.llm_registry.clone(),
        ));

        let runtime: ProductionRuntime = ConversationRuntime::new(
            context,
            ConvState::Idle, // Always resume from idle (REQ-BED-007)
            storage,
            llm_client,
            tool_executor,
            event_rx,
            event_tx.clone(),
            broadcast_tx.clone(),
        );

        // Start runtime in background
        let conv_id = conversation_id.to_string();
        tokio::spawn(async move {
            runtime.run().await;
            tracing::info!(conv_id = %conv_id, "Conversation runtime finished");
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
    pub async fn send_event(&self, conversation_id: &str, event: Event) -> Result<(), String> {
        let handle = self.get_or_create(conversation_id).await?;
        handle
            .event_tx
            .send(event)
            .await
            .map_err(|e| format!("Failed to send event: {e}"))
    }

    /// Subscribe to conversation updates
    pub async fn subscribe(
        &self,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<SseEvent>, String> {
        let handle = self.get_or_create(conversation_id).await?;
        Ok(handle.broadcast_tx.subscribe())
    }

    /// Get the database handle
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Get the LLM registry
    #[allow(dead_code)] // For future API use
    pub fn llm_registry(&self) -> &Arc<ModelRegistry> {
        &self.llm_registry
    }
}
