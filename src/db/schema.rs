//! Database schema and types

use crate::llm::ContentBlock;
pub use crate::state_machine::state::ConvState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::Path;

/// A string guaranteed to be non-empty at construction time.
/// Serde deserialization rejects empty strings.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    pub fn new(s: impl Into<String>) -> Result<Self, &'static str> {
        let s = s.into();
        if s.is_empty() {
            Err("string must not be empty")
        } else {
            Ok(Self(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.is_empty() {
            Err(serde::de::Error::custom("string must not be empty"))
        } else {
            Ok(Self(s))
        }
    }
}

/// Validated worktree configuration fields shared by Work and Branch modes.
///
/// Not embedded via `#[serde(flatten)]` because serde's internally-tagged enums
/// don't support flatten. Exists as a logical grouping for accessor methods and
/// future extraction.
#[allow(dead_code)] // Introduced for A2/C1 phases; `worktree_config()` exercises it now
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeConfig {
    pub branch_name: NonEmptyString,
    pub worktree_path: NonEmptyString,
    pub base_branch: NonEmptyString,
}

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

CREATE TABLE IF NOT EXISTS turn_usage (
    id INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    root_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_turn_usage_conversation ON turn_usage(conversation_id);
CREATE INDEX IF NOT EXISTS idx_turn_usage_root ON turn_usage(root_conversation_id);
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

/// Migration SQL to create `mcp_disabled_servers` table.
pub const MIGRATION_CREATE_MCP_DISABLED_SERVERS: &str = r"
CREATE TABLE IF NOT EXISTS mcp_disabled_servers (
    server_name TEXT PRIMARY KEY
);
";

/// Migration SQL to create `share_tokens` table (REQ-AUTH-008).
pub const MIGRATION_CREATE_SHARE_TOKENS: &str = r"
CREATE TABLE IF NOT EXISTS share_tokens (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_share_tokens_token ON share_tokens(token);
CREATE INDEX IF NOT EXISTS idx_share_tokens_conversation ON share_tokens(conversation_id);
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
///
/// All string fields in Work and Branch use `NonEmptyString` to make empty
/// strings structurally unrepresentable. The migration system backfills
/// legacy rows; deserialization of missing fields is now a hard error.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode")]
pub enum ConvMode {
    /// Read-only mode. No file writes, no bash (unless sandboxed).
    /// Opt-in "Managed" workflow: `propose_task` available, gateway to Work.
    #[default]
    Explore,
    /// Direct mode: full tool access, no lifecycle ceremony.
    /// Default for all new conversations (git and non-git).
    Direct,
    /// Write mode on a task branch (Managed workflow). Full tool suite with file write access.
    Work {
        /// The git branch name for this work conversation (e.g., `task-0042-fix-bug`)
        branch_name: NonEmptyString,
        /// Absolute path to the git worktree for this conversation.
        worktree_path: NonEmptyString,
        /// The branch that was checked out when the task was approved (e.g., `main`).
        /// Used as the merge target for Complete and the restore target for Abandon.
        base_branch: NonEmptyString,
        /// The task ID assigned at approval time (e.g., "YF042").
        /// Used to locate and update the task file in `tasks/`.
        task_id: NonEmptyString,
        /// Human-readable task title (e.g., "Fix auth middleware token storage").
        task_title: NonEmptyString,
    },
    /// Branch mode: work directly on an existing branch (e.g., fix a PR).
    /// No task file, no Explore phase. Full tool access.
    /// REQ-PROJ-024
    Branch {
        /// The existing branch name (e.g., "q-branch-observer")
        branch_name: NonEmptyString,
        /// Absolute path to the git worktree
        worktree_path: NonEmptyString,
        /// The branch this worktree was created from (same as `branch_name` for Branch mode)
        base_branch: NonEmptyString,
    },
}

impl ConvMode {
    /// Human-readable label for UI display
    pub fn label(&self) -> &'static str {
        match self {
            Self::Explore => "Explore",
            Self::Direct => "Direct",
            Self::Work { .. } => "Work",
            Self::Branch { .. } => "Branch",
        }
    }

    /// The branch name if in Work or Branch mode, None otherwise.
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Self::Work { branch_name, .. } | Self::Branch { branch_name, .. } => {
                Some(branch_name.as_str())
            }
            Self::Explore | Self::Direct => None,
        }
    }

    /// The worktree path if in Work or Branch mode, None otherwise.
    pub fn worktree_path(&self) -> Option<&str> {
        match self {
            Self::Work { worktree_path, .. } | Self::Branch { worktree_path, .. } => {
                Some(worktree_path.as_str())
            }
            Self::Explore | Self::Direct => None,
        }
    }

    /// The base branch if in Work or Branch mode, None otherwise.
    pub fn base_branch(&self) -> Option<&str> {
        match self {
            Self::Work { base_branch, .. } | Self::Branch { base_branch, .. } => {
                Some(base_branch.as_str())
            }
            Self::Explore | Self::Direct => None,
        }
    }

    /// The task ID if in Work mode, None otherwise. Branch mode has no task.
    #[allow(dead_code)] // Used by M4 Complete/Abandon flows (task 0604)
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Self::Work { task_id, .. } => Some(task_id.as_str()),
            Self::Explore | Self::Direct | Self::Branch { .. } => None,
        }
    }

    /// The task title if in Work mode, None otherwise. Branch mode has no task.
    pub fn task_title(&self) -> Option<&str> {
        match self {
            Self::Work { task_title, .. } => Some(task_title.as_str()),
            Self::Explore | Self::Direct | Self::Branch { .. } => None,
        }
    }

    /// Extract `WorktreeConfig` from Work or Branch mode. Returns None for
    /// Explore and Direct.
    #[allow(dead_code)] // Introduced for A2/C1 phases; tested in conv_mode_tests
    pub fn worktree_config(&self) -> Option<WorktreeConfig> {
        match self {
            Self::Work {
                branch_name,
                worktree_path,
                base_branch,
                ..
            }
            | Self::Branch {
                branch_name,
                worktree_path,
                base_branch,
            } => Some(WorktreeConfig {
                branch_name: branch_name.clone(),
                worktree_path: worktree_path.clone(),
                base_branch: base_branch.clone(),
            }),
            Self::Explore | Self::Direct => None,
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
    /// Human-readable title for UI display (e.g., "Fix Login Page CSS").
    /// Derived from the slug by title-casing when not set explicitly.
    #[serde(default)]
    pub title: Option<String>,
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
    /// Desired base branch for Managed mode (set at creation, consumed at task approval).
    /// `#[serde(default)]` handles old DB rows that predate this column.
    #[serde(default)]
    pub desired_base_branch: Option<String>,
    #[serde(default)]
    pub message_count: i64,
    /// Seed parent for decorative UI breadcrumb (REQ-SEED-003). Distinct from
    /// `parent_conversation_id` above (which is sub-agent parentage); this one
    /// is set when a user-initiated conversation was spawned from another via
    /// a "seed" action. Never traversed by runtime logic.
    #[serde(default)]
    pub seed_parent_id: Option<String>,
    /// Seed label for decorative UI display (REQ-SEED-004).
    #[serde(default)]
    pub seed_label: Option<String>,
    /// Continuation pointer — if this conversation has been continued into a
    /// new conversation (REQ-BED-030), this is the continuation's id. Nullable
    /// for all conversations that have not been continued. When set, this
    /// conversation no longer owns its worktree; the continuation does.
    /// `#[serde(default)]` handles old DB rows that predate this column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued_in_conv_id: Option<String>,
}

/// Derive a human-readable title from a kebab-case slug.
/// E.g., "my-test-conversation" -> "My Test Conversation"
pub fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    format!("{upper}{}", chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// Outcome of a tool execution. Replaces the contradictory `success: bool` +
/// `is_error: bool` pair — this enum makes the three meaningful states explicit
/// and the fourth (`success=false`, `is_error=false` but not cancelled) unrepresentable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutcome {
    Success {
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_data: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ToolContentImage>,
    },
    Error {
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_data: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ToolContentImage>,
    },
    Cancelled {
        message: String,
    },
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub outcome: ToolOutcome,
}

impl ToolResult {
    pub fn success(tool_use_id: String, output: String) -> Self {
        Self {
            tool_use_id,
            outcome: ToolOutcome::Success {
                output,
                display_data: None,
                images: vec![],
            },
        }
    }

    pub fn error(tool_use_id: String, error: String) -> Self {
        Self {
            tool_use_id,
            outcome: ToolOutcome::Error {
                output: error,
                display_data: None,
                images: vec![],
            },
        }
    }

    pub fn cancelled(tool_use_id: String, message: &str) -> Self {
        Self {
            tool_use_id,
            outcome: ToolOutcome::Cancelled {
                message: message.to_string(),
            },
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
            outcome: ToolOutcome::Success {
                output,
                display_data,
                images: vec![],
            },
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.outcome, ToolOutcome::Error { .. })
    }

    #[allow(dead_code)] // Used in tests; main code uses is_error()
    pub fn is_success(&self) -> bool {
        matches!(self.outcome, ToolOutcome::Success { .. })
    }

    pub fn output(&self) -> &str {
        match &self.outcome {
            ToolOutcome::Success { output, .. } | ToolOutcome::Error { output, .. } => output,
            ToolOutcome::Cancelled { message } => message,
        }
    }

    pub fn display_data(&self) -> Option<&serde_json::Value> {
        match &self.outcome {
            ToolOutcome::Success { display_data, .. } | ToolOutcome::Error { display_data, .. } => {
                display_data.as_ref()
            }
            ToolOutcome::Cancelled { .. } => None,
        }
    }

    #[allow(dead_code)] // Public API for completeness; used by tests
    pub fn images(&self) -> &[ToolContentImage] {
        match &self.outcome {
            ToolOutcome::Success { images, .. } | ToolOutcome::Error { images, .. } => images,
            ToolOutcome::Cancelled { .. } => &[],
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
    /// System-generated user message (e.g., task approval). Delivered to the LLM
    /// as user role but rendered distinctly in the UI (no "You" label).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_meta: bool,
}

impl UserContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            llm_text: None,
            is_meta: false,
        }
    }

    pub fn with_images(text: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self {
            text: text.into(),
            images,
            llm_text: None,
            is_meta: false,
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
            is_meta: false,
        }
    }

    /// Create a system-generated user message (task approval, mode transitions).
    /// Delivered to the LLM as user role but rendered distinctly in the UI.
    pub fn meta(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
            llm_text: None,
            is_meta: true,
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

/// Skill invocation content (REQ-SK-002)
///
/// Delivered as a user-role message to the LLM but marked as system-generated
/// in conversation history. Carries the skill name, fully expanded body, and
/// the original user text that triggered the invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillContent {
    /// The skill name (e.g., "build")
    pub name: String,
    /// The fully expanded skill body (frontmatter stripped, base directory
    /// prepended, arguments substituted)
    pub body: String,
    /// The original user text that triggered the invocation (for display)
    pub trigger: String,
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
    /// Skill invocation -- delivered as a user-role message to the LLM
    /// but marked as system-generated in conversation history (REQ-SK-002)
    Skill(SkillContent),
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
            Self::Skill(_) => MessageType::Skill,
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
            Self::Skill(c) => serde_json::to_value(c).unwrap_or(Value::Null),
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
            MessageType::Skill => serde_json::from_value(value)
                .map(Self::Skill)
                .map_err(|e| format!("Invalid skill content: {e}")),
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
            Self::Skill(c) => c.serialize(serializer),
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/generated/")]
pub enum MessageType {
    User,
    Agent,
    Tool,
    System,
    Error,
    Continuation,
    Skill,
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
            MessageType::Skill => write!(f, "skill"),
        }
    }
}

