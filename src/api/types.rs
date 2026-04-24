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
    /// Seed parent conversation id (REQ-SEED-003). Decorative link only; the
    /// spawned conversation runs independently.
    #[serde(default)]
    pub seed_parent_id: Option<String>,
    /// Seed label (REQ-SEED-004). Short human-readable context string shown in
    /// the seeded conversation's breadcrumb.
    #[serde(default)]
    pub seed_label: Option<String>,
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

/// Response for cancel action.
///
/// `ok` is always true; `no_op` is `true` when the conversation was already
/// idle or terminal (nothing to cancel). Callers that need to distinguish
/// "cancelled in-flight work" from "already idle" should check `no_op`.
/// Task 24682: this replaces the earlier behaviour where cancelling an
/// idle conversation would dispatch `UserCancel`, fail the state
/// transition, and broadcast a raw `InvalidTransition` error via SSE.
#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_op: bool,
}

/// Response for lifecycle actions
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

/// Response for the context-continuation transfer endpoint (REQ-BED-030).
///
/// Returned from `POST /api/conversations/:id/continue`. The caller receives
/// the id (and slug, if present) of the continuation conversation. When the
/// parent already had a continuation, this returns that existing id
/// idempotently — callers distinguish "just created" from "already existed"
/// via the `already_existed` flag.
#[derive(Debug, Serialize)]
pub struct ContinueConversationResponse {
    pub conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// True iff the parent already had a continuation when the endpoint was
    /// called. The UI can use this to route directly (vs. announcing the
    /// continuation as fresh). Always serialized so the wire shape matches
    /// the typed client contract — callers don't have to treat absent as
    /// false.
    pub already_existed: bool,
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

/// Credential helper status surfaced to the frontend
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatusApi {
    /// No credential helper configured and no static API key set.
    NotConfigured,
    /// A valid credential is available (static key, env var, or cached helper result).
    Valid,
    /// Helper configured but no valid cached credential — user must authenticate.
    Required,
    /// Helper subprocess is currently executing.
    Running,
    /// Last helper run exited non-zero or produced no output.
    Failed,
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
    /// Credential helper status (only meaningful when helper is configured).
    pub credential_status: CredentialStatusApi,
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
    /// Absolute path to the task file on disk. Used by the UI to fetch the task
    /// body via the generic file read endpoint when seeding a "start working
    /// on this task" conversation.
    pub path: String,
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
    /// Slug of the conversation that owns the contested resource (branch
    /// already active, etc.) — the UI routes to this slug instead of showing
    /// the error text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_slug: Option<String>,
    /// Id of the continuation conversation when the action was rejected because
    /// the parent has been continued (`error_type = "continuation_exists"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<String>,
}

impl ConflictErrorResponse {
    pub fn new(error: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            error_type: error_type.into(),
            dirty_files: vec![],
            can_auto_stash: false,
            conflict_slug: None,
            continuation_id: None,
        }
    }

    pub fn with_conflict_slug(mut self, slug: impl Into<String>) -> Self {
        self.conflict_slug = Some(slug.into());
        self
    }

    pub fn with_continuation_id(mut self, id: impl Into<String>) -> Self {
        self.continuation_id = Some(id.into());
        self
    }
}

/// Query parameters for listing git branches
#[derive(Debug, Deserialize)]
pub struct GitBranchesQuery {
    pub cwd: String,
    /// When present, searches remote refs via `git ls-remote` (substring match).
    /// When absent, returns local branches sorted by recency.
    pub search: Option<String>,
}

/// A single branch entry with local/remote provenance.
#[derive(Debug, Serialize)]
pub struct GitBranchEntry {
    pub name: String,
    /// true if this branch exists locally
    pub local: bool,
    /// true if a remote-tracking ref exists (e.g. `origin/<name>`)
    pub remote: bool,
    /// How many commits the local ref is behind the remote tracking ref.
    /// Only set when both local and remote exist and they diverge. 0 = up-to-date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_remote: Option<u32>,
    /// If this branch is already checked out in a worktree with an active conversation,
    /// the slug of that conversation. The UI can link to it or warn before selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_slug: Option<String>,
}

/// Response for git branch listing
#[derive(Debug, Serialize)]
pub struct GitBranchesResponse {
    pub branches: Vec<GitBranchEntry>,
    pub current: String,
    /// The remote's default branch (e.g. "main"), if detectable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
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
