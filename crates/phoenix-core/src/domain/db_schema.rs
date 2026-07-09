//! Database schema and types

use crate::domain::llm_types::ContentBlock;
use crate::domain::retry_policy::{AutoRetryPolicy, UserResumePolicy};
pub use crate::domain::sm_state::ConvState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NotificationToggle {
    Enabled,
    Disabled,
}

impl NotificationToggle {
    #[must_use]
    pub fn as_bool(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for NotificationToggle {
    fn from(value: bool) -> Self {
        if value {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

impl Serialize for NotificationToggle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bool(self.as_bool())
    }
}

impl<'de> Deserialize<'de> for NotificationToggle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        bool::deserialize(deserializer).map(Self::from)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationEventSettings {
    #[serde(rename = "notify_task_approval")]
    pub task_approval: NotificationToggle,
    #[serde(rename = "notify_question")]
    pub question: NotificationToggle,
    #[serde(rename = "notify_error")]
    pub error: NotificationToggle,
    #[serde(rename = "notify_idle")]
    pub idle: NotificationToggle,
}

impl Default for NotificationEventSettings {
    fn default() -> Self {
        Self {
            task_approval: NotificationToggle::Enabled,
            question: NotificationToggle::Enabled,
            error: NotificationToggle::Enabled,
            idle: NotificationToggle::Enabled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationSettings {
    pub enabled: NotificationToggle,
    #[serde(flatten)]
    pub events: NotificationEventSettings,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: NotificationToggle::Enabled,
            events: NotificationEventSettings::default(),
        }
    }
}

/// A string guaranteed to be non-empty at construction time.
/// Serde deserialization rejects empty strings.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// # Errors
    /// Returns `Err` if the input string is empty.
    pub fn new(s: impl Into<String>) -> Result<Self, &'static str> {
        let s = s.into();
        if s.is_empty() {
            Err("string must not be empty")
        } else {
            Ok(Self(s))
        }
    }

    #[must_use]
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

/// Conversation mode — determines tool availability and write access.
///
/// Stored as JSON in the `conv_mode` TEXT column on conversations.
/// REQ-BED-027: Conversation-level field, NOT embedded in `ConvState`.
///
/// All string fields in Work and Branch use `NonEmptyString` to make empty
/// strings structurally unrepresentable. The migration system backfills
/// legacy rows; deserialization of missing fields is now a hard error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode")]
pub enum ConvMode {
    /// Read-only mode. No file writes, no bash (unless sandboxed).
    /// Opt-in "Managed" workflow: `propose_task` available, gateway to Work.
    ///
    /// `worktree_path` is `Some` for top-level managed Explore conversations
    /// (the conv runs in a dedicated worktree pre-approval) and `None` for
    /// sub-agent Explore conversations (which share the parent's working
    /// directory and have no worktree of their own). Migration 007 backfills
    /// the field for legacy top-level Explore rows.
    Explore {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<NonEmptyString>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_taskmd_id_hint: Option<NonEmptyString>,
    },
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
        /// Used as the diff comparator (View diff, abandon snapshot) and the
        /// PR base the agent is told to target.
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

impl Default for ConvMode {
    fn default() -> Self {
        Self::Explore {
            worktree_path: None,
            next_taskmd_id_hint: None,
        }
    }
}

impl ConvMode {
    /// Human-readable label for UI display
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Explore { .. } => "Explore",
            Self::Direct => "Direct",
            Self::Work { .. } => "Work",
            Self::Branch { .. } => "Branch",
        }
    }

    /// The branch name if in Work or Branch mode, None otherwise.
    #[must_use]
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Self::Work { branch_name, .. } | Self::Branch { branch_name, .. } => {
                Some(branch_name.as_str())
            }
            Self::Explore { .. } | Self::Direct => None,
        }
    }

    /// The worktree path for this mode, if any.
    ///
    /// - Work/Branch always have one (typed `NonEmptyString`).
    /// - Top-level managed Explore has one (typed `Option<NonEmptyString>`,
    ///   `Some` for managed-flow rows, `None` for sub-agent Explore).
    /// - Direct never has one.
    pub fn worktree_path(&self) -> Option<&str> {
        match self {
            Self::Work { worktree_path, .. } | Self::Branch { worktree_path, .. } => {
                Some(worktree_path.as_str())
            }
            Self::Explore { worktree_path, .. } => {
                worktree_path.as_ref().map(NonEmptyString::as_str)
            }
            Self::Direct => None,
        }
    }

    /// The base branch if in Work or Branch mode, None otherwise.
    #[must_use]
    pub fn base_branch(&self) -> Option<&str> {
        match self {
            Self::Work { base_branch, .. } | Self::Branch { base_branch, .. } => {
                Some(base_branch.as_str())
            }
            Self::Explore { .. } | Self::Direct => None,
        }
    }

