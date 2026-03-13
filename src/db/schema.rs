//! Database schema and types

use crate::llm::ContentBlock;
pub use crate::state_machine::state::ConvState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::Path;

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
    message_id TEXT PRIMARY KEY,
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

/// Migration: replace `"unknown"` `error_kind` with `"server_error"` in JSON state.
/// The `Unknown` variant was removed from `ErrorKind`; old rows need updating
/// so serde can deserialize them.
pub const MIGRATION_REMOVE_UNKNOWN_ERROR_KIND: &str = r#"
UPDATE conversations
SET state = REPLACE(state, '"error_kind":"unknown"', '"error_kind":"server_error"')
WHERE state LIKE '%"error_kind":"unknown"%';
"#;

/// Migration SQL to add model column
#[allow(dead_code)] // Will be used in future
pub const MIGRATION_ADD_MODEL: &str = r"
-- This is a no-op if the column already exists
-- SQLite will return an error which we'll ignore
";

/// Migration SQL to create projects table
pub const MIGRATION_CREATE_PROJECTS: &str = r"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    canonical_path TEXT UNIQUE NOT NULL,
    main_ref TEXT NOT NULL DEFAULT 'main',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_projects_path ON projects(canonical_path);
";

/// Migration SQL to add `local_id` column for idempotent message sends
/// Migration to rename `messages.id` to `messages.message_id`
/// `SQLite` 3.25+ supports ALTER TABLE RENAME COLUMN
/// For older versions or if column already renamed, this is a no-op
pub const MIGRATION_RENAME_MESSAGE_ID: &str = r"
-- Rename id to message_id for searchability
-- This will fail silently if already renamed or SQLite is too old
ALTER TABLE messages RENAME COLUMN id TO message_id;
";

/// Conversation mode — determines tool availability and write access.
///
/// Stored as JSON in the `conv_mode` TEXT column on conversations.
/// REQ-BED-027: Conversation-level field, NOT embedded in `ConvState`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode")]
pub enum ConvMode {
    /// Read-only mode. No file writes, no bash (unless sandboxed).
    #[default]
    Explore,
    /// Standalone mode for non-git directories. Full tool suite, no project association.
    Standalone,
    /// Write mode on a task branch. Full tool suite with file write access.
    Work {
        /// The git branch name for this work conversation (e.g., `task-0042-fix-bug`)
        branch_name: String,
        /// Absolute path to the git worktree for this conversation.
        /// `#[serde(default)]` is a rollout shim for existing Work rows -- startup
        /// reconciliation reverts rows with empty `worktree_path` to Explore.
        #[serde(default)]
        worktree_path: String,
        /// The branch that was checked out when the task was approved (e.g., `main`).
        /// Used as the merge target for Complete and the restore target for Abandon.
        /// `#[serde(default)]` is a rollout shim -- startup reconciliation reverts
        /// rows with empty `base_branch` to Explore.
        #[serde(default)]
        base_branch: String,
        /// The task number assigned at approval time (e.g., 42).
        /// Used to locate and update the task file in `tasks/`.
        /// `#[serde(default)]` is a rollout shim for existing Work rows.
        #[serde(default)]
        task_number: u32,
    },
}

impl ConvMode {
    /// Human-readable label for UI display
    pub fn label(&self) -> &'static str {
        match self {
            Self::Explore => "Explore",
            Self::Standalone => "Standalone",
            Self::Work { .. } => "Work",
        }
    }

    /// The branch name if in Work mode, None otherwise.
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Self::Work { branch_name, .. } => Some(branch_name),
            Self::Explore | Self::Standalone => None,
        }
    }

    /// The worktree path if in Work mode, None otherwise.
    pub fn worktree_path(&self) -> Option<&str> {
        match self {
            Self::Work { worktree_path, .. } => Some(worktree_path),
            Self::Explore | Self::Standalone => None,
        }
    }

    /// The base branch if in Work mode, None otherwise.
    pub fn base_branch(&self) -> Option<&str> {
        match self {
            Self::Work { base_branch, .. } => Some(base_branch),
            Self::Explore | Self::Standalone => None,
        }
    }

    /// The task number if in Work mode, None otherwise.
    #[allow(dead_code)] // Used by M4 Complete/Abandon flows (task 0604)
    pub fn task_number(&self) -> Option<u32> {
        match self {
            Self::Work { task_number, .. } => Some(*task_number),
            Self::Explore | Self::Standalone => None,
        }
    }
}

