//! API request and response types

use serde::{Deserialize, Serialize};

/// Request to create a new conversation with initial message
#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub cwd: String,
    pub model: Option<String>,
    /// Initial message text (required)
    pub text: String,
    /// Client-generated message ID for idempotency
    pub message_id: String,
    /// Optional image attachments
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
    /// Conversation mode: "managed" for Explore/Work lifecycle, omit or "direct" for full access.
    /// "managed" requires a git repository.
    #[serde(default)]
    pub mode: Option<String>,
    /// Desired base branch for Managed mode. If None, uses currently checked-out branch.
    #[serde(default)]
    pub base_branch: Option<String>,
}

/// Request to upgrade a conversation's model
#[derive(Debug, Deserialize)]
pub struct UpgradeModelRequest {
    /// Target model ID (e.g., "claude-sonnet-4-6-1m")
    pub model: String,
}

/// Request to send a chat message
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub text: String,
    /// Client-generated UUID - the canonical identifier for this message
    /// Enables idempotent retries (sending same `message_id` twice = no duplicate)
    pub message_id: String,
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
    /// Browser user agent for display (e.g., show iPhone icon)
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// Image attachment in a chat message
#[derive(Debug, Clone, Deserialize)]
pub struct ImageAttachment {
    pub data: String,
    pub media_type: String,
}

/// Request to rename a conversation
#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}

/// Response with a list of conversations
#[derive(Debug, Serialize)]
pub struct ConversationListResponse {
    pub conversations: Vec<serde_json::Value>,
}

/// Response with a single conversation
#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub conversation: serde_json::Value,
}

/// Response with conversation and messages
#[derive(Debug, Serialize)]
pub struct ConversationWithMessagesResponse {
    pub conversation: serde_json::Value,
    pub messages: Vec<serde_json::Value>,
    pub agent_working: bool,
    /// Semantic state category: idle, working, error, terminal
    pub display_state: String,
    pub context_window_size: u64,
}

/// Response for chat action
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub queued: bool,
}

/// Response for cancel action
#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub ok: bool,
}

/// Response for lifecycle actions
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

/// Response for directory validation
#[derive(Debug, Serialize)]
pub struct ValidateCwdResponse {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the directory is inside a git repository.
    pub is_git: bool,
}

/// Response for directory listing
#[derive(Debug, Serialize)]
pub struct ListDirectoryResponse {
    pub entries: Vec<DirectoryEntry>,
}

/// Response for mkdir
#[derive(Debug, Serialize)]
pub struct MkdirResponse {
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Directory entry
#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Enhanced file entry for file browser (REQ-PF-001 through REQ-PF-004)
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_time: Option<u64>, // Unix timestamp in seconds
    pub file_type: String, // folder, markdown, code, config, text, image, data, unknown
    pub is_text_file: bool,
    #[serde(default)]
    pub is_gitignored: bool,
}

/// Response for file listing
#[derive(Debug, Serialize)]
pub struct ListFilesResponse {
    pub items: Vec<FileEntry>,
}

/// Response for file reading
#[derive(Debug, Serialize)]
pub struct ReadFileResponse {
    pub content: String,
    pub encoding: String,
}

/// Error response for file operations
#[derive(Debug, Serialize)]
#[allow(dead_code)] // Reserved for future use
pub struct FileErrorResponse {
    pub error: String,
    pub is_binary: bool,
}

/// Model information with metadata
#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub description: String,
    pub context_window: usize,
    pub recommended: bool,
}

/// Gateway reachability status surfaced to the frontend
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayStatusApi {
    /// No gateway is configured; running in direct API-key mode
    NotConfigured,
    /// Gateway is configured and was reachable at startup
    Healthy,
    /// Gateway is configured but was unreachable at startup
    Unreachable,
}

/// Response for model list
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
    pub default: String,
    /// Gateway reachability status determined at startup
    pub gateway_status: GatewayStatusApi,
    /// True when at least one LLM provider is configured (gateway or direct key)
    pub llm_configured: bool,
}

/// Response containing the current system prompt for a conversation
#[derive(Debug, Serialize)]
pub struct SystemPromptResponse {
    pub system_prompt: String,
}

