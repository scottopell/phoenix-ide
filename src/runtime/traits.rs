//! Trait abstractions for runtime I/O
//!
//! These traits enable testing the executor with mock implementations.

use crate::db::{Message, MessageContent, UsageData};
use crate::llm::{LlmError, LlmRequest, LlmResponse};
use crate::state_machine::ConvState;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Storage for conversation messages
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Add a message to the conversation
    async fn add_message(
        &self,
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String>;

    /// Get all messages for a conversation
    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String>;
}

/// Storage for conversation state
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Update the conversation state (full state as JSON)
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
    ) -> Result<(), String>;

    /// Get the current conversation state
    #[allow(dead_code)] // API completeness
    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String>;
}

/// Client for making LLM requests
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Complete an LLM request
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Get the model ID
    #[allow(dead_code)] // API completeness
    fn model_id(&self) -> &str;
}

/// Executor for tools
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool by name with cancellation support
    async fn execute(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Option<ToolOutput>;

    /// Get tool definitions for LLM
    fn definitions(&self) -> Vec<crate::llm::ToolDefinition>;
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
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        (**self).add_message(conv_id, content, display_data, usage_data).await
    }

    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String> {
        (**self).get_messages(conv_id).await
    }
}

#[async_trait]
impl<T: StateStore + ?Sized> StateStore for Arc<T> {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
    ) -> Result<(), String> {
        (**self).update_state(conv_id, state).await
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        (**self).get_state(conv_id).await
    }
}

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClient for Arc<T> {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        (**self).complete(request).await
    }

    fn model_id(&self) -> &str {
        (**self).model_id()
    }
}

#[async_trait]
impl<T: ToolExecutor + ?Sized> ToolExecutor for Arc<T> {
    async fn execute(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Option<ToolOutput> {
        (**self).execute(name, input, cancel).await
    }

    fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        (**self).definitions()
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
        conv_id: &str,
        content: &MessageContent,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String> {
        let id = uuid::Uuid::new_v4().to_string();
        self.db
            .add_message(&id, conv_id, content, display_data, usage_data)
            .map_err(|e| e.to_string())
    }

    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String> {
        self.db.get_messages(conv_id).map_err(|e| e.to_string())
    }
}

#[async_trait]
impl StateStore for DatabaseStorage {
    async fn update_state(
        &self,
        conv_id: &str,
        state: &ConvState,
    ) -> Result<(), String> {
        self.db
            .update_conversation_state(conv_id, state)
            .map_err(|e| e.to_string())
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        let conv = self.db.get_conversation(conv_id).map_err(|e| e.to_string())?;
        Ok(conv.state)
    }
}

/// Adapter to use ModelRegistry as LlmClient
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
        let llm = self
            .registry
            .get(&self.model_id)
            .or_else(|| self.registry.default())
            .ok_or_else(|| LlmError::network("No LLM available"))?;
        llm.complete(request).await
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// Adapter to use ToolRegistry as ToolExecutor
pub struct ToolRegistryExecutor {
    registry: ToolRegistry,
}

impl ToolRegistryExecutor {
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolExecutor for ToolRegistryExecutor {
    async fn execute(
        &self,
        name: &str,
        input: Value,
        cancel: CancellationToken,
    ) -> Option<ToolOutput> {
        self.registry.execute(name, input, cancel).await
    }

    fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        self.registry.definitions()
    }
}


