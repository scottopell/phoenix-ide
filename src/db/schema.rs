//! Database schema and types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// SQL schema for initialization
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    slug TEXT UNIQUE,
    cwd TEXT NOT NULL,
    parent_conversation_id TEXT,
    user_initiated BOOLEAN NOT NULL,
    state TEXT NOT NULL DEFAULT 'idle',
    state_data TEXT,
    state_updated_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT 0,
    
    FOREIGN KEY (parent_conversation_id) 
        REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_conversations_slug ON conversations(slug);
CREATE INDEX IF NOT EXISTS idx_conversations_parent ON conversations(parent_conversation_id);
CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sequence_id INTEGER NOT NULL,
    message_type TEXT NOT NULL,
    content TEXT NOT NULL,
    display_data TEXT,
    usage_data TEXT,
    created_at TEXT NOT NULL,
    
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id, sequence_id);
"#;

/// Conversation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub slug: Option<String>,
    pub cwd: String,
    pub parent_conversation_id: Option<String>,
    pub user_initiated: bool,
    pub state: ConversationState,
    pub state_data: Option<serde_json::Value>,
    pub state_updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived: bool,
}

impl Conversation {
    /// Check if the agent is currently working
    pub fn is_agent_working(&self) -> bool {
        !matches!(self.state, ConversationState::Idle | ConversationState::Error { .. })
    }
}

/// Conversation state machine states
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationState {
    /// Ready for user input, no pending operations
    Idle,
    
    /// User message received, preparing LLM request
    AwaitingLlm,
    
    /// LLM request in flight, with retry tracking
    LlmRequesting { attempt: u32 },
    
    /// Executing tools serially
    ToolExecuting {
        current_tool: crate::state_machine::state::ToolCall,
        remaining_tools: Vec<crate::state_machine::state::ToolCall>,
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
    
    /// Error occurred
    Error {
        message: String,
        error_kind: ErrorKind,
    },
}

impl fmt::Display for ConversationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConversationState::Idle => write!(f, "idle"),
            ConversationState::AwaitingLlm => write!(f, "awaiting_llm"),
            ConversationState::LlmRequesting { .. } => write!(f, "llm_requesting"),
            ConversationState::ToolExecuting { .. } => write!(f, "tool_executing"),
            ConversationState::Cancelling { .. } => write!(f, "cancelling"),
            ConversationState::AwaitingSubAgents { .. } => write!(f, "awaiting_sub_agents"),
            ConversationState::Error { .. } => write!(f, "error"),
        }
    }
}

/// Error classification for UI display
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Auth,
    RateLimit,
    Network,
    InvalidRequest,
    Unknown,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub success: bool,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(tool_use_id: String, output: String) -> Self {
        Self {
            tool_use_id,
            success: true,
            output,
            is_error: false,
        }
    }

    pub fn error(tool_use_id: String, error: String) -> Self {
        Self {
            tool_use_id,
            success: false,
            output: error,
            is_error: true,
        }
    }

    pub fn cancelled(tool_use_id: String, message: &str) -> Self {
        Self {
            tool_use_id,
            success: false,
            output: message.to_string(),
            is_error: false,
        }
    }
}

/// Sub-agent result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubAgentResult {
    pub agent_id: String,
    pub success: bool,
    pub result: String,
}

/// Message record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub sequence_id: i64,
    pub message_type: MessageType,
    pub content: serde_json::Value,
    pub display_data: Option<serde_json::Value>,
    pub usage_data: Option<UsageData>,
    pub created_at: DateTime<Utc>,
}

/// Message type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    User,
    Agent,
    Tool,
    System,
    Error,
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageType::User => write!(f, "user"),
            MessageType::Agent => write!(f, "agent"),
            MessageType::Tool => write!(f, "tool"),
            MessageType::System => write!(f, "system"),
            MessageType::Error => write!(f, "error"),
        }
    }
}

/// Usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageData {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

impl UsageData {
    pub fn context_window_used(&self) -> u64 {
        self.input_tokens + self.output_tokens + 
        self.cache_creation_tokens + self.cache_read_tokens
    }
}