/// A single file search result (REQ-IR-004)
#[derive(Debug, Serialize)]
pub struct FileSearchEntry {
    /// Path relative to the conversation's working directory
    pub path: String,
    /// True when the file can be read as text (false = binary)
    pub is_text_file: bool,
}

/// Response for conversation-scoped file search (REQ-IR-004)
#[derive(Debug, Serialize)]
pub struct FileSearchResponse {
    pub items: Vec<FileSearchEntry>,
}

/// Query parameters for file search
#[derive(Debug, Deserialize)]
pub struct FileSearchQuery {
    /// Fuzzy query string (empty = return all up to limit)
    #[serde(default)]
    pub q: String,
    /// Maximum number of results (default 50)
    pub limit: Option<usize>,
}

/// A single skill entry returned by the skills API (REQ-IR-005)
#[derive(Debug, Serialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Where this skill was discovered (e.g., ".claude/skills" or ".agents/skills")
    pub source: String,
    /// Absolute path to the SKILL.md file
    pub path: String,
}

/// Response for the skills list endpoint (REQ-IR-005)
#[derive(Debug, Serialize)]
pub struct SkillsResponse {
    pub skills: Vec<SkillEntry>,
}

/// A task file entry returned by the tasks list endpoint.
#[derive(Debug, Serialize)]
pub struct TaskEntry {
    pub id: String,
    pub priority: String,
    pub status: String,
    pub slug: String,
    /// Slug of the conversation working on this task (if any active Work conversation owns it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_slug: Option<String>,
}

/// Response for the tasks list endpoint.
#[derive(Debug, Serialize)]
pub struct TasksResponse {
    pub tasks: Vec<TaskEntry>,
}

/// Expansion error detail returned to the frontend (REQ-IR-007)
#[derive(Debug, Clone, Serialize)]
pub struct ExpansionErrorResponse {
    pub error: String,
    pub error_type: String,
    /// The reference token that caused the failure (e.g. `@src/missing.rs` or `/skill-name`)
    pub reference: String,
}

/// Request to provide feedback on a proposed task plan
#[derive(Debug, Deserialize)]
pub struct TaskFeedbackRequest {
    pub annotations: String,
}

/// Response for task approval actions
#[derive(Debug, Serialize)]
pub struct TaskApprovalResponse {
    pub success: bool,
    /// True when this was the first task created in the project (tasks/ didn't exist)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_task: Option<bool>,
}

/// Response for the complete-task pre-check endpoint (REQ-PROJ-009)
#[derive(Debug, Serialize)]
pub struct CompleteTaskResponse {
    pub success: bool,
    pub commit_message: String,
    /// True when the task file exists but its status is not `done`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_not_done: Option<bool>,
}

/// Request body for confirm-complete endpoint (REQ-PROJ-009)
#[derive(Debug, Deserialize)]
pub struct ConfirmCompleteRequest {
    pub commit_message: String,
    /// If true, auto-stash dirty main checkout before merge and pop after.
    #[serde(default)]
    pub auto_stash: bool,
}

/// Response for confirm-complete endpoint (REQ-PROJ-009)
#[derive(Debug, Serialize)]
pub struct ConfirmCompleteResponse {
    pub success: bool,
    pub commit_sha: String,
    /// Warning message (e.g., stash pop failure) — displayed to user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// 409 Conflict error with typed `error_type` for frontend dispatch
#[derive(Debug, Serialize)]
pub struct ConflictErrorResponse {
    pub error: String,
    pub error_type: String,
    /// Dirty files on the main checkout (only for `dirty_main_checkout`)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dirty_files: Vec<String>,
    /// Whether auto-stash is safe (stash will pop cleanly after merge)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub can_auto_stash: bool,
}

impl ConflictErrorResponse {
    pub fn new(error: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            error_type: error_type.into(),
            dirty_files: vec![],
            can_auto_stash: false,
        }
    }
}

/// Query parameters for listing git branches
#[derive(Debug, Deserialize)]
pub struct GitBranchesQuery {
    pub cwd: String,
}

/// Response for git branch listing
#[derive(Debug, Serialize)]
pub struct GitBranchesResponse {
    pub branches: Vec<String>,
    pub current: String,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}
