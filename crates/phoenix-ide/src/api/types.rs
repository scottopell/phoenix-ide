//! API request and response types

pub use phoenix_core::domain::pr_feedback_status::PrFeedbackStatus;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Request to create a new conversation with initial message
#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    #[serde(default)]
    pub conversation_id: Option<String>,
    pub cwd: String,
    pub model: Option<String>,
    /// Initial message text (required)
    pub text: String,
    /// Client-generated message ID for idempotency
    pub message_id: String,
    /// Optional image attachments
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
    /// Non-image files already stored by the server-side attachment pipeline.
    #[serde(default)]
    pub files: Vec<FileAttachment>,
    /// Conversation mode: "managed" for Explore/Work lifecycle, omit or "direct" for full access.
    /// "managed" requires a git repository.
    #[serde(default)]
    pub mode: Option<String>,
    /// Desired base branch for Managed mode. If None, uses currently checked-out branch.
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub checkout_ref: Option<String>,
    /// Seed parent conversation id (REQ-SEED-003). Decorative link only; the
    /// spawned conversation runs independently.
    #[serde(default)]
    pub seed_parent_id: Option<String>,
    /// Seed label (REQ-SEED-004). Short human-readable context string shown in
    /// the seeded conversation's breadcrumb.
    #[serde(default)]
    pub seed_label: Option<String>,
}

/// Request body for one-shot shell-command suggestion (`POST /api/suggest`).
/// Stateless: no conversation, no tools, no persistence.
#[derive(Debug, Deserialize)]
pub struct SuggestRequest {
    /// Natural-language description of what the user wants to do.
    pub query: String,
    /// Optional model id; defaults to a cheap/fast model when omitted.
    #[serde(default)]
    pub model: Option<String>,
}

/// Response for `POST /api/suggest`: the suggested command lines in model order.
#[derive(Debug, Serialize)]
pub struct SuggestResponse {
    pub commands: Vec<String>,
}

/// Request body for persisted desktop notification preferences (REQ-NOTIF-006/009).
#[derive(Debug, Deserialize)]
pub struct NotificationSettingsRequest {
    pub enabled: crate::db::NotificationToggle,
    #[serde(flatten)]
    pub events: crate::db::NotificationEventSettings,
}

impl From<NotificationSettingsRequest> for crate::db::NotificationSettings {
    fn from(value: NotificationSettingsRequest) -> Self {
        Self {
            enabled: value.enabled,
            events: value.events,
        }
    }
}

/// Request to upgrade a conversation's model
#[derive(Debug, Deserialize)]
pub struct UpgradeModelRequest {
    /// Target model ID (e.g., "claude-opus-4-7").
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
    #[serde(default)]
    pub files: Vec<FileAttachment>,
    /// Browser user agent for display (e.g., show iPhone icon)
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// Metadata for a non-image file attachment already written to server temp storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAttachment {
    pub original_name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub stored_path: String,
}

impl From<FileAttachment> for phoenix_core::domain::db_schema::FileAttachment {
    fn from(value: FileAttachment) -> Self {
        Self {
            original_name: value.original_name,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
            stored_path: value.stored_path,
        }
    }
}