/// Project record — a git repository tracked by Phoenix.
///
/// REQ-PROJ-001: Keyed by resolved git repo root path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub canonical_path: String,
    pub main_ref: String,
    pub created_at: DateTime<Utc>,
    /// Derived: count of non-archived conversations in this project
    #[serde(default)]
    pub conversation_count: i64,
}

/// Detect the git repository root for a given directory path.
///
/// Returns `None` if the path is not inside a git repository.
pub fn detect_git_repo_root(path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

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
    /// Project this conversation belongs to (None for legacy pre-project conversations)
    #[serde(default)]
    pub project_id: Option<String>,
    /// Conversation mode — determines tool availability. Default: Explore.
    #[serde(default)]
    pub conv_mode: ConvMode,
    #[serde(default)]
    pub message_count: i64,
}

impl Conversation {
    /// Check if the agent is currently working (derived from `display_state`)
    pub fn is_agent_working(&self) -> bool {
        self.state.display_state() == crate::state_machine::state::DisplayState::Working
    }
}

/// Error classification for UI display.
///
/// No `Unknown` variant. Every error gets an explicit, intentional classification.
/// Adding a new error class requires handling it in every consumer — the compiler
/// forces it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Rate limited (429) - retryable with backoff
    RateLimit,
    /// Network issues, connection failures - retryable
    Network,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Server error (5xx) - retryable
    ServerError,
    /// Request timed out - retryable
    TimedOut,
    /// Operation was cancelled - not retryable
    Cancelled,
    /// Sub-agent failed - not retryable
    SubAgentError,
    /// Context window exhausted - not retryable
    ContextExhausted,
    /// Content filter or safety block - not retryable
    ContentFilter,
}

impl ErrorKind {
    /// Returns true if this error type should trigger automatic retry
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network | Self::RateLimit | Self::ServerError | Self::TimedOut => true,
            Self::Auth
            | Self::InvalidRequest
            | Self::Cancelled
            | Self::SubAgentError
            | Self::ContextExhausted
            | Self::ContentFilter => false,
        }
    }
}

/// Image data in a tool result message (for LLM consumption).
/// Stored as JSON in `messages.content` alongside the text output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolContentImage {
    pub media_type: String,
    pub data: String,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub success: bool,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_data: Option<serde_json::Value>,
    /// Typed images for LLM consumption.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolContentImage>,
}

impl ToolResult {
    #[allow(dead_code)] // Constructor for API completeness
    pub fn success(tool_use_id: String, output: String) -> Self {
        Self {
            tool_use_id,
            success: true,
            output,
            is_error: false,
            display_data: None,
            images: vec![],
        }
    }

    pub fn error(tool_use_id: String, error: String) -> Self {
        Self {
            tool_use_id,
            success: false,
            output: error,
            is_error: true,
            display_data: None,
            images: vec![],
        }
    }

    pub fn cancelled(tool_use_id: String, message: &str) -> Self {
        Self {
            tool_use_id,
            success: false,
            output: message.to_string(),
            is_error: false,
            display_data: None,
            images: vec![],
        }
    }

    /// Create a successful result with display data for UI rendering
    #[allow(dead_code)]
    pub fn success_with_display(
        tool_use_id: String,
        output: String,
        display_data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            tool_use_id,
            success: true,
            output,
            is_error: false,
            display_data,
            images: vec![],
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
    /// Display text — stored in DB and shown in conversation history.
    /// For messages with `@` expansion this is the original shorthand (REQ-IR-006).
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageData>,
    /// Expanded text delivered to the LLM (REQ-IR-001).
    /// `None` means no expansion occurred and `text` is used verbatim for the LLM.
    /// `Some` holds the fully resolved form (e.g. `<file path="…">…</file>` blocks).
    /// `#[serde(default)]` handles old DB rows that predate this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_text: Option<String>,
}

impl UserContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            llm_text: None,
        }
    }

    pub fn with_images(text: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            text: text.into(),
            images,
            llm_text: None,
        }
    }

    /// Create a user message where `display_text` is stored/shown and `llm_text`
    /// is the expanded form delivered to the LLM (REQ-IR-001, REQ-IR-006).
    pub fn with_expansion(
        display_text: impl Into<String>,
        llm_text: impl Into<String>,
        images: Vec<ImageData>,
    ) -> Self {
        Self {
            text: display_text.into(),
            images,
            llm_text: Some(llm_text.into()),
        }
    }

    /// The text to deliver to the LLM: expanded form if present, display text otherwise.
    pub fn llm_text(&self) -> &str {
        self.llm_text.as_deref().unwrap_or(&self.text)
    }
}