    /// The task ID if in Work mode, None otherwise. Branch mode has no task.
    #[must_use]
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Self::Work { task_id, .. } => Some(task_id.as_str()),
            Self::Explore { .. } | Self::Direct | Self::Branch { .. } => None,
        }
    }

    /// The task title if in Work mode, None otherwise. Branch mode has no task.
    #[must_use]
    pub fn task_title(&self) -> Option<&str> {
        match self {
            Self::Work { task_title, .. } => Some(task_title.as_str()),
            Self::Explore { .. } | Self::Direct | Self::Branch { .. } => None,
        }
    }

    /// Extract `WorktreeConfig` from Work or Branch mode. Returns None for
    /// Explore and Direct.
    #[allow(dead_code)] // Introduced for A2/C1 phases; tested in conv_mode_tests
    #[must_use]
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
            Self::Explore { .. } | Self::Direct => None,
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

fn default_transcript_generation() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationCreationPhase {
    Accepted,
    Provisioning,
    Ready,
    Failed,
}

impl ConversationCreationPhase {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Provisioning => "provisioning",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn from_db_str(value: &str) -> Option<Self> {
        Some(match value {
            "accepted" => Self::Accepted,
            "provisioning" => Self::Provisioning,
            "ready" => Self::Ready,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationCreationIntent {
    pub cwd: String,
    #[serde(default)]
    pub model: Option<String>,
    pub text: String,
    // owned: pre-feature rows had no accepted expansion snapshot; None correctly re-expands.
    #[serde(default)]
    pub llm_text: Option<String>,
    // owned: pre-feature rows had no accepted skill invocation snapshot; None correctly re-expands.
    #[serde(default)]
    pub skill_invocation: Option<crate::domain::skill_invocation::SkillInvocation>,
    pub message_id: String,
    #[serde(default)]
    pub images: Vec<ImageData>,
    #[serde(default)]
    pub files: Vec<FileAttachment>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub checkout_ref: Option<String>,
    #[serde(default)]
    pub seed_parent_id: Option<String>,
    #[serde(default)]
    pub seed_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationCreationJob {
    pub id: String,
    pub conversation_id: String,
    #[serde(default)]
    pub message_id: Option<String>,
    pub phase: ConversationCreationPhase,
    pub intent: ConversationCreationIntent,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub accepted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub provisioning_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub failed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub error: Option<String>,
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
    /// Conversation-level transcript/replica generation. Bumped when the
    /// server invalidates previously cached incremental transcript state so
    /// clients can discard stale ranges/patches and rebuild from a fresh
    /// snapshot.
    #[serde(default = "default_transcript_generation")]
    pub transcript_generation: i64,
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
    /// User-set name for the chain rooted at this conversation
    /// (REQ-CHN-007). Only meaningful on the root of a chain; ignored at
    /// read time on non-root members. NULL means "use this conversation's
    /// title as the displayed chain name." `#[serde(default)]` handles old
    /// DB rows that predate this column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_name: Option<String>,
    /// LLM-facing prose language fixed at creation (e.g. `phoenix-native`,
    /// `caveman`). Chain continuations and sub-agents inherit it.
    /// `#[serde(default)]` handles old DB rows that predate this column.
    #[serde(default)]
    pub llm_language: crate::llm_language::LlmLanguage,
    /// Provenance breadcrumb for a decoupled task fork (REQ-PROJ-035): the
    /// originating conversation that proposed the fork. A raw, non-FK id that
    /// may dangle — distinct from `parent_conversation_id` (sub-agent parentage)
    /// and `continued_in_conv_id` (chain continuation), and never traversed by
    /// runtime logic. `#[serde(default)]` handles old DB rows that predate this
    /// column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_from_conversation_id: Option<String>,
}

/// Derive a human-readable title from a kebab-case slug.
/// E.g., "my-test-conversation" -> "My Test Conversation"
#[must_use]
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

/// Derive a kebab-case slug from a human-readable title — the inverse of
/// [`title_from_slug`]. E.g., "Fix Login Bug!" -> "fix-login-bug".
/// Any run of non-alphanumeric characters becomes a single hyphen. Returns
/// an empty string when the title has no slug-able characters; callers
/// supply their own fallback.
#[must_use]
pub fn slug_from_title(title: &str) -> String {
    title
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
}

#[cfg(test)]
mod slug_title_tests {
    use super::{slug_from_title, title_from_slug};

    #[test]
    fn slug_from_title_kebabs_and_lowercases() {
        assert_eq!(slug_from_title("Fix Login Bug!"), "fix-login-bug");
        assert_eq!(slug_from_title("Approve Fresh"), "approve-fresh");
        assert_eq!(
            slug_from_title("  Refactor   Auth Layer  "),
            "refactor-auth-layer"
        );
    }

    #[test]
    fn slug_from_title_empty_when_no_slugable_chars() {
        assert_eq!(slug_from_title("!!! ---"), "");
        assert_eq!(slug_from_title(""), "");
    }

    #[test]
    fn slug_and_title_round_trip_for_simple_titles() {
        // A clean lowercase prose title survives the slug -> title round trip
        // (title-casing is the only transformation).
        assert_eq!(
            title_from_slug(&slug_from_title("fix login bug")),
            "Fix Login Bug"
        );
    }
}

impl Conversation {
    /// Check if the agent is currently working (derived from `display_state`)
    #[must_use]
    pub fn is_agent_working(&self) -> bool {
        self.state.display_state() == crate::domain::sm_state::DisplayState::Working
    }

    #[must_use]
    pub fn file_root(&self) -> &str {
        self.conv_mode.worktree_path().unwrap_or(&self.cwd)
    }
}

/// Resolution status of a fork proposal (REQ-PROJ-034/037).
///
/// Stored as a `snake_case` TEXT column; the enum is the authoritative shape on
/// the Rust side. `pending` is the only non-terminal state; the three resolved
/// states are terminal and single-use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkProposalStatus {
    Pending,
    Spawned,
    Dismissed,
    Promoted,
}

impl ForkProposalStatus {
    /// Persisted (`snake_case`) string representation. Stable across releases —
    /// changing this breaks rows in the `fork_proposals` table.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Spawned => "spawned",
            Self::Dismissed => "dismissed",
            Self::Promoted => "promoted",
        }
    }

    /// Parse the persisted string back into the enum. Returns `None` for
    /// unknown values so callers surface a typed error rather than silently
    /// defaulting.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "spawned" => Self::Spawned,
            "dismissed" => Self::Dismissed,
            "promoted" => Self::Promoted,
            _ => return None,
        })
    }
}