/// Response for attachment upload.
#[derive(Debug, Serialize)]
pub struct AttachmentUploadResponse {
    pub files: Vec<FileAttachment>,
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

#[derive(Debug, Serialize)]
pub struct ConversationMetaResponse {
    pub conversation: serde_json::Value,
    pub agent_working: bool,
    /// Presentation mode: `idle`, `working`, `needs_action`, `error`, `done`
    pub presentation_mode: String,
    pub context_window_size: u64,
}

/// Response with conversation and messages
#[derive(Debug, Serialize)]
pub struct ConversationWithMessagesResponse {
    pub conversation: serde_json::Value,
    pub messages: Vec<crate::api::wire::EnrichedMessage>,
    pub agent_working: bool,
    /// Presentation mode: `idle`, `working`, `needs_action`, `error`, `done`
    pub presentation_mode: String,
    pub context_window_size: u64,
}

/// Response for message history slice endpoints.
#[derive(Debug, Serialize)]
pub struct ConversationMessageSliceResponse {
    pub messages: Vec<crate::api::wire::EnrichedMessage>,
    /// Current server does not persist message tombstones yet, so slice
    /// endpoints return an empty list until that model exists.
    pub tombstones: Vec<serde_json::Value>,
    pub transcript_generation: Option<i64>,
    pub server_message_tail: Option<i64>,
    pub has_older_messages: bool,
}

/// Response for exact inclusive range fetches.
#[derive(Debug, Serialize)]
pub struct ConversationMessageRangeResponse {
    pub messages: Vec<crate::api::wire::EnrichedMessage>,
    /// Sequence ids in the requested inclusive range that are absent from the DB.
    /// The server currently has no persisted tombstone model, so holes are
    /// surfaced explicitly instead of being silently omitted.
    pub missing_sequences: Vec<i64>,
    pub tombstones: Vec<serde_json::Value>,
    pub transcript_generation: Option<i64>,
    pub server_message_tail: Option<i64>,
}

/// Response for around-a-sequence fetches.
///
/// The `before` and `after` slices exclude the pivot sequence from the route
/// (`/around/:sequence`). Clients that need the pivot message should fetch it
/// from already-loaded transcript state or an exact range that includes it.
#[derive(Debug, Serialize)]
pub struct ConversationMessagesAroundResponse {
    pub before: Vec<crate::api::wire::EnrichedMessage>,
    pub after: Vec<crate::api::wire::EnrichedMessage>,
    /// Current server does not persist message tombstones yet, so slice
    /// endpoints return an empty list until that model exists.
    pub tombstones: Vec<serde_json::Value>,
    pub transcript_generation: Option<i64>,
    pub server_message_tail: Option<i64>,
}

/// Response for chat action
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub queued: bool,
    /// Present and `true` when the message was accepted as a steering message
    /// (the conversation was busy and the message was queued for later delivery).
    /// Absent (`null`/`undefined` on the client) for normal immediate processing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub steering: bool,
}

#[derive(Debug, Deserialize)]
pub struct AddressPrFeedbackRequest {
    /// Optional caller-provided idempotency key. Browser callers should provide
    /// one so a retried one-shot Address feedback request cannot double-submit.
    #[serde(default)]
    pub message_id: Option<String>,
    /// Browser user agent for display when the request originated from the UI.
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AddressPrFeedbackResponse {
    pub queued: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub steering: bool,
    pub artifact_path: String,
    pub pr_number: u64,
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

/// Refines a text file for icon and syntax-highlight selection. These are
/// distinctions the viewer's openability dispatch collapses (all are openable
/// text) but the renderer still needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum TextCategory {
    Markdown,
    Code,
    Config,
    Plain,
    /// Extension unrecognized — treated as text; the read path confirms by
    /// content-sniffing and downgrades to an error if the bytes are binary.
    Unknown,
}

/// How the file viewer treats a path — the single authority on "can this be
/// opened, and as what." [`ReadFileResponse`] dispatches on exactly this, and
/// the listing/search endpoints carry it so every entry point (sidebar,
/// quick-open, @-mention, linkified path) shares one verdict instead of
/// re-deriving openability from extensions. The variant structure encodes
/// openability directly: only [`FileViewerKind::Opaque`] is non-openable, so
/// there is no separate boolean to drift out of sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum FileViewerKind {
    /// Opens in the text/prose reader; `category` refines icon and highlighting.
    Text { category: TextCategory },
    /// Opens as an image preview.
    Image,
    /// Binary or otherwise unsupported — the viewer cannot open it. Also the
    /// kind reported for directories (which are entered, not opened).
    Opaque,
}

