//! Database schema and types

use crate::llm::ContentBlock;
pub use crate::state_machine::state::ConvState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// SQL schema for initialization
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    slug TEXT UNIQUE,
    cwd TEXT NOT NULL,
    parent_conversation_id TEXT,
    user_initiated BOOLEAN NOT NULL,
    state TEXT NOT NULL DEFAULT '{"type":"idle"}',
    state_data TEXT,
    state_updated_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT 0,
    model TEXT,
    
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

/// Migration SQL to convert old state format to typed JSON
/// Runs at startup to ensure all state values are valid JSON
pub const MIGRATION_TYPED_STATE: &str = r#"
-- Migrate old string-based state to JSON format
-- Only runs if there are non-JSON state values

-- Convert 'idle' string to JSON
UPDATE conversations SET state = '{"type":"idle"}' WHERE state = 'idle';

-- Convert all other non-JSON states to idle (they would be reset on startup anyway)
-- This handles: awaiting_llm, llm_requesting, tool_executing, etc.
UPDATE conversations SET state = '{"type":"idle"}', state_data = NULL 
WHERE state NOT LIKE '{%}';
"#;

/// Migration SQL to add model column
pub const MIGRATION_ADD_MODEL: &str = r#"
-- This is a no-op if the column already exists
-- SQLite will return an error which we'll ignore
"#;

/// Conversation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub slug: Option<String>,
    pub cwd: String,
    pub parent_conversation_id: Option<String>,
    pub user_initiated: bool,
    pub state: ConvState,
    pub state_updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived: bool,
    pub model: Option<String>,
}

impl Conversation {
    /// Check if the agent is currently working
    pub fn is_agent_working(&self) -> bool {
        !matches!(
            self.state,
            ConvState::Idle | ConvState::Error { .. }
        )
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
    TimedOut,
    Cancelled,
    SubAgentError,
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
    #[allow(dead_code)] // Constructor for API completeness
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

// SubAgentResult is now in state_machine::state

// ============================================================
// Message Content Types
// ============================================================

/// User message content
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserContent {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
}

impl UserContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn with_images(text: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            text: text.into(),
            images,
        }
    }
}

/// Image attachment in a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageData {
    pub data: String,
    pub media_type: String,
}

impl ImageData {
    /// Convert to LLM ImageSource format
    pub fn to_image_source(&self) -> crate::llm::ImageSource {
        crate::llm::ImageSource::Base64 {
            media_type: self.media_type.clone(),
            data: self.data.clone(),
        }
    }
}

/// Tool result message content
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolContent {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

impl ToolContent {
    pub fn new(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
        }
    }
}

/// System message content
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemContent {
    pub text: String,
}

/// Error message content
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorContent {
    pub message: String,
}

/// Typed message content
/// 
/// This enum provides type safety for message content while maintaining
/// backward compatibility with the database schema where `message_type`
/// and `content` are stored as separate columns.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    User(UserContent),
    Agent(Vec<ContentBlock>),
    Tool(ToolContent),
    System(SystemContent),
    Error(ErrorContent),
}

impl MessageContent {
    /// Get the message type for this content
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::User(_) => MessageType::User,
            Self::Agent(_) => MessageType::Agent,
            Self::Tool(_) => MessageType::Tool,
            Self::System(_) => MessageType::System,
            Self::Error(_) => MessageType::Error,
        }
    }

    /// Serialize content to JSON value (without type tag)
    pub fn to_json(&self) -> Value {
        match self {
            Self::User(c) => serde_json::to_value(c).unwrap_or(Value::Null),
            Self::Agent(c) => serde_json::to_value(c).unwrap_or(Value::Null),
            Self::Tool(c) => serde_json::to_value(c).unwrap_or(Value::Null),
            Self::System(c) => serde_json::to_value(c).unwrap_or(Value::Null),
            Self::Error(c) => serde_json::to_value(c).unwrap_or(Value::Null),
        }
    }

    /// Deserialize content from JSON value using the message type as discriminator
    pub fn from_json(msg_type: MessageType, value: Value) -> Result<Self, String> {
        match msg_type {
            MessageType::User => {
                serde_json::from_value(value)
                    .map(Self::User)
                    .map_err(|e| format!("Invalid user content: {e}"))
            }
            MessageType::Agent => {
                serde_json::from_value(value)
                    .map(Self::Agent)
                    .map_err(|e| format!("Invalid agent content: {e}"))
            }
            MessageType::Tool => {
                serde_json::from_value(value)
                    .map(Self::Tool)
                    .map_err(|e| format!("Invalid tool content: {e}"))
            }
            MessageType::System => {
                serde_json::from_value(value)
                    .map(Self::System)
                    .map_err(|e| format!("Invalid system content: {e}"))
            }
            MessageType::Error => {
                serde_json::from_value(value)
                    .map(Self::Error)
                    .map_err(|e| format!("Invalid error content: {e}"))
            }
        }
    }

    /// Create user content
    pub fn user(text: impl Into<String>) -> Self {
        Self::User(UserContent::new(text))
    }

    /// Create user content with images
    pub fn user_with_images(text: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self::User(UserContent::with_images(text, images))
    }

    /// Create agent content
    pub fn agent(blocks: Vec<ContentBlock>) -> Self {
        Self::Agent(blocks)
    }

    /// Create tool content
    pub fn tool(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self::Tool(ToolContent::new(tool_use_id, content, is_error))
    }

    /// Create system content
    #[allow(dead_code)] // Constructor for API completeness
    pub fn system(text: impl Into<String>) -> Self {
        Self::System(SystemContent { text: text.into() })
    }

    /// Create error content
    #[allow(dead_code)] // Used as fallback for parse errors
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(ErrorContent { message: message.into() })
    }
}

// Custom Serialize for MessageContent - just serializes the inner value
impl Serialize for MessageContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::User(c) => c.serialize(serializer),
            Self::Agent(c) => c.serialize(serializer),
            Self::Tool(c) => c.serialize(serializer),
            Self::System(c) => c.serialize(serializer),
            Self::Error(c) => c.serialize(serializer),
        }
    }
}

/// Message record
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub sequence_id: i64,
    pub message_type: MessageType,
    pub content: MessageContent,
    pub display_data: Option<Value>,
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
#[allow(clippy::struct_field_names)] // tokens suffix is meaningful
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
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}