/// A decoupled task fork proposal (REQ-PROJ-033/034/035/037).
///
/// Control-plane state bound to the originating conversation: a content snapshot
/// of a brief the origin agent shed via `propose_task`, addressed by its `id`.
/// `fork_conversation_id` and `refinement_conversation_id` are raw, non-FK ids
/// that may dangle — the spawned fork / promoted refinement has an independent
/// lifecycle and may be hard-deleted while this origin-bound proposal lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkProposal {
    pub id: String,
    pub origin_conversation_id: String,
    /// The drafted file's path, normalized to repository-relative at capture.
    pub task_file: String,
    pub title: String,
    pub priority: String,
    /// Snapshotted file bytes — authoritative at spawn time.
    pub body: String,
    pub status: ForkProposalStatus,
    /// Raw id of the spawned conversation, present iff `status == Spawned`.
    pub fork_conversation_id: Option<String>,
    /// Raw id of the Explore refinement conversation, present iff
    /// `status == Promoted`.
    pub refinement_conversation_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Error classification for UI display.
///
/// No `Unknown` variant. Every error gets an explicit, intentional classification.
/// Adding a new error class requires handling it in every consumer — the compiler
/// forces it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ErrorKind {
    /// Authentication failed (401, 403) - not retryable
    Auth,
    /// Transient rate-limit throttle (429) - retryable with backoff
    RateLimit,
    /// Quota window exhausted (plan-level cap, depleted credits, etc.) - not retryable
    UsageLimitReached,
    /// Network issues, connection failures - retryable
    Network,
    /// Bad request (400) - not retryable
    InvalidRequest,
    /// Provider returned bytes we could not parse or understand (malformed SSE
    /// event, unparseable body, unexpected content-block shape) - retryable
    InvalidResponse,
    /// Server error (5xx) - retryable
    ServerError,
    /// Selected model is at capacity (`server_is_overloaded` / `slow_down`) - not retryable
    ServerOverloaded,
    /// Request timed out - retryable
    TimedOut,
    /// Operation was cancelled - not retryable
    Cancelled,
    /// Sub-agent failed - not retryable
    SubAgentError,
    /// Context window exhausted - not retryable
    ContextExhausted,
    /// Sub-agent exhausted its grace turn without a terminal outcome - not retryable
    TurnLimitExhausted,
    /// Content filter or safety block - not retryable
    ContentFilter,
}

impl ErrorKind {
    /// Policy for runtime-initiated retry while the same turn is still in flight.
    #[must_use]
    pub fn auto_retry_policy(&self) -> AutoRetryPolicy {
        match self {
            Self::Network
            | Self::RateLimit
            | Self::ServerError
            | Self::InvalidResponse
            | Self::TimedOut => AutoRetryPolicy::AutoRetryable,
            Self::Auth
            | Self::UsageLimitReached
            | Self::ServerOverloaded
            | Self::InvalidRequest
            | Self::Cancelled
            | Self::SubAgentError
            | Self::ContextExhausted
            | Self::TurnLimitExhausted
            | Self::ContentFilter => AutoRetryPolicy::NoAutoRetry,
        }
    }