/// Image attachment in a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageData {
    pub data: String,
    pub media_type: String,
}

impl ImageData {
    /// Convert to LLM `ImageSource` format
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
    /// Images to send to the LLM as image content blocks (not tokenized as text).
    /// `#[serde(default)]` ensures old DB rows (no `images` key) deserialize to empty vec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolContentImage>,
}

impl ToolContent {
    pub fn new(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
            images: vec![],
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

/// Continuation summary content (REQ-BED-021)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuationContent {
    pub summary: String,
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
    Continuation(ContinuationContent),
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
            Self::Continuation(_) => MessageType::Continuation,
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
            Self::Continuation(c) => serde_json::to_value(c).unwrap_or(Value::Null),
        }
    }

    /// Deserialize content from JSON value using the message type as discriminator
    pub fn from_json(msg_type: MessageType, value: Value) -> Result<Self, String> {
        match msg_type {
            MessageType::User => serde_json::from_value(value)
                .map(Self::User)
                .map_err(|e| format!("Invalid user content: {e}")),
            MessageType::Agent => serde_json::from_value(value)
                .map(Self::Agent)
                .map_err(|e| format!("Invalid agent content: {e}")),
            MessageType::Tool => serde_json::from_value(value)
                .map(Self::Tool)
                .map_err(|e| format!("Invalid tool content: {e}")),
            MessageType::System => serde_json::from_value(value)
                .map(Self::System)
                .map_err(|e| format!("Invalid system content: {e}")),
            MessageType::Error => serde_json::from_value(value)
                .map(Self::Error)
                .map_err(|e| format!("Invalid error content: {e}")),
            MessageType::Continuation => serde_json::from_value(value)
                .map(Self::Continuation)
                .map_err(|e| format!("Invalid continuation content: {e}")),
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
    pub fn tool(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
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
        Self::Error(ErrorContent {
            message: message.into(),
        })
    }

    /// Create continuation summary content
    pub fn continuation(summary: impl Into<String>) -> Self {
        Self::Continuation(ContinuationContent {
            summary: summary.into(),
        })
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
            Self::Continuation(c) => c.serialize(serializer),
        }
    }
}

/// Message record
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_field_names)]
pub struct Message {
    pub message_id: String,
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
    Continuation,
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageType::User => write!(f, "user"),
            MessageType::Agent => write!(f, "agent"),
            MessageType::Tool => write!(f, "tool"),
            MessageType::System => write!(f, "system"),
            MessageType::Error => write!(f, "error"),
            MessageType::Continuation => write!(f, "continuation"),
        }
    }
}

/// Type alias for backward compatibility — `Usage` is the canonical type.
pub type UsageData = crate::llm::Usage;

#[cfg(test)]
mod error_kind_tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        // These should be retryable
        assert!(
            ErrorKind::Network.is_retryable(),
            "Network errors should be retryable"
        );
        assert!(
            ErrorKind::RateLimit.is_retryable(),
            "Rate limit errors should be retryable"
        );
        assert!(
            ErrorKind::ServerError.is_retryable(),
            "Server errors (5xx) should be retryable"
        );
        assert!(
            ErrorKind::TimedOut.is_retryable(),
            "Timeout errors should be retryable"
        );
    }

    #[test]
    fn test_non_retryable_errors() {
        // These should NOT be retryable
        assert!(
            !ErrorKind::Auth.is_retryable(),
            "Auth errors should not be retryable"
        );
        assert!(
            !ErrorKind::InvalidRequest.is_retryable(),
            "Invalid request errors should not be retryable"
        );
        assert!(
            !ErrorKind::Cancelled.is_retryable(),
            "Cancelled errors should not be retryable"
        );
        assert!(
            !ErrorKind::SubAgentError.is_retryable(),
            "Sub-agent errors should not be retryable"
        );
        assert!(
            !ErrorKind::ContextExhausted.is_retryable(),
            "Context exhausted errors should not be retryable"
        );
        assert!(
            !ErrorKind::ContentFilter.is_retryable(),
            "Content filter errors should not be retryable"
        );
    }

    #[test]
    fn test_error_kind_serialization() {
        // Ensure ServerError serializes correctly (for DB/SSE compatibility)
        let json = serde_json::to_string(&ErrorKind::ServerError).unwrap();
        assert_eq!(json, "\"server_error\"");

        let parsed: ErrorKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ErrorKind::ServerError);
    }
}