impl FileViewerKind {
    /// Classify a path by extension — the single source of truth for "how does
    /// the viewer treat this file." The directory listing carries this as an
    /// affordance prediction; `/api/files/read` dispatches on the same verdict
    /// and additionally content-sniffs [`TextCategory::Unknown`] files before
    /// committing to text. Works equally on filesystem paths and on git-tree
    /// path strings (extension-only, no I/O), so every callsite shares it.
    pub fn for_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);

        let text = |category| FileViewerKind::Text { category };

        match ext.as_deref() {
            Some("md" | "markdown") => text(TextCategory::Markdown),
            Some(
                "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "cpp" | "c" | "h"
                | "hpp" | "css" | "html" | "htm" | "vue" | "svelte" | "php" | "rb" | "swift" | "kt"
                | "scala" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "sql" | "graphql" | "proto",
            ) => text(TextCategory::Code),
            Some(
                "json" | "yaml" | "yml" | "toml" | "ini" | "env" | "conf" | "cfg" | "xml"
                | "properties",
            ) => text(TextCategory::Config),
            Some("txt" | "log" | "csv" | "tsv" | "rtf") => text(TextCategory::Plain),
            Some(
                "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "tiff" | "tif",
            ) => FileViewerKind::Image,
            // Known-binary extensions: the viewer cannot open them.
            Some(
                "db" | "sqlite" | "sqlite3" | "bin" | "dat" | "exe" | "dll" | "so" | "dylib" | "o"
                | "a" | "wasm" | "class" | "jar" | "war" | "pyc" | "pyo" | "pdf" | "doc" | "docx"
                | "xls" | "xlsx" | "ppt" | "pptx" | "zip" | "tar" | "gz" | "bz2" | "xz" | "7z"
                | "rar" | "mp3" | "mp4" | "wav" | "avi" | "mkv" | "mov" | "webm" | "flac" | "ogg"
                | "woff" | "woff2" | "ttf" | "otf",
            ) => FileViewerKind::Opaque,
            // Unknown extension: optimistically text; the read path confirms by
            // content-sniffing before committing.
            _ => text(TextCategory::Unknown),
        }
    }
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
    /// How the viewer treats this entry. Directories report
    /// [`FileViewerKind::Opaque`]; `is_directory` distinguishes "enter" from
    /// "cannot open."
    pub viewer: FileViewerKind,
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
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadFileResponse {
    Text {
        content: String,
        encoding: String,
        category: TextCategory,
    },
    Image {
        mime_type: String,
        url: String,
    },
}

/// Error response for file operations
#[derive(Debug, Serialize)]
#[allow(dead_code)] // Reserved for future use
pub struct FileErrorResponse {
    pub error: String,
    pub is_binary: bool,
}

// Per-model metadata is owned by the LLM layer (it is built from a ModelSpec +
// the live service's effective context window). Re-exported here so the
// `/api/models` response type and its `crate::api::ModelInfo` consumers resolve
// against the same struct the registry produces.
pub use phoenix_llm::ModelInfo;

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
    /// True when at least one LLM provider is configured.
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
    /// How the viewer treats this path — the same verdict the listing endpoint
    /// and `/api/files/read` use.
    pub viewer: FileViewerKind,
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

/// Query parameters for directory-scoped file search — used by the composer on
/// the new-conversation page, where no conversation (and thus no file root)
/// exists yet. Same fuzzy semantics as [`FileSearchQuery`] plus an explicit
/// working directory to walk.
#[derive(Debug, Deserialize)]
pub struct ProjectFileSearchQuery {
    /// Working directory to search within.
    pub cwd: String,
    /// Fuzzy query string (empty = return all up to limit)
    #[serde(default)]
    pub q: String,
    /// Maximum number of results (default 50)
    pub limit: Option<usize>,
    /// Resolved creation mode (`direct`/`managed`/`branch`). Together with
    /// `base_branch` this selects the resolution root: branch/managed modes
    /// search the chosen branch's committed tree (what the conversation's fresh
    /// worktree will hold), so suggestions match create-time expansion.
    /// Absent ⇒ Direct (search `cwd`).
    pub mode: Option<String>,
    /// Branch the conversation will be created on, for branch/managed modes.
    pub base_branch: Option<String>,
}

/// A single code content search result.
#[derive(Debug, Serialize)]
pub struct CodeSearchEntry {
    /// Path relative to the conversation's file root
    pub path: String,
    /// 1-based line number of the match
    pub line_number: usize,
    /// Full text of the matched line, without the trailing newline
    pub line_text: String,
    /// 0-based character offset where the match starts within `line_text`
    pub match_start: usize,
    /// 0-based character offset where the match ends within `line_text`
    pub match_end: usize,
}