    #[must_use]
    pub fn is_auto_retryable(&self) -> bool {
        self.auto_retry_policy().allows_auto_retry()
    }

    /// Policy for a persisted error state accepting a user-triggered `continue`.
    #[must_use]
    pub fn user_resume_policy(&self) -> UserResumePolicy {
        match self {
            // A usage-limit window resets on a clock boundary ("try again at
            // 1:01 AM"). Like `ServerOverloaded`, the user can resume once the
            // window clears, so it is user-resumable even though it is never
            // *auto*-retried (no point hammering a reset-on-clock quota).
            Self::Auth
            | Self::RateLimit
            | Self::Network
            | Self::ServerError
            | Self::InvalidResponse
            | Self::ServerOverloaded
            | Self::UsageLimitReached
            | Self::TimedOut => UserResumePolicy::Resumable,
            Self::InvalidRequest
            | Self::Cancelled
            | Self::SubAgentError
            | Self::ContextExhausted
            | Self::TurnLimitExhausted
            | Self::ContentFilter => UserResumePolicy::NotResumable,
        }
    }

    #[must_use]
    pub fn is_user_resumable(&self) -> bool {
        self.user_resume_policy().allows_user_resume()
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
        // serde(default): owned backward-compat decision, not a pending-migration
        // shim (task 13023). Tool-result rows in `messages.content` written
        // before the `images` feature existed genuinely carried no images, so an
        // absent key correctly deserialises to an empty vec — there is no data to
        // backfill and no migration is owed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ToolContentImage>,
    },
    Error {
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_data: Option<serde_json::Value>,
        // serde(default): see `ToolOutcome::Success::images` — owned
        // backward-compat decision for pre-images rows (task 13023).
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
    /// Wall-clock duration of the tool execution in milliseconds.
    /// `None` for tool results that were not produced by `dispatch_tool_execution`
    /// (e.g. sub-agent summaries, `spawn_agents` placeholders).
    /// Stored here so the executor can inject it into the persisted message's
    /// `display_data` without a separate wire event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ToolResult {
    #[must_use]
    pub fn success(tool_use_id: String, output: String) -> Self {
        Self {
            tool_use_id,
            outcome: ToolOutcome::Success {
                output,
                display_data: None,
                images: vec![],
            },
            duration_ms: None,
        }
    }

    #[must_use]
    pub fn error(tool_use_id: String, error: String) -> Self {
        Self {
            tool_use_id,
            outcome: ToolOutcome::Error {
                output: error,
                display_data: None,
                images: vec![],
            },
            duration_ms: None,
        }
    }

    #[must_use]
    pub fn cancelled(tool_use_id: String, message: &str) -> Self {
        Self {
            tool_use_id,
            outcome: ToolOutcome::Cancelled {
                message: message.to_string(),
            },
            duration_ms: None,
        }
    }

    /// Create a successful result with display data for UI rendering
    #[allow(dead_code)]
    #[must_use]
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
            duration_ms: None,
        }
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self.outcome, ToolOutcome::Error { .. })
    }

    #[allow(dead_code)] // Used in tests; main code uses is_error()
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self.outcome, ToolOutcome::Success { .. })
    }

    #[must_use]
    pub fn output(&self) -> &str {
        match &self.outcome {
            ToolOutcome::Success { output, .. } | ToolOutcome::Error { output, .. } => output,
            ToolOutcome::Cancelled { message } => message,
        }
    }

    #[must_use]
    pub fn display_data(&self) -> Option<&serde_json::Value> {
        match &self.outcome {
            ToolOutcome::Success { display_data, .. } | ToolOutcome::Error { display_data, .. } => {
                display_data.as_ref()
            }
            ToolOutcome::Cancelled { .. } => None,
        }
    }

    #[must_use]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileAttachment>,
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
            files: Vec::new(),
            llm_text: None,
            is_meta: false,
        }
    }

    pub fn with_images(text: impl Into<String>, images: Vec<ImageData>) -> Self {
        Self::with_attachments(text, images, Vec::new())
    }

    pub fn with_attachments(
        text: impl Into<String>,
        images: Vec<ImageData>,
        files: Vec<FileAttachment>,
    ) -> Self {
        Self {
            text: text.into(),
            images,
            files,
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
        files: Vec<FileAttachment>,
    ) -> Self {
        Self {
            text: display_text.into(),
            images,
            files,
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
            files: Vec::new(),
            llm_text: None,
            is_meta: true,
        }
    }

    /// The text to deliver to the LLM: expanded form if present, display text otherwise.
    #[must_use]
    pub fn llm_text(&self) -> &str {
        self.llm_text.as_deref().unwrap_or(&self.text)
    }
}

