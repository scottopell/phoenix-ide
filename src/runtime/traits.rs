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

    /// Get all messages for a conversation
    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String>;

    /// Get a single message by ID
    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String>;

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
}

/// Storage for conversation state
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Update the conversation state (full state as JSON)
    async fn update_state(&self, conv_id: &str, state: &ConvState) -> Result<(), String>;

    /// Get the current conversation state
    #[allow(dead_code)] // API completeness
    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String>;

    /// Update the conversation mode (e.g., Explore -> Work on task approval)
    async fn update_conversation_mode(&self, conv_id: &str, mode: &ConvMode) -> Result<(), String>;
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

use crate::tools::ToolContext;

/// Executor for tools
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool by name with context
    async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput>;

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

    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String> {
        (**self).get_messages(conv_id).await
    }

    async fn get_message_by_id(&self, message_id: &str) -> Result<Message, String> {
        (**self).get_message_by_id(message_id).await
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
}

#[async_trait]
impl<T: StateStore + ?Sized> StateStore for Arc<T> {
    async fn update_state(&self, conv_id: &str, state: &ConvState) -> Result<(), String> {
        (**self).update_state(conv_id, state).await
    }

    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String> {
        (**self).get_state(conv_id).await
    }

    async fn update_conversation_mode(&self, conv_id: &str, mode: &ConvMode) -> Result<(), String> {
        (**self).update_conversation_mode(conv_id, mode).await
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
    async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput> {
        (**self).execute(name, input, ctx).await
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
}

#[async_trait]
impl StateStore for DatabaseStorage {
    async fn update_state(&self, conv_id: &str, state: &ConvState) -> Result<(), String> {
        self.db
            .update_conversation_state(conv_id, state)
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
    async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput> {
        self.registry.execute(name, input, ctx).await
    }

    fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        self.registry.definitions()
    }
}