/// Response for conversation-scoped code content search.
#[derive(Debug, Serialize)]
pub struct CodeSearchResponse {
    pub items: Vec<CodeSearchEntry>,
}

/// Query parameters for code content search.
#[derive(Debug, Deserialize)]
pub struct CodeSearchQuery {
    /// Literal substring query (empty = no results)
    #[serde(default)]
    pub q: String,
    /// Maximum number of results (default 50, capped server-side)
    pub limit: Option<usize>,
}

/// A single skill entry returned by the skills API (REQ-IR-005, REQ-BS-003)
#[derive(Debug, Serialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Where this skill was discovered. Either a discovery directory like
    /// `".claude/skills"` / `".agents/skills"` for filesystem skills, or the
    /// literal `"builtin"` for skills bundled with the phoenix binary
    /// (extracted to `<HOME>/.phoenix-ide/builtin-skills/` at startup).
    pub source: String,
    /// Absolute path to the SKILL.md file. Always populated; built-in skills
    /// point at the extracted location.
    pub path: String,
}

/// Response for the skills list endpoint (REQ-IR-005)
#[derive(Debug, Serialize)]
pub struct SkillsResponse {
    pub skills: Vec<SkillEntry>,
}

/// Query parameters for directory-scoped skill discovery — used by the composer
/// on the new-conversation page, before a conversation exists (REQ-IR-005).
#[derive(Debug, Deserialize)]
pub struct ProjectSkillsQuery {
    /// Working directory to discover skills from.
    pub cwd: String,
    /// Resolved creation mode (`direct`/`managed`/`branch`). With `base_branch`,
    /// branch/managed modes discover skills from the chosen branch's committed
    /// `.claude/skills` / `.agents/skills` (plus global + built-in), matching
    /// what the conversation's worktree will expose. Absent ⇒ Direct (`cwd`).
    pub mode: Option<String>,
    /// Branch the conversation will be created on, for branch/managed modes.
    pub base_branch: Option<String>,
}

/// A task file entry returned by the tasks list endpoint.
#[derive(Debug, Serialize)]
pub struct TaskEntry {
    pub id: String,
    pub priority: String,
    pub status: String,
    pub slug: String,
    /// Absolute path to the task file on disk when the task exists in the current checkout.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Slug of the conversation working on this task (if any active Work conversation owns it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_slug: Option<String>,
}