/// Non-image file attachment in a user message. The bytes live on disk and the
/// LLM sees the path as text context; provider-native image payloads remain in
/// [`ImageData`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileAttachment {
    pub original_name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub stored_path: String,
}

impl FileAttachment {
    #[must_use]
    pub fn llm_context_tag(&self) -> String {
        format!(
            "<attached_file name=\"{}\" media_type=\"{}\" size_bytes=\"{}\" path=\"{}\" />",
            xml_attr_escape(&self.original_name),
            xml_attr_escape(&self.media_type),
            self.size_bytes,
            xml_attr_escape(&self.stored_path)
        )
    }
}

fn xml_attr_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Image attachment in a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageData {
    pub data: String,
    pub media_type: String,
}

impl ImageData {
    /// Convert to LLM `ImageSource` format
    #[must_use]
    pub fn to_image_source(&self) -> crate::domain::llm_types::ImageSource {
        crate::domain::llm_types::ImageSource::Base64 {
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
    ///
    /// serde(default): owned backward-compat decision, not a pending-migration
    /// shim (task 13023). Old `messages.content` rows written before the
    /// `images` feature existed genuinely carried no images, so an absent key
    /// correctly deserialises to an empty vec — there is no data to backfill
    /// and no migration is owed.
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileAttachment>,
}

/// Persisted projection of [`UserContent`].
///
/// The `messages.content` blob does NOT carry attachments — they live in the
/// `message_files` / `message_images` child tables, hydrated onto the runtime
/// [`UserContent`] by the DB layer. This stored shape has no attachment fields,
/// so a child collection cannot be smuggled back into the blob: the rollout-shim
/// bug class is structurally unrepresentable here.
#[derive(Serialize, Deserialize)]
struct StoredUserContent {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    llm_text: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    is_meta: bool,
}

impl From<&UserContent> for StoredUserContent {
    fn from(c: &UserContent) -> Self {
        Self {
            text: c.text.clone(),
            llm_text: c.llm_text.clone(),
            is_meta: c.is_meta,
        }
    }
}

impl StoredUserContent {
    /// Rebuild the runtime content with empty attachments; the DB layer fills
    /// them from the child tables.
    fn into_runtime(self) -> UserContent {
        UserContent {
            text: self.text,
            images: Vec::new(),
            files: Vec::new(),
            llm_text: self.llm_text,
            is_meta: self.is_meta,
        }
    }
}

/// Persisted projection of [`SkillContent`] — see [`StoredUserContent`].
#[derive(Serialize, Deserialize)]
struct StoredSkillContent {
    name: String,
    body: String,
    trigger: String,
}

impl From<&SkillContent> for StoredSkillContent {
    fn from(c: &SkillContent) -> Self {
        Self {
            name: c.name.clone(),
            body: c.body.clone(),
            trigger: c.trigger.clone(),
        }
    }
}

impl StoredSkillContent {
    fn into_runtime(self) -> SkillContent {
        SkillContent {
            name: self.name,
            body: self.body,
            trigger: self.trigger,
            files: Vec::new(),
        }
    }
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
    #[must_use]
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
    #[must_use]
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
    ///
    /// # Errors
    /// Returns `Err` if `value` does not deserialize into the content shape
    /// implied by `msg_type`.
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

    /// Serialize content for the persisted `messages.content` column.
    ///
    /// User/skill attachments are NOT included — they persist in the
    /// `message_files` / `message_images` child tables. Every other variant is
    /// byte-identical to [`Self::to_json`]. The wire path uses `to_json` (with
    /// attachments); only the DB write path uses this.
    #[must_use]
    pub fn to_stored_json(&self) -> Value {
        match self {
            Self::User(c) => {
                serde_json::to_value(StoredUserContent::from(c)).unwrap_or(Value::Null)
            }
            Self::Skill(c) => {
                serde_json::to_value(StoredSkillContent::from(c)).unwrap_or(Value::Null)
            }
            Self::Agent(_)
            | Self::Tool(_)
            | Self::System(_)
            | Self::Error(_)
            | Self::Continuation(_) => self.to_json(),
        }
    }

    /// Deserialize content from the persisted `messages.content` column.
    ///
    /// User/skill attachments come back EMPTY; the DB layer hydrates them from
    /// the child tables via [`Self::set_attachments`]. Every other variant
    /// matches [`Self::from_json`].
    ///
    /// # Errors
    /// Returns `Err` if `value` does not deserialize into the stored shape
    /// implied by `msg_type`.
    pub fn from_stored_json(msg_type: MessageType, value: Value) -> Result<Self, String> {
        match msg_type {
            MessageType::User => serde_json::from_value::<StoredUserContent>(value)
                .map(|s| Self::User(s.into_runtime()))
                .map_err(|e| format!("Invalid user content: {e}")),
            MessageType::Skill => serde_json::from_value::<StoredSkillContent>(value)
                .map(|s| Self::Skill(s.into_runtime()))
                .map_err(|e| format!("Invalid skill content: {e}")),
            MessageType::Agent
            | MessageType::Tool
            | MessageType::System
            | MessageType::Error
            | MessageType::Continuation => Self::from_json(msg_type, value),
        }
    }

    /// The user/skill attachments carried by this content — the source for the
    /// child-table writes. Empty for variants and messages that have none.
    #[must_use]
    pub fn attachments(&self) -> (&[ImageData], &[FileAttachment]) {
        match self {
            Self::User(c) => (&c.images, &c.files),
            Self::Skill(c) => (&[], &c.files),
            Self::Agent(_)
            | Self::Tool(_)
            | Self::System(_)
            | Self::Error(_)
            | Self::Continuation(_) => (&[], &[]),
        }
    }

    /// Replace the user/skill attachments — used when hydrating a row read from
    /// the DB with its child-table rows. `images` is ignored for skill content
    /// (skill invocations carry no inline images).
    pub fn set_attachments(&mut self, images: Vec<ImageData>, files: Vec<FileAttachment>) {
        match self {
            Self::User(c) => {
                c.images = images;
                c.files = files;
            }
            Self::Skill(c) => c.files = files,
            Self::Agent(_)
            | Self::Tool(_)
            | Self::System(_)
            | Self::Error(_)
            | Self::Continuation(_) => {}
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

    /// Create user content with typed attachments.
    pub fn user_with_attachments(
        text: impl Into<String>,
        images: Vec<ImageData>,
        files: Vec<FileAttachment>,
    ) -> Self {
        Self::User(UserContent::with_attachments(text, images, files))
    }

    /// Create agent content
    #[must_use]
    pub fn agent(blocks: Vec<ContentBlock>) -> Self {
        Self::Agent(blocks)
    }

    /// Create tool content with no images. Use [`MessageContent::tool_with_images`]
    /// when the source `ToolResult` carries images — passing none here would
    /// silently strand image bytes (the LLM/UI read images from this struct).
    pub fn tool(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::Tool(ToolContent::new(tool_use_id, content, is_error))
    }

    /// Create tool content carrying typed images for LLM/UI consumption.
    pub fn tool_with_images(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
        images: Vec<ToolContentImage>,
    ) -> Self {
        Self::Tool(ToolContent {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
            images,
        })
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
#[ts(export, export_to = "../../../ui/src/generated/")]
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

/// Lifecycle status for a `chain_qa` row (REQ-CHN-005).
///
/// Stored as a lowercase TEXT column; the enum is the authoritative shape on
/// the Rust side. New variants must update [`ChainQaStatus::as_str`] and
/// [`ChainQaStatus::from_db_str`] in lockstep — `from_db_str` is exhaustive,
/// so the compiler enforces the round-trip.
#[allow(dead_code)] // Phase 2 inserts via DB helpers; Phase 4 wires the API
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum ChainQaStatus {
    /// Row inserted at submission; stream is being generated by a live process.
    InFlight,
    /// Stream finished cleanly; `answer` and `completed_at` populated.
    Completed,
    /// Stream errored before producing a full answer (model error, parse failure,
    /// network drop). `answer` may carry a partial string; `completed_at` remains NULL.
    Failed,
    /// Stream did not complete and is no longer in flight (server restart, channel
    /// closed pre-completion). Distinct from `Failed`: there is no active failure
    /// cause, the stream was simply orphaned and cannot resume.
    Abandoned,
}

#[allow(dead_code)] // Phase 4 wires API consumers
impl ChainQaStatus {
    /// Persisted (lowercase) string representation. Stable across releases —
    /// changing this breaks DB rows in flight.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InFlight => "in_flight",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }

    /// Parse the persisted string back into the enum. Returns `None` for
    /// unknown values; callers (currently `parse_chain_qa_row` in `db.rs`)
    /// surface this as a typed error so unknown values are loud, not silent.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "in_flight" => Self::InFlight,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "abandoned" => Self::Abandoned,
            _ => return None,
        })
    }
}

/// New `chain_qa` row at insertion time (REQ-CHN-005).
///
/// Insert is the only path that writes all columns at once; subsequent state
/// transitions are partial UPDATEs. `answer` and `completed_at` are NULL by
/// construction — they're populated only when [`ChainQaStatus::Completed`]
/// or [`ChainQaStatus::Failed`] is reached.
#[allow(dead_code)] // Phase 2 constructs via chain_qa::ChainQa; Phase 4 wires API
#[derive(Debug, Clone)]
pub struct NewChainQa {
    pub id: String,
    pub root_conv_id: String,
    pub question: String,
    pub model: String,
    pub chain_members_at_answer: i64,
    pub chain_messages_at_answer: i64,
    pub created_at: DateTime<Utc>,
}

/// Persisted `chain_qa` row (REQ-CHN-005).
#[allow(dead_code)] // Phase 2 reads via chain_qa::ChainQa; Phase 4 wires API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ChainQaRow {
    pub id: String,
    pub root_conv_id: String,
    pub question: String,
    pub answer: Option<String>,
    pub model: String,
    pub status: ChainQaStatus,
    pub chain_members_at_answer: i64,
    pub chain_messages_at_answer: i64,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Type alias for backward compatibility — `Usage` is the canonical type.
pub type UsageData = crate::domain::llm_types::Usage;

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

/// One `(day, model)` aggregate of `turn_usage`. `day` is the UTC calendar day
/// (`date(created_at)`). Cost is *not* computed here — the API layer prices each
/// row by model, so the persistence layer stays pricing-agnostic.
#[derive(Debug, Clone, Serialize)]
pub struct UsageDailyModelRow {
    pub day: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub turns: i64,
}

/// One `(root_conversation, model)` aggregate of `turn_usage`, carrying the
/// conversation's display metadata. Sub-agent turns roll into their root, so a
/// conversation with mixed models yields one row per model — the API sums their
/// per-model costs to get a correct conversation total.
#[derive(Debug, Clone, Serialize)]
pub struct UsageConversationModelRow {
    pub root_conversation_id: String,
    pub model: String,
    pub slug: Option<String>,
    pub title: Option<String>,
    pub project_id: Option<String>,
    /// The conversation's worktree path (from the normalized `cm_worktree_path`
    /// column). `None` for modes without a worktree (Direct, sub-agent Explore).
    pub worktree_path: Option<String>,
    pub started_at: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub turns: i64,
}

/// One `turn_usage` row, scoped to a single conversation tree, for the
/// per-conversation drill-down timeseries.
#[derive(Debug, Clone, Serialize)]
pub struct UsageTurnRow {
    pub id: i64,
    pub conversation_id: String,
    pub root_conversation_id: String,
    pub model: String,
    pub created_at: String,
    pub first_byte_at: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
}

/// Timestamp-only message anchor used to derive turn latency without hydrating
/// full message content or attachments.
#[derive(Debug, Clone, Serialize)]
pub struct UsageAnchorRow {
    pub conversation_id: String,
    pub created_at: String,
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
        assert_eq!(
            parsed,
            ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            }
        );
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

        assert!(ConvMode::Explore {
            worktree_path: None,
            next_taskmd_id_hint: None,
        }
        .worktree_config()
        .is_none());
        assert!(ConvMode::Direct.worktree_config().is_none());
    }

    /// Pin the concrete `conv_mode` JSON shape that phoenix-db migrations query
    /// via raw SQL `json_extract`. Those SQL string literals (`$.mode`='Work',
    /// `$.worktree_path`, `$.base_branch`, `$.branch_name`, …) are untyped and
    /// hand-authored in `crates/phoenix-db/src/migrations.rs`; the serde
    /// roundtrip tests above are symmetric and would still pass if the `mode`
    /// tag or a field key were renamed — silently breaking every migration's
    /// `json_extract` path and any runtime SQL that reads this column. This
    /// test fails loudly on such a rename. If you change a key here, update the
    /// SQL literals in migrations.rs in the same change.
    #[test]
    fn test_sql_json_extract_contract() {
        // Tag values the SQL matches on `$.mode`.
        let direct: Value = serde_json::to_value(ConvMode::Direct).unwrap();
        assert_eq!(direct["mode"], "Direct");

        let explore_none: Value = serde_json::to_value(ConvMode::Explore {
            worktree_path: None,
            next_taskmd_id_hint: None,
        })
        .unwrap();
        assert_eq!(explore_none["mode"], "Explore");
        // skip_serializing_if = Option::is_none: the key must be ABSENT, which
        // the migration-007 Explore backfill relies on to detect un-backfilled
        // rows (`$.worktree_path` IS NULL).
        assert!(explore_none.get("worktree_path").is_none());
        assert!(explore_none.get("next_taskmd_id_hint").is_none());

        let explore_some: Value = serde_json::to_value(ConvMode::Explore {
            worktree_path: Some(NonEmptyString::new("/wt/explore").unwrap()),
            next_taskmd_id_hint: Some(NonEmptyString::new("43002").unwrap()),
        })
        .unwrap();
        assert_eq!(explore_some["mode"], "Explore");
        assert_eq!(explore_some["worktree_path"], "/wt/explore");
        assert_eq!(explore_some["next_taskmd_id_hint"], "43002");

        // Work: SQL reads $.mode, $.worktree_path, $.base_branch, $.branch_name.
        let work: Value = serde_json::to_value(ConvMode::Work {
            branch_name: NonEmptyString::new("task-0042-fix-bug").unwrap(),
            worktree_path: NonEmptyString::new("/wt/abc").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("YF042").unwrap(),
            task_title: NonEmptyString::new("Fix the bug").unwrap(),
        })
        .unwrap();
        assert_eq!(work["mode"], "Work");
        assert_eq!(work["worktree_path"], "/wt/abc");
        assert_eq!(work["base_branch"], "main");
        assert_eq!(work["branch_name"], "task-0042-fix-bug");

        // Branch: SQL reads the same path fields, and Branch must carry no task_id.
        let branch: Value = serde_json::to_value(ConvMode::Branch {
            branch_name: NonEmptyString::new("fix-login").unwrap(),
            worktree_path: NonEmptyString::new("/wt/login").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
        })
        .unwrap();
        assert_eq!(branch["mode"], "Branch");
        assert_eq!(branch["worktree_path"], "/wt/login");
        assert_eq!(branch["base_branch"], "main");
        assert_eq!(branch["branch_name"], "fix-login");
        assert!(branch.get("task_id").is_none());
    }
}

#[cfg(test)]
mod error_kind_tests {
    use super::*;

