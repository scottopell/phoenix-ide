//! Conversation state types

use crate::db::{ErrorKind, SubAgentResult, ToolResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Conversation state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConvState {
    /// Ready for user input, no pending operations
    Idle,
    
    /// User message received, preparing LLM request
    AwaitingLlm,
    
    /// LLM request in flight, with retry tracking
    LlmRequesting { attempt: u32 },
    
    /// Executing tools serially
    ToolExecuting {
        current_tool_id: String,
        remaining_tool_ids: Vec<String>,
        #[serde(default)]
        completed_results: Vec<ToolResult>,
    },
    
    /// User requested cancellation, waiting for graceful completion
    Cancelling { pending_tool_id: Option<String> },
    
    /// Waiting for sub-agents to complete
    AwaitingSubAgents {
        pending_ids: Vec<String>,
        #[serde(default)]
        completed_results: Vec<SubAgentResult>,
    },
    
    /// Error occurred - UI displays this state directly
    Error {
        message: String,
        error_kind: ErrorKind,
    },
}

impl ConvState {
    /// Check if this is a terminal state (conversation should stop processing)
    pub fn is_terminal(&self) -> bool {
        false // Conversations can always be continued from any state
    }

    /// Check if agent is currently working
    pub fn is_working(&self) -> bool {
        !matches!(self, ConvState::Idle | ConvState::Error { .. })
    }

    /// Convert to database state string
    pub fn to_db_state(&self) -> &'static str {
        match self {
            ConvState::Idle => "idle",
            ConvState::AwaitingLlm => "awaiting_llm",
            ConvState::LlmRequesting { .. } => "llm_requesting",
            ConvState::ToolExecuting { .. } => "tool_executing",
            ConvState::Cancelling { .. } => "cancelling",
            ConvState::AwaitingSubAgents { .. } => "awaiting_sub_agents",
            ConvState::Error { .. } => "error",
        }
    }
}

impl Default for ConvState {
    fn default() -> Self {
        ConvState::Idle
    }
}

/// Context for a conversation (immutable configuration)
#[derive(Debug, Clone)]
pub struct ConvContext {
    pub conversation_id: String,
    pub working_dir: PathBuf,
    pub model_id: String,
    pub is_sub_agent: bool,
}

impl ConvContext {
    pub fn new(
        conversation_id: impl Into<String>,
        working_dir: PathBuf,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            working_dir,
            model_id: model_id.into(),
            is_sub_agent: false,
        }
    }

    pub fn as_sub_agent(mut self) -> Self {
        self.is_sub_agent = true;
        self
    }
}