/// Type alias for backward compatibility — `Usage` is the canonical type.
pub type UsageData = crate::llm::Usage;

/// Aggregated token counts and turn count for a query scope.
#[derive(Debug, Serialize)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub turns: i64,
}

/// Token usage for a conversation, broken out by scope.
///
/// `own` covers only the conversation itself; `total` includes all sub-agents
/// that share the same root conversation id.
#[derive(Debug, Serialize)]
pub struct ConversationUsage {
    pub own: UsageTotals,
    pub total: UsageTotals,
}

#[cfg(test)]
mod conv_mode_tests {
    use super::*;

    #[test]
    fn test_direct_serialization() {
        let json = serde_json::to_string(&ConvMode::Direct).unwrap();
        assert_eq!(json, r#"{"mode":"Direct"}"#);
        let parsed: ConvMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ConvMode::Direct);
    }

    #[test]
    fn test_standalone_no_longer_deserializes() {
        // Migration 001 rewrites "Standalone" -> "Direct" in the DB.
        // The serde alias is removed; raw "Standalone" JSON is now rejected.
        let old_json = r#"{"mode":"Standalone"}"#;
        assert!(serde_json::from_str::<ConvMode>(old_json).is_err());
    }

    #[test]
    fn test_explore_still_works() {
        let json = r#"{"mode":"Explore"}"#;
        let parsed: ConvMode = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, ConvMode::Explore);
    }

    #[test]
    fn test_branch_serialization() {
        let mode = ConvMode::Branch {
            branch_name: NonEmptyString::new("fix-login").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
        };
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: ConvMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
        // Verify no task_id in JSON
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("task_id").is_none());
    }

    #[test]
    fn test_work_serialization_roundtrip() {
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new("task-0042-fix-bug").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt/abc").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("YF042").unwrap(),
            task_title: NonEmptyString::new("Fix the bug").unwrap(),
        };
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: ConvMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);
    }

    #[test]
    fn test_non_empty_string_rejects_empty() {
        assert!(NonEmptyString::new("").is_err());
        assert!(NonEmptyString::new("ok").is_ok());
    }

    #[test]
    fn test_non_empty_string_serde_rejects_empty() {
        let result: Result<NonEmptyString, _> = serde_json::from_str(r#""""#);
        assert!(result.is_err());
    }

    #[test]
    fn test_work_missing_fields_is_hard_error() {
        // After migration cleanup, missing fields are rejected (no serde(default))
        let json = r#"{"mode":"Work","branch_name":"old-branch"}"#;
        assert!(serde_json::from_str::<ConvMode>(json).is_err());
    }

    #[test]
    fn test_worktree_config_extraction() {
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new("task-1").unwrap(),
            worktree_path: NonEmptyString::new("/wt").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("T1").unwrap(),
            task_title: NonEmptyString::new("Title").unwrap(),
        };
        let config = mode.worktree_config().unwrap();
        assert_eq!(config.branch_name.as_str(), "task-1");
        assert_eq!(config.worktree_path.as_str(), "/wt");
        assert_eq!(config.base_branch.as_str(), "main");

        assert!(ConvMode::Explore.worktree_config().is_none());
        assert!(ConvMode::Direct.worktree_config().is_none());
    }
}

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