    #[test]
    fn all_error_kinds_have_explicit_auto_retry_and_user_resume_policy() {
        use crate::domain::retry_policy::{AutoRetryPolicy, UserResumePolicy};
        use ErrorKind::{
            Auth, Cancelled, ContentFilter, ContextExhausted, InvalidRequest, InvalidResponse,
            Network, RateLimit, ServerError, ServerOverloaded, SubAgentError, TimedOut,
            TurnLimitExhausted, UsageLimitReached,
        };

        let cases = [
            (
                Network,
                AutoRetryPolicy::AutoRetryable,
                UserResumePolicy::Resumable,
            ),
            (
                RateLimit,
                AutoRetryPolicy::AutoRetryable,
                UserResumePolicy::Resumable,
            ),
            (
                UsageLimitReached,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::Resumable,
            ),
            (
                ServerError,
                AutoRetryPolicy::AutoRetryable,
                UserResumePolicy::Resumable,
            ),
            (
                InvalidResponse,
                AutoRetryPolicy::AutoRetryable,
                UserResumePolicy::Resumable,
            ),
            (
                ServerOverloaded,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::Resumable,
            ),
            (
                TimedOut,
                AutoRetryPolicy::AutoRetryable,
                UserResumePolicy::Resumable,
            ),
            (
                Auth,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::Resumable,
            ),
            (
                InvalidRequest,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::NotResumable,
            ),
            (
                Cancelled,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::NotResumable,
            ),
            (
                SubAgentError,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::NotResumable,
            ),
            (
                ContextExhausted,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::NotResumable,
            ),
            (
                TurnLimitExhausted,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::NotResumable,
            ),
            (
                ContentFilter,
                AutoRetryPolicy::NoAutoRetry,
                UserResumePolicy::NotResumable,
            ),
        ];

        for (kind, auto_retry, user_resume) in cases {
            assert_eq!(
                kind.auto_retry_policy(),
                auto_retry,
                "auto retry for {kind:?}"
            );
            assert_eq!(
                kind.user_resume_policy(),
                user_resume,
                "user resume for {kind:?}"
            );
        }
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
            transcript_generation: 1,
            model: None,
            project_id: None,
            conv_mode: ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            },
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id,
            chain_name: None,
            llm_language: crate::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
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