#[allow(dead_code)]
/// Response for the tasks list endpoint.
#[derive(Debug, Serialize)]
pub struct TasksResponse {
    pub tasks: Vec<TaskEntry>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct TaskAvailabilityResponse {
    pub available: bool,
}

/// Query parameters for listing project task files before a conversation exists.
#[derive(Debug, Deserialize)]
pub struct ProjectTasksQuery {
    pub cwd: String,
}

/// Lightweight status counts for the Tasks panel's collapsed header. Lets the
/// header carry its count summary without fetching the full task list (and its
/// per-conversation slug mapping) on every conversation mount — the full list
/// is fetched only on expand.
#[derive(Debug, Serialize)]
pub struct TaskCountResponse {
    /// Tasks in a non-terminal status (anything but `done`/`wont-do`).
    pub active: u32,
    /// Tasks in a terminal status (`done`/`wont-do`).
    pub closed: u32,
    /// Tasks in the `blocked` status.
    pub blocked: u32,
    /// Whether `current_task_id` (the branch-derived task the UI considers
    /// current) is present among the listed tasks.
    pub current: bool,
}

/// Query parameters for the task-count endpoint.
#[derive(Debug, Deserialize)]
pub struct TaskCountQuery {
    /// The task the caller considers "current" (derived UI-side from the branch
    /// name), so the collapsed header's "current set" indicator matches the
    /// expanded list's. Absent ⇒ no current task.
    #[serde(default)]
    pub current_task_id: Option<String>,
}

/// Expansion error detail returned to the frontend (REQ-IR-007)
#[derive(Debug, Clone, Serialize)]
pub struct ExpansionErrorResponse {
    pub error: String,
    pub error_type: String,
    /// The reference token that caused the failure (e.g. `@src/missing.rs` or `/skill-name`)
    pub reference: String,
}

/// Request to approve a proposed task plan.
#[derive(Debug, Deserialize, Default)]
pub struct TaskApprovalRequest {
    #[serde(default)]
    pub handoff: crate::state_machine::state::TaskApprovalHandoff,
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

// ============================================================
// Fork proposal resolution (REQ-PROJ-034 / 037)
//
// Plain `Serialize` (no `ts_rs::TS`), matching every other API response type in
// this module. The UI chunk that consumes these can add codegen derives if it
// adopts the typed-SSE pattern; that decision is left to that chunk.
// ============================================================

/// Body for `POST /proposals/:proposal_id/request-changes` — the user's
/// free-text change request (REQ-PROJ-037).
#[derive(Debug, Deserialize)]
pub struct RequestChangesRequest {
    pub note: String,
}

/// Response for approving a fork proposal (REQ-PROJ-034).
#[derive(Debug, Serialize)]
pub struct ForkSpawnResponse {
    pub fork_conversation_id: String,
}

/// Response for promoting a fork proposal to an Explore refinement (REQ-PROJ-037).
#[derive(Debug, Serialize)]
pub struct ForkPromoteResponse {
    pub refinement_conversation_id: String,
}

/// Response for dismissing a fork proposal (REQ-PROJ-034). `no_op` is true when
/// the proposal was already resolved (idempotent dismiss).
#[derive(Debug, Serialize)]
pub struct ForkDismissResponse {
    pub success: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_op: bool,
}

/// One fork proposal as rendered to the review surface.
#[derive(Debug, Serialize)]
pub struct ForkProposalSummary {
    pub id: String,
    pub status: String,
    pub title: String,
    pub priority: String,
    pub task_file: String,
    /// Snapshotted brief body — the snapshot is not in the transcript, so the
    /// review modal renders it from here keyed by proposal id.
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork_conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refinement_conversation_id: Option<String>,
}

/// Response listing a conversation's fork proposals.
#[derive(Debug, Serialize)]
pub struct ForkProposalListResponse {
    pub proposals: Vec<ForkProposalSummary>,
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

/// Response for `GET /api/conversations/:id/diff` — what the worktree
/// has done relative to its base. Used by the Work-mode "View diff"
/// action so users can review changes before deciding to merge or
/// abandon. Each diff section is capped at 256KiB; when the raw output
/// exceeded the cap, the `*_truncated_kib` fields hold the total size
/// in KiB so the UI can label the truncation. The `*_saturated` flags
/// indicate whether the streaming reader hit its hard limit and stopped
/// counting — when `true`, `*_truncated_kib` is a LOWER BOUND, not the
/// exact size, and consumers should render it as e.g. "≥X KiB".
#[derive(Debug, Serialize)]
pub struct ConversationDiffResponse {
    /// The ref used as the comparator — e.g. `"origin/main"` when the
    /// remote-tracking ref exists, or bare `"main"` for local-only repos.
    pub comparator: String,
    /// Human label for this diff surface (`Workspace Diff`, `PR #123 Diff`).
    pub label: String,
    /// Which diff surface this payload represents.
    pub kind: String,
    /// The active PR number for PR-specific diffs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    /// `git log --oneline <comparator>..HEAD` — commits on the branch
    /// not yet in the comparator. Subject lines only; uncapped (commit
    /// titles are tiny).
    pub commit_log: String,
    /// `git diff <comparator>...HEAD` — committed work, file-level diff
    /// to the common ancestor.
    pub committed_diff: String,
    /// Total stdout size in KiB when the diff was truncated; `None` when
    /// it fit under the cap. When `committed_saturated` is `true`, this
    /// is a lower bound — render as "≥X KiB total" in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed_truncated_kib: Option<u32>,
    /// `true` when the streaming reader hit its hard limit (8× the
    /// visible cap) and stopped counting. `committed_truncated_kib` is
    /// then a lower bound, not the exact total.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub committed_saturated: bool,
    /// `git diff HEAD` after `git add -N .` — uncommitted working-tree
    /// changes, including untracked files surfaced via intent-to-add.
    pub uncommitted_diff: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncommitted_truncated_kib: Option<u32>,
    /// Like `committed_saturated` but for the uncommitted-diff section.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uncommitted_saturated: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrUnavailableReason {
    GhMissing,
    NotAuthenticated,
    NotGitRepo,
    CommandFailed,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrCheckState {
    Passing,
    Pending,
    Failing,
    Unknown,
}

// PrDisplayState is co-owned by this api layer (serialized to the client) and
// the persisted db layer; it lives in phoenix-core. Re-export at the historical
// `crate::api::PrDisplayState` path so existing call sites resolve unchanged.
pub use phoenix_core::domain::pr_display_state::PrDisplayState;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PrCheckSummary {
    pub passing: u32,
    pub pending: u32,
    pub failing: u32,
    pub skipped: u32,
    pub unknown: u32,
    pub failing_names: Vec<String>,
    pub pending_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PrCheckDetail {
    pub name: String,
    pub state: String,
    pub bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrFeedbackSource {
    IssueComment,
    ReviewComment,
    ReviewSummary,
    ReviewThread,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrCheckLogSource {
    /// No extractable logs; `snippet` explains why and `url` points at the
    /// provider's web UI for the full logs.
    CheckUrl,
    /// Failed-step logs extracted from a GitHub Actions job via `gh run view`.
    GhActionsLog,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PrCheckLogSnippet {
    pub check_name: String,
    pub source: PrCheckLogSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub snippet: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrFeedbackCoverageSurface {
    IssueComments,
    ReviewComments,
    ReviewSummaries,
    ReviewThreads,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrFeedbackCoverageStatus {
    Fetched,
    Unavailable,
    AuthFailed,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PrFeedbackCoverage {
    pub surface: PrFeedbackCoverageSurface,
    pub status: PrFeedbackCoverageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PrFeedbackItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// GraphQL `pullRequestReviewThread` node id (`PRRT_…`). Populated only for
    /// `ReviewThread` items — it is the id `resolveReviewThread` accepts, distinct
    /// from `id` which is the per-comment node id and is rejected for resolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub source: PrFeedbackSource,
    pub author: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PrFeedbackSummary {
    pub total: u32,
    pub unresolved: u32,
    // owned: pre-reaction and failed-reaction captures have no trustworthy status
    #[serde(default)]
    pub feedback_status: Option<PrFeedbackStatus>,
    pub items: Vec<PrFeedbackItem>,
    pub coverage: Vec<PrFeedbackCoverage>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ActivePrIdentityResponse {
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivePrSelectionProvenanceResponse {
    Inferred,
    Pinned,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ActivePrSelectionResponse {
    pub pr: ActivePrIdentityResponse,
    pub provenance: ActivePrSelectionProvenanceResponse,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ObservedBranchSummaryResponse {
    pub repository_identity: String,
    pub branch_name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AssociatedPrSummaryResponse {
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub display_state: PrDisplayState,
    pub base: String,
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_updated_at: Option<String>,
    pub feedback_status: PrFeedbackStatus,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AssociatedPrStatusEnvelope {
    pub associated_prs: Vec<AssociatedPrSummaryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_pr: Option<ActivePrSelectionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_observed_branch: Option<ObservedBranchSummaryResponse>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct PinAssociatedPrRequest {
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ActivePrSelectionMutationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_pr: Option<ActivePrSelectionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_observed_branch: Option<ObservedBranchSummaryResponse>,
}

#[derive(Debug, Serialize)]
pub struct PrAutoFixContextResponse {
    pub artifact_path: String,
    pub pr_number: u64,
    pub repo_owner: String,
    pub repo_name: String,
    pub message: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrRefreshState {
    Fresh,
    Unavailable,
    NotFound,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct PrRefreshMetadata {
    pub state: PrRefreshState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PrUnavailableReason>,
    pub last_attempted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refreshed_at: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct PrIdentity {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub display_state: PrDisplayState,
    pub base: String,
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// How the PR's feedback has moved since the last captured baseline. Each
/// variant carries its own count, so a count is never absent and "new" can
/// never be confused with "edited". Serializes with an internal `state` tag:
/// `{ "state": "new", "count": 3 }` / `{ "state": "edited", "count": 1 }`.
///
/// Coverage gaps and fetch failures are deliberately *not* represented here —
/// they are an error condition, not a content change, and surface separately.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PrFeedbackFreshness {
    /// `count` feedback items are present that were not in the baseline.
    New { count: u32 },
    /// `count` baseline feedback items changed actionable content with no
    /// net-new actionable items.
    Edited { count: u32 },
}

/// Health of the feedback fetch that produced a freshness signal. Orthogonal to
/// [`PrFeedbackFreshness`]: when a surface can't be read, any freshness count is
/// only a lower bound (the UI renders "at least N"). Absent when every surface
/// was fetched. Auth precedes transient unavailability because only the former
/// is user-actionable. Serializes with an internal `kind` tag.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrFeedbackCoverageHealth {
    /// At least one surface couldn't be read because the GitHub CLI is not
    /// authenticated — the user can fix this (`gh auth login`).
    AuthRequired {
        surfaces: Vec<PrFeedbackCoverageSurface>,
    },
    /// At least one surface was transiently unavailable (no auth problem);
    /// feedback may be incomplete.
    Incomplete {
        surfaces: Vec<PrFeedbackCoverageSurface>,
    },
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkChangeSummary {
    Clean,
    DirtyPrReady {
        create_pr_url: String,
        branch_name: String,
        base_branch: String,
    },
    DirtyNeedsReview {
        reason: WorkChangeNeedsReviewReason,
    },
    Loading,
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkChangeNeedsReviewReason {
    UncommittedChanges,
    BranchNotPushed,
    LocalAheadOfRemote,
    RemoteDiverged,
    NonGithubRemote,
    UnknownRemote,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct PrStatusResponse {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrIdentity>,
    #[serde(flatten)]
    pub selection: AssociatedPrStatusEnvelope,
    pub refresh: PrRefreshMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<PrUnavailableReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_state: Option<PrCheckState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_summary: Option<PrCheckSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_summary: Option<PrFeedbackSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_state: Option<PrDisplayState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_status: Option<PrFeedbackStatus>,
    pub feedback_freshness: Option<PrFeedbackFreshness>,
    /// Degraded coverage of the feedback fetch, if any — distinct from
    /// `feedback_freshness` so an incomplete fetch is never mistaken for a
    /// content change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_coverage: Option<PrFeedbackCoverageHealth>,
    /// Coarse state of branch/worktree changes, used by the `WorkActions` bar to
    /// avoid terminal cleanup affordances for dirty no-PR work. Always present:
    /// unavailable/loading are explicit states, not omitted fields.
    pub work_change: WorkChangeSummary,
}

impl PrStatusResponse {
    pub fn not_found() -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            found: false,
            pr: None,
            selection: AssociatedPrStatusEnvelope {
                associated_prs: Vec::new(),
                active_pr: None,
                latest_observed_branch: None,
            },
            refresh: PrRefreshMetadata {
                state: PrRefreshState::NotFound,
                reason: None,
                last_attempted_at: now.clone(),
                last_refreshed_at: Some(now),
                stale: false,
            },
            unavailable_reason: None,
            number: None,
            title: None,
            url: None,
            state: None,
            draft: None,
            base: None,
            head: None,
            check_state: None,
            check_summary: None,
            feedback_summary: None,
            updated_at: None,
            display_state: None,
            feedback_status: None,
            feedback_freshness: None,
            feedback_coverage: None,
            work_change: WorkChangeSummary::Loading,
        }
    }

    pub fn unavailable(reason: PrUnavailableReason) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            refresh: PrRefreshMetadata {
                state: PrRefreshState::Unavailable,
                reason: Some(reason.clone()),
                last_attempted_at: now,
                last_refreshed_at: None,
                stale: false,
            },
            unavailable_reason: Some(reason),
            ..Self::not_found()
        }
    }
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            error_type: None,
        }
    }

    pub fn typed(message: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            error_type: Some(error_type.into()),
        }
    }
}