#[cfg(test)]
mod conversation_serde_tests {
    use super::*;
    use chrono::TimeZone;

    fn fixture_ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap()
    }

    fn fixture(continued_in_conv_id: Option<String>) -> Conversation {
        Conversation {
            id: "conv-1".to_string(),
            slug: Some("test-conv".to_string()),
            title: Some("Test Conv".to_string()),
            cwd: "/tmp/work".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: fixture_ts(),
            created_at: fixture_ts(),
            updated_at: fixture_ts(),
            archived: false,
            model: None,
            project_id: None,
            conv_mode: ConvMode::Explore,
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id,
        }
    }

    /// REQ-BED-030 Phase 1: Conversation round-trips through serde with
    /// `continued_in_conv_id` absent (the default for pre-continuation rows).
    /// The field uses `skip_serializing_if = "Option::is_none"`, so the wire
    /// form omits the key entirely when None.
    #[test]
    fn continued_in_conv_id_none_round_trips() {
        let original = fixture(None);
        let json = serde_json::to_value(&original).unwrap();
        assert!(
            json.get("continued_in_conv_id").is_none(),
            "None should be omitted from serialization, got: {json}"
        );
        let parsed: Conversation = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.continued_in_conv_id, None);
        assert_eq!(parsed.id, original.id);
    }

    /// REQ-BED-030 Phase 1: Conversation round-trips with
    /// `continued_in_conv_id = Some(...)` — the wire form includes the key
    /// and deserialization preserves the pointer.
    #[test]
    fn continued_in_conv_id_some_round_trips() {
        let original = fixture(Some("other-conv-id".to_string()));
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(
            json.get("continued_in_conv_id"),
            Some(&serde_json::Value::String("other-conv-id".to_string())),
        );
        let parsed: Conversation = serde_json::from_value(json).unwrap();
        assert_eq!(
            parsed.continued_in_conv_id,
            Some("other-conv-id".to_string())
        );
    }

    /// REQ-BED-030 Phase 1: legacy DB rows that predate the column deserialize
    /// cleanly — `#[serde(default)]` fills `None` when the key is absent.
    #[test]
    fn continued_in_conv_id_defaults_to_none_for_legacy_rows() {
        let mut json = serde_json::to_value(fixture(None)).unwrap();
        if let serde_json::Value::Object(ref mut map) = json {
            map.remove("continued_in_conv_id");
        }
        let parsed: Conversation = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.continued_in_conv_id, None);
    }
}