    /// Task 13023: tool-result rows in `messages.content` written before the
    /// `images` feature deserialize cleanly to an empty `images` vec. This is
    /// an owned backward-compat decision (those rows genuinely had no images),
    /// not a pending-migration shim — this test locks the contract so a future
    /// change to a hard error is a deliberate, visible break.
    #[test]
    fn pre_images_tool_rows_deserialize_to_empty_images() {
        // ToolOutcome::Success / Error — the persisted tag-shaped JSON with no
        // `images` key, as old rows were written.
        let success: ToolOutcome =
            serde_json::from_str(r#"{"type":"success","output":"ok"}"#).unwrap();
        match success {
            ToolOutcome::Success { images, .. } => assert!(images.is_empty()),
            other @ (ToolOutcome::Error { .. } | ToolOutcome::Cancelled { .. }) => {
                panic!("expected Success, got {other:?}")
            }
        }
        let error: ToolOutcome =
            serde_json::from_str(r#"{"type":"error","output":"boom"}"#).unwrap();
        match error {
            ToolOutcome::Error { images, .. } => assert!(images.is_empty()),
            other @ (ToolOutcome::Success { .. } | ToolOutcome::Cancelled { .. }) => {
                panic!("expected Error, got {other:?}")
            }
        }

        // ToolContent — old tool-result message content with no `images` key.
        let content: ToolContent =
            serde_json::from_str(r#"{"tool_use_id":"t1","content":"out","is_error":false}"#)
                .unwrap();
        assert!(content.images.is_empty());
    }
}
