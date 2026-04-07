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

    /// Update the conversation working directory (e.g., after worktree creation)
    async fn update_conversation_cwd(&self, conv_id: &str, cwd: &str) -> Result<(), String>;
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
    async fn definitions(&self) -> Vec<crate::llm::ToolDefinition>;

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

    async fn update_conversation_cwd(&self, conv_id: &str, cwd: &str) -> Result<(), String> {
        (**self).update_conversation_cwd(conv_id, cwd).await
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

    async fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        (**self).definitions().await
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

    async fn update_conversation_cwd(&self, conv_id: &str, cwd: &str) -> Result<(), String> {
        self.db
            .update_conversation_cwd(conv_id, cwd)
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
}

impl ToolRegistryExecutor {
    /// Create an executor with built-in tools only (no MCP).
    /// Used for sub-agents which have a restricted tool set.
    pub fn builtin_only(registry: ToolRegistry) -> Self {
        Self {
            registry: std::sync::RwLock::new(registry),
            mcp_manager: None,
        }
    }

    /// Create an executor with built-in tools + live MCP tool resolution.
    /// MCP tools are resolved from the manager on every `definitions()` and
    /// `execute()` call, so enable/disable and reload take effect immediately.
    pub fn with_mcp(
        registry: ToolRegistry,
        manager: Arc<crate::tools::mcp::McpClientManager>,
    ) -> Self {
        Self {
            registry: std::sync::RwLock::new(registry),
            mcp_manager: Some(manager),
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
    async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput> {
        // Look up the tool while holding the read lock, then drop the guard
        // before the async .run() call (RwLockReadGuard is !Send).
        let tool = {
            let registry = self
                .registry
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.find_tool(name)
        };
        if let Some(t) = tool {
            return Some(t.run(input, ctx).await);
        }

        // Fall back to live MCP tool resolution.
        if let Some(ref manager) = self.mcp_manager {
            if let Some(mcp_tool) = crate::tools::mcp::create_mcp_tool_by_name(manager, name).await
            {
                return Some(mcp_tool.run(input, ctx).await);
            }
        }

        None
    }

    async fn definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        let mut defs = {
            let registry = self
                .registry
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.definitions()
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
        self.swap_registry(ToolRegistry::direct());
        tracing::info!("Tool registry upgraded to Work mode (full tool suite)");
    }
}
