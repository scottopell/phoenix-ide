//! Trait abstractions for runtime I/O
//!
//! These traits enable testing the executor with mock implementations.

use crate::db::{Message, MessageType, ToolResult, UsageData};
use crate::llm::{LlmError, LlmRequest, LlmResponse};
use crate::state_machine::ConvState;
use crate::tools::ToolOutput;
use async_trait::async_trait;
use serde_json::Value;

/// Storage for conversation messages
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Add a message to the conversation
    async fn add_message(
        &self,
        conv_id: &str,
        msg_type: MessageType,
        content: &Value,
        display_data: Option<&Value>,
        usage_data: Option<&UsageData>,
    ) -> Result<Message, String>;

    /// Get all messages for a conversation
    async fn get_messages(&self, conv_id: &str) -> Result<Vec<Message>, String>;
}

/// Storage for conversation state
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Update the conversation state
    async fn update_state(
        &self,
        conv_id: &str,
        state: &crate::db::ConversationState,
        state_data: Option<&Value>,
    ) -> Result<(), String>;

    /// Get the current conversation state
    async fn get_state(&self, conv_id: &str) -> Result<ConvState, String>;
}

/// Client for making LLM requests
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Complete an LLM request
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError>;

    /// Get the model ID
    fn model_id(&self) -> &str;
}

/// Executor for tools
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool by name
    async fn execute(&self, name: &str, input: Value) -> Option<ToolOutput>;

    /// Get tool definitions for LLM
    fn definitions(&self) -> Vec<crate::llm::ToolDefinition>;
}

/// Combined storage trait for convenience
pub trait Storage: MessageStore + StateStore {}
impl<T: MessageStore + StateStore> Storage for T {}
