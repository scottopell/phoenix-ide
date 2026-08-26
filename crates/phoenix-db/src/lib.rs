//! Database module for Phoenix IDE
//!
//! Provides persistence for conversations and messages.

mod close_foundation;
mod coordinator_query;
mod ddl;
mod git_repository_reconciliation;
mod migrations;
mod product_conversation_read;
pub mod retrieval;
mod sqlite_native_statement;
mod sqlite_telemetry;
mod sqlite_workload;
pub mod workflow;
// The schema *types* (MessageContent, ToolResult, ConvState's persisted shape,
// …) moved to the phoenix-core domain crate to break the db↔state_machine
// cycle. Alias the module back as `schema` so the persistence logic in this
// file and `phoenix_db::*` call sites resolve unchanged.
use phoenix_core::domain::creation_protocol::{
    CreationClaim, CreationClaimToken, CreationError, CreationKind, CreationProtocolState,
    CreationStage, CreationStatus, CreationWorkerId,
};
use phoenix_core::domain::db_schema as schema;
use phoenix_core::domain::sm_state::LEGACY_CONTINUATION_OPERATION_ID;
use phoenix_core::work_scope::{
    AuthorityKind, EnvironmentContext, RuntimeRole, WorkScopeId, WorkScopeRetirementBlocker,
    WorkScopeRetirementOutcome, WorkScopeRetirementPrecondition,
};

pub use close_foundation::*;
pub use coordinator_query::{
    execute_coordinator_query, CoordinatorQueryError, CoordinatorQueryResult,
};
pub(crate) use git_repository_reconciliation::{
    DormantGitRepositoryCatchupOutcome, DormantGitRepositoryCatchupPermit,
};
pub use migrations::run_pending_migrations;
pub use product_conversation_read::{
    ProductConversationAggregate, ProductConversationHandoff, ProductConversationListProjection,
    ProductConversationSegment, ProductConversationSegmentCeiling, ProductConversationSnapshotRead,
    ProductConversationSource, ProductConversationSourceKind, ProductConversationTranscriptRow,
    ProductConversationWorkIdentity, ResolvedProductConversation,
};
pub use retrieval::{
    Fts5Retriever, MessageRetriever, ReconcileStats, RetrievalError, RetrievalGrouping,
    RetrievalMatchMode, RetrievalRequest, RetrievalScope, RetrievalVisibility, RetrievedChunk,
};
pub use schema::*;
pub use sqlite_workload::{
    abandoned_count, approximate_percentiles_from_histogram, operation_count, BucketCategoryTotals,
    SampledSqliteWorkloadAggregateReport, SqliteAccessKind, SqliteLatencyBin, SqliteOutcome,
    SqlitePercentiles, SqliteSnapshotWindow, SqliteWorkloadAggregateReport, SqliteWorkloadCategory,
    SqliteWorkloadCollector, SqliteWorkloadSnapshot,
};
pub use workflow::*;

/// Maximum pending steering entries permitted per conversation.
pub const MAX_STEERING_QUEUE_DEPTH: usize = 5;

use chrono::{DateTime, Utc};
use phoenix_core::domain::llm_types::{
    EffectiveEffort, EffortSource, LlmAttemptMetrics, LlmAttemptOutcome, LlmTransport, ModelEffort,
    ProviderStreamTelemetry, ServiceTier, StreamTelemetryOutputKind,
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use sqlite_native_statement::install_native_statement_baseline;
use sqlite_telemetry::{SqliteOperation, SqliteTelemetry};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{Connection, Row, Sqlite, SqlitePool, Transaction};
use std::fmt::Write as _;
use std::str::FromStr;
use thiserror::Error;

/// Restrict the `SQLite` database file and its WAL sidecars to owner read/write
/// (`0600`). The DB holds conversation history (command output, secrets the
/// agent observed) and credentials at rest (MCP OAuth access/refresh tokens,
/// registered client secrets); the default umask can leave it
/// group/world-readable on a shared host. Best-effort and Unix-only: a `chmod`
/// failure is logged at debug and never fails startup; a no-op on non-Unix
/// platforms.
///
/// Owner-only filesystem permission is the accepted at-rest control for these
/// secrets: Phoenix is a single-user server, so the DB owner is the trust
/// boundary, and application-layer encryption would need a key stored next to
/// the data on the same single-user host — moving, not closing, the exposure.
/// Anyone who can read this file as its owner can already read the live
/// process's memory. The same posture covers `~/.phoenix-ide/codex-auth.json`
/// (written `0600`).
#[cfg(unix)]
fn restrict_db_permissions(path: &str) {
    use std::os::unix::fs::PermissionsExt;

    // The `-wal` and `-shm` sidecars share the DB's sensitivity. Only files that
    // exist are chmod'd; absent sidecars are skipped silently.
    for suffix in ["", "-wal", "-shm"] {
        let p = format!("{path}{suffix}");
        match std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::debug!(path = %p, error = %e, "could not chmod db file to 0600");
            }
        }
    }
}

#[cfg(not(unix))]
fn restrict_db_permissions(_path: &str) {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseFoundationRepair {
    UnresolvedWorktreeIdentity {
        attempt_id: phoenix_core::domain::close::CloseAttemptId,
        scope: WorkScopeId,
        locator: phoenix_core::domain::close::GitPathIdentity,
    },
}

/// Project identity carried only to the repository seed boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectSeedId(String);

impl ProjectSeedId {
    /// # Errors
    /// Returns [`ProjectSeedIdError`] when the supplied identifier is empty.
    pub(crate) fn parse(value: impl Into<String>) -> Result<Self, ProjectSeedIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProjectSeedIdError::Empty);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectSeedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectSeedIdError {
    Empty,
}

impl std::fmt::Display for ProjectSeedIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Project seed id must not be empty"),
        }
    }
}

impl std::error::Error for ProjectSeedIdError {}

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Conversation not found: {0}")]
    ConversationNotFound(String),
    #[error("Conversation already exists: {0}")]
    ConversationAlreadyExists(String),
    #[error("Message not found: {0}")]
    MessageNotFound(String),
    #[error("Slug already exists: {0}")]
    SlugExists(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Continuation precondition failed: {0}")]
    ContinuationPrecondition(String),
    #[error("Close foundation conflict: {0}")]
    CloseFoundationConflict(String),
    #[error("new aggregate work is fenced by Close attempt {0:?}")]
    CloseAdmissionFenced(CloseAdmissionFence),
    #[error("ProductConversation {0} is unavailable because it is in History")]
    ProductConversationUnavailable(
        phoenix_core::domain::product_conversation::ProductConversationId,
    ),
    #[error("steering queue is full")]
    SteeringQueueFull,
    #[error("Close foundation precondition failed: {0}")]
    CloseFoundationPrecondition(String),
    #[error("Close foundation repair required: {0:?}")]
    CloseFoundationRepairRequired(CloseFoundationRepair),
    #[error("Close foundation record not found: {0}")]
    CloseFoundationNotFound(String),
    #[error("Direct-turn conflict: {0:?}")]
    DirectTurnConflict(phoenix_workflow::TurnConflict),
    /// A fork-proposal resolution was attempted but the proposal is already
    /// resolved to a different state or child id (REQ-PROJ-034/037). Distinct
    /// from the idempotent no-op case (same child id), which returns `Ok`.
    #[error("Fork proposal conflict: {0}")]
    ForkProposalConflict(String),
    #[error("git repository work-scope project conflict for {work_scope_id}: {project_ids:?}")]
    GitRepositoryWorkScopeProjectConflict {
        work_scope_id: WorkScopeId,
        project_ids: [ProjectSeedId; 2],
    },
    #[error("dormant git repository catch-up permit targeted a different database")]
    DormantGitRepositoryCatchupPermitTargetMismatch,
    #[error("dormant git repository catch-up operation is stale")]
    DormantGitRepositoryCatchupStaleOperation,
    #[error("dormant git repository catch-up cannot start while readiness has claimed a receipt")]
    DormantGitRepositoryCatchupBlockedByReadinessClaim,
    #[error(
        "dormant git repository readiness cannot claim a receipt while catch-up is in progress"
    )]
    DormantGitRepositoryReadinessCatchupInProgress,
    #[error("dormant git repository readiness receipt targeted a different database lifecycle")]
    DormantGitRepositoryReadinessReceiptTargetMismatch,
    #[error("dormant git repository readiness receipt did not match an exact completed catch-up operation")]
    DormantGitRepositoryReadinessReceiptOperationMismatch,
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductRootReservationRecord {
    pub id: String,
    pub cwd: String,
    pub kind: String,
    pub repo_root: Option<String>,
    pub repository_id: Option<String>,
    pub exact_checkout_oid: Option<String>,
    pub logical_base: Option<String>,
    pub freshness: Option<String>,
    pub unresolved_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentHiddenRepositoryManagementRoot {
    pub repository_id: phoenix_core::git_repository::GitRepositoryId,
    pub management_root: String,
    pub observed_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRepositoryDefaultBranchObservation {
    Resolved { branch: String, provenance: String },
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachHiddenGitRepositoryInput {
    pub conversation_id: String,
    pub common_dir: String,
    pub management_root: String,
    pub materialized_worktree: String,
    pub default_branch: GitRepositoryDefaultBranchObservation,
    pub observed_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedTaskRootReservationInput {
    pub repository_id: phoenix_core::git_repository::GitRepositoryId,
    pub repository_root: String,
    pub exact_checkout_oid: String,
    pub logical_base: String,
}

const PRODUCT_ROOT_RESERVATION_RECLAIM_AFTER: chrono::Duration = chrono::Duration::days(7);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedHiddenGitRepository {
    pub work_scope_id: WorkScopeId,
    pub repository_id: phoenix_core::git_repository::GitRepositoryId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteeringDrainMessageStatus {
    Inserted,
    LegacyAlreadyMaterialized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteeringAcceptanceFingerprint {
    Exact(String),
    LegacyUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSearchMetadata {
    pub slug: String,
    pub archived: bool,
}

pub(crate) async fn persist_continuation_start_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    operation_id: &str,
    message: &Message,
    target_state: &ConvState,
    state_updated_at: DateTime<Utc>,
) -> DbResult<ContinuationCommitOutcome> {
    let row = sqlx::query("SELECT state FROM conversations WHERE id = ?1")
        .bind(conversation_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::ConversationNotFound(conversation_id.to_string()))?;
    let persisted_json: String = row.get("state");
    let persisted: ConvState = serde_json::from_str(&persisted_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let owns_operation = matches!(
        &persisted,
        ConvState::AwaitingContinuation { request } if request.operation_id == operation_id
    ) || matches!(
        &persisted,
        ConvState::RecoverableContinuationFailure { failure }
            if failure.request.operation_id == operation_id
    );
    if owns_operation {
        let exists = sqlx::query("SELECT 1 FROM messages WHERE message_id = ?1")
            .bind(&message.message_id)
            .fetch_optional(&mut **tx)
            .await?
            .is_some();
        return Ok(if exists {
            ContinuationCommitOutcome::Duplicate
        } else {
            ContinuationCommitOutcome::Stale
        });
    }
    if !matches!(
        persisted,
        ConvState::LlmRequesting { .. } | ConvState::SeededLlmRequesting { .. }
    ) {
        return Ok(ContinuationCommitOutcome::Stale);
    }
    insert_message_tx(tx, message).await?;
    let target_json = serde_json::to_string(target_state)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let updated = sqlx::query(
        "UPDATE conversations
         SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4
         WHERE id = ?5 AND state = ?6",
    )
    .bind(target_json)
    .bind(conv_state_kind(target_state))
    .bind(state_updated_at.to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .bind(conversation_id)
    .bind(persisted_json)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(ContinuationCommitOutcome::Stale);
    }
    Ok(ContinuationCommitOutcome::Applied)
}

pub(crate) async fn commit_continuation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    operation_id: &str,
    message: &Message,
    completed_state: &ConvState,
    state_updated_at: DateTime<Utc>,
) -> DbResult<ContinuationCommitOutcome> {
    let row = sqlx::query("SELECT state FROM conversations WHERE id = ?1")
        .bind(conversation_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::ConversationNotFound(conversation_id.to_string()))?;
    let persisted_json: String = row.get("state");
    let persisted: ConvState = serde_json::from_str(&persisted_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;

    let ConvState::ContextExhausted {
        summary: completed_summary,
    } = completed_state
    else {
        return Err(DbError::Serialization(
            "continuation commit requires context exhausted state".to_string(),
        ));
    };
    if matches!(
        &persisted,
        ConvState::ContextExhausted { summary } if summary == completed_summary
    ) {
        let exists = sqlx::query("SELECT 1 FROM messages WHERE message_id = ?1")
            .bind(&message.message_id)
            .fetch_optional(&mut **tx)
            .await?
            .is_some();
        return Ok(if exists {
            ContinuationCommitOutcome::Duplicate
        } else {
            ContinuationCommitOutcome::Stale
        });
    }

    if !matches!(
        &persisted,
        ConvState::AwaitingContinuation { request }
            if request.operation_id == operation_id
    ) {
        return Ok(ContinuationCommitOutcome::Stale);
    }

    insert_message_tx(tx, message).await?;
    let completed_json = serde_json::to_string(completed_state)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let updated = sqlx::query(
        "UPDATE conversations
         SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4
         WHERE id = ?5 AND state = ?6",
    )
    .bind(completed_json)
    .bind(conv_state_kind(completed_state))
    .bind(state_updated_at.to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .bind(conversation_id)
    .bind(persisted_json)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(ContinuationCommitOutcome::Stale);
    }
    Ok(ContinuationCommitOutcome::Applied)
}

pub(crate) async fn reconcile_legacy_half_committed_continuation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    state_updated_at: DateTime<Utc>,
) -> DbResult<Option<String>> {
    let row = sqlx::query("SELECT state FROM conversations WHERE id = ?1")
        .bind(conversation_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::ConversationNotFound(conversation_id.to_string()))?;
    let persisted_json: String = row.get("state");
    let persisted: ConvState = serde_json::from_str(&persisted_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    if !matches!(
        persisted,
        ConvState::AwaitingContinuation { ref request }
            if request.operation_id == LEGACY_CONTINUATION_OPERATION_ID
    ) {
        return Ok(None);
    }

    let row = sqlx::query(
        "SELECT content FROM messages
         WHERE conversation_id = ?1 AND message_type = 'continuation'
         ORDER BY sequence_id DESC LIMIT 1",
    )
    .bind(conversation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let content_json: String = row.get("content");
    let value = serde_json::from_str(&content_json)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let content = MessageContent::from_stored_json(MessageType::Continuation, value)
        .map_err(DbError::Serialization)?;
    let MessageContent::Continuation(summary) = content else {
        return Ok(None);
    };
    let summary = summary.summary;
    let target = ConvState::ContextExhausted {
        summary: summary.clone(),
    };
    let target_json = serde_json::to_string(&target)
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let updated = sqlx::query(
        "UPDATE conversations
         SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4
         WHERE id = ?5 AND state = ?6",
    )
    .bind(target_json)
    .bind(conv_state_kind(&target))
    .bind(state_updated_at.to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .bind(conversation_id)
    .bind(persisted_json)
    .execute(&mut **tx)
    .await?;
    Ok((updated.rows_affected() == 1).then_some(summary))
}

#[derive(Debug, Clone)]
pub struct InsertConversationCreationJob {
    pub id: String,
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub intent: ConversationCreationIntent,
}

#[derive(Debug, Clone)]
pub enum CreationClaimOutcome {
    Claimed(Box<ConversationCreationJob>),
    NoEligibleJob,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationCasOutcome {
    Applied,
    ClaimLost,
}

#[derive(Debug, Clone)]
pub enum CreationRuntimeMaterialization {
    Materialized(Box<Message>),
    ClaimLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationResourceReservation {
    pub id: String,
    pub job_id: String,
    pub generation: u64,
    pub repository_identity: String,
    pub resource_identity: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreationCleanupJob {
    pub job_id: String,
    pub conversation_id: String,
    pub intent: ConversationCreationIntent,
    pub status: String,
    pub generation: u64,
    pub worker_id: String,
    pub token: String,
    pub lease_until: DateTime<Utc>,
    pub reservations: Vec<CreationResourceReservation>,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationCreationMetadataUpdate {
    pub slug: Option<String>,
    pub title: Option<Option<String>>,
    pub cwd: Option<String>,
    pub project_id: Option<Option<String>>,
    pub desired_base_branch: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationDispatchIntent {
    pub parent_conversation_id: String,
    pub successor_conversation_id: String,
    pub message_id: String,
    pub handoff: String,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewContinuationDispatchIntent {
    pub message_id: String,
    pub handoff: String,
    pub user_agent: Option<String>,
}

/// Outcome of [`Database::continue_conversation`] (REQ-BED-030).
///
/// The DB layer returns a typed outcome so the handler can map each arm to a
/// distinct HTTP status without restringifying error messages. Each variant
/// is a first-class result, not an error.
#[derive(Debug)]
pub enum ContinueOutcome {
    /// The transaction applied: a new conversation was created and the
    /// parent's `continued_in_conv_id` now points at it.
    Created(Conversation),
    /// The parent already had a continuation. The transaction did not run;
    /// the returned conversation is the pre-existing continuation (the
    /// endpoint returns this idempotently rather than rejecting).
    AlreadyContinued(Conversation),
    /// The parent exists but is not in `ContextExhausted` state. The
    /// transaction did not run.
    ParentNotContextExhausted { state_variant: &'static str },
}

/// One `mcp_oauth_registrations` row: the OAuth client identity for an
/// authorization server, shared by every MCP server behind it (REQ-MCP-010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthRegistrationRow {
    pub auth_server: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: String,
    /// The `redirect_uri` the client was registered with, when known; `None`
    /// for a pre-configured client or a pre-redirect-tracking row.
    pub redirect_uri: Option<String>,
}

/// One `mcp_oauth_tokens` row: the OAuth token for an MCP server,
/// audience-bound to `resource_uri` (REQ-MCP-012). `scopes` is the
/// space-separated granted scope set; `expires_at` is unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthTokenRow {
    pub server_name: String,
    pub resource_uri: String,
    pub scopes: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopePrObservation {
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub display_state: phoenix_core::domain::pr_display_state::PrDisplayState,
    pub base: String,
    pub head: String,
    pub github_updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBranchQualificationInput {
    pub conversation_base_branch: String,
    pub task_relative_work_base_head_oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopeObservedBranchUpsert {
    pub repository_identity: String,
    pub branch_name: String,
    pub head_oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopePrFeedbackBaseline {
    pub work_scope_id: phoenix_core::work_scope::WorkScopeId,
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
    pub captured_at: String,
    pub github_updated_at: Option<String>,
    pub feedback_identities: Vec<String>,
    pub feedback_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkScopePrFeedbackBaselineInput {
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
    pub captured_at: String,
    pub github_updated_at: Option<String>,
    pub feedback_identities: Vec<String>,
    pub feedback_fingerprints: Vec<String>,
}

type PrFeedbackStatus = phoenix_core::domain::pr_feedback_status::PrFeedbackStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopePrAssociation {
    pub work_scope_id: phoenix_core::work_scope::WorkScopeId,
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub display_state: phoenix_core::domain::pr_display_state::PrDisplayState,
    pub base: String,
    pub head: String,
    pub github_updated_at: Option<String>,
    pub feedback_status: PrFeedbackStatus,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopeObservedBranch {
    pub work_scope_id: phoenix_core::work_scope::WorkScopeId,
    pub repository_identity: String,
    pub branch_name: String,
    pub first_observed_head_oid: String,
    pub last_observed_head_oid: String,
    pub first_observed_at: String,
    pub last_observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopeActivePrSelectionRow {
    pub work_scope_id: phoenix_core::work_scope::WorkScopeId,
    pub selection: Option<phoenix_core::domain::active_pr_selection::ActivePrSelection>,
    pub latest_observed_branch:
        Option<phoenix_core::domain::active_pr_selection::ActivePrBranchContext>,
    pub inference_generation: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequestMetricsRow {
    pub request_id: String,
    pub retry_attempt: u32,
    pub created_at: String,
    pub metrics: LlmAttemptMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecentLlmMetricRow {
    pub request_id: String,
    pub retry_attempt: u32,
    pub created_at: String,
    pub provider: String,
    pub model: String,
    pub transport: LlmTransport,
    pub outcome: LlmAttemptOutcome,
    pub dispatch_to_first_generation_event_ms: Option<u64>,
}

fn pr_display_state_db(
    state: &phoenix_core::domain::pr_display_state::PrDisplayState,
) -> &'static str {
    match state {
        phoenix_core::domain::pr_display_state::PrDisplayState::Open => "open",
        phoenix_core::domain::pr_display_state::PrDisplayState::Draft => "draft",
        phoenix_core::domain::pr_display_state::PrDisplayState::Merged => "merged",
        phoenix_core::domain::pr_display_state::PrDisplayState::Closed => "closed",
    }
}

fn pr_display_state_from_db(
    value: &str,
) -> DbResult<phoenix_core::domain::pr_display_state::PrDisplayState> {
    match value {
        "open" => Ok(phoenix_core::domain::pr_display_state::PrDisplayState::Open),
        "draft" => Ok(phoenix_core::domain::pr_display_state::PrDisplayState::Draft),
        "merged" => Ok(phoenix_core::domain::pr_display_state::PrDisplayState::Merged),
        "closed" => Ok(phoenix_core::domain::pr_display_state::PrDisplayState::Closed),
        other => Err(DbError::Serialization(format!(
            "invalid PR display_state in database: {other}"
        ))),
    }
}

fn pr_feedback_status_db(status: PrFeedbackStatus) -> &'static str {
    match status {
        PrFeedbackStatus::Open => "open",
        PrFeedbackStatus::InProgress => "in_progress",
        PrFeedbackStatus::Approved => "approved",
    }
}

fn pr_feedback_status_from_db(value: &str) -> DbResult<PrFeedbackStatus> {
    match value {
        "open" => Ok(PrFeedbackStatus::Open),
        "in_progress" => Ok(PrFeedbackStatus::InProgress),
        "approved" => Ok(PrFeedbackStatus::Approved),
        other => Err(DbError::Serialization(format!(
            "invalid PR feedback_status in database: {other}"
        ))),
    }
}

fn row_to_work_scope_observed_branch(row: &SqliteRow) -> WorkScopeObservedBranch {
    WorkScopeObservedBranch {
        work_scope_id: row_work_scope_id(row),
        repository_identity: row.get("repository_identity"),
        branch_name: row.get("branch_name"),
        first_observed_head_oid: row.get("first_observed_head_oid"),
        last_observed_head_oid: row.get("last_observed_head_oid"),
        first_observed_at: row.get("first_observed_at"),
        last_observed_at: row.get("last_observed_at"),
    }
}

fn u64_to_i64(value: u64) -> DbResult<i64> {
    i64::try_from(value).map_err(|_| {
        DbError::Serialization(format!("value out of range for SQLite INTEGER: {value}"))
    })
}

fn opt_u64_to_i64(value: Option<u64>) -> DbResult<Option<i64>> {
    value.map(u64_to_i64).transpose()
}

fn i64_to_u64(value: i64, field: &str) -> DbResult<u64> {
    u64::try_from(value)
        .map_err(|_| DbError::Serialization(format!("negative {field} in database: {value}")))
}

fn llm_transport_from_db(value: &str) -> DbResult<LlmTransport> {
    match value {
        "http_sse" => Ok(LlmTransport::HttpSse),
        "websocket" => Ok(LlmTransport::Websocket),
        "in_process" => Ok(LlmTransport::InProcess),
        "http_json" => Ok(LlmTransport::HttpJson),
        other => Err(DbError::Serialization(format!(
            "invalid llm transport in database: {other}"
        ))),
    }
}

fn stream_output_kind_db(value: StreamTelemetryOutputKind) -> &'static str {
    match value {
        StreamTelemetryOutputKind::None => "none",
        StreamTelemetryOutputKind::Text => "text",
        StreamTelemetryOutputKind::Reasoning => "reasoning",
        StreamTelemetryOutputKind::Tool => "tool",
        StreamTelemetryOutputKind::Structured => "structured",
        StreamTelemetryOutputKind::Mixed => "mixed",
    }
}

fn stream_output_kind_from_db(value: &str) -> DbResult<StreamTelemetryOutputKind> {
    match value {
        "none" => Ok(StreamTelemetryOutputKind::None),
        "text" => Ok(StreamTelemetryOutputKind::Text),
        "reasoning" => Ok(StreamTelemetryOutputKind::Reasoning),
        "tool" => Ok(StreamTelemetryOutputKind::Tool),
        "structured" => Ok(StreamTelemetryOutputKind::Structured),
        "mixed" => Ok(StreamTelemetryOutputKind::Mixed),
        other => Err(DbError::Serialization(format!(
            "invalid stream output kind in database: {other}"
        ))),
    }
}

fn llm_attempt_outcome_db(value: &LlmAttemptOutcome) -> &'static str {
    match value {
        LlmAttemptOutcome::Success => "success",
        LlmAttemptOutcome::RateLimited => "rate_limited",
        LlmAttemptOutcome::UsageLimitReached => "usage_limit_reached",
        LlmAttemptOutcome::ServerError => "server_error",
        LlmAttemptOutcome::InvalidResponse => "invalid_response",
        LlmAttemptOutcome::ServerOverloaded => "server_overloaded",
        LlmAttemptOutcome::NetworkError => "network_error",
        LlmAttemptOutcome::TokenBudgetExceeded => "token_budget_exceeded",
        LlmAttemptOutcome::AuthError => "auth_error",
        LlmAttemptOutcome::RequestRejected => "request_rejected",
        LlmAttemptOutcome::Cancelled => "cancelled",
    }
}

fn llm_attempt_outcome_from_db(value: &str) -> DbResult<LlmAttemptOutcome> {
    match value {
        "success" => Ok(LlmAttemptOutcome::Success),
        "rate_limited" => Ok(LlmAttemptOutcome::RateLimited),
        "usage_limit_reached" => Ok(LlmAttemptOutcome::UsageLimitReached),
        "server_error" => Ok(LlmAttemptOutcome::ServerError),
        "invalid_response" => Ok(LlmAttemptOutcome::InvalidResponse),
        "server_overloaded" => Ok(LlmAttemptOutcome::ServerOverloaded),
        "network_error" => Ok(LlmAttemptOutcome::NetworkError),
        "token_budget_exceeded" => Ok(LlmAttemptOutcome::TokenBudgetExceeded),
        "auth_error" => Ok(LlmAttemptOutcome::AuthError),
        "request_rejected" => Ok(LlmAttemptOutcome::RequestRejected),
        "cancelled" => Ok(LlmAttemptOutcome::Cancelled),
        other => Err(DbError::Serialization(format!(
            "invalid llm attempt outcome in database: {other}"
        ))),
    }
}

fn row_to_llm_request_metrics(row: &SqliteRow) -> DbResult<LlmRequestMetricsRow> {
    let request_id: String = row.try_get("request_id")?;
    let retry_attempt_i64: i64 = row.try_get("retry_attempt")?;
    let retry_attempt = u32::try_from(retry_attempt_i64).map_err(|_| {
        DbError::Serialization(format!(
            "retry_attempt out of range in database: {retry_attempt_i64}"
        ))
    })?;
    let total_duration_ms = i64_to_u64(row.try_get("total_duration_ms")?, "total_duration_ms")?;
    Ok(LlmRequestMetricsRow {
        request_id: request_id.clone(),
        retry_attempt,
        created_at: row.try_get("created_at")?,
        metrics: LlmAttemptMetrics {
            conversation_id: row.try_get("conversation_id")?,
            root_conversation_id: row.try_get("root_conversation_id")?,
            request_id,
            retry_attempt,
            provider: row.try_get("provider")?,
            model: row.try_get("model")?,
            transport: llm_transport_from_db(&row.try_get::<String, _>("transport")?)?,
            total_duration_ms,
            stream: ProviderStreamTelemetry {
                dispatch_to_first_provider_event_ms: opt_u64_from_db(
                    row.try_get("dispatch_to_first_provider_event_ms")?,
                    "dispatch_to_first_provider_event_ms",
                )?,
                dispatch_to_first_generation_event_ms: opt_u64_from_db(
                    row.try_get("dispatch_to_first_generation_event_ms")?,
                    "dispatch_to_first_generation_event_ms",
                )?,
                dispatch_to_first_visible_text_ms: opt_u64_from_db(
                    row.try_get("dispatch_to_first_visible_text_ms")?,
                    "dispatch_to_first_visible_text_ms",
                )?,
                provider_event_count: u32::try_from(row.try_get::<i64, _>("provider_event_count")?)
                    .map_err(|_| {
                        DbError::Serialization(
                            "provider_event_count out of range in database".to_string(),
                        )
                    })?,
                generation_event_count: u32::try_from(
                    row.try_get::<i64, _>("generation_event_count")?,
                )
                .map_err(|_| {
                    DbError::Serialization(
                        "generation_event_count out of range in database".to_string(),
                    )
                })?,
                visible_text_event_count: u32::try_from(
                    row.try_get::<i64, _>("visible_text_event_count")?,
                )
                .map_err(|_| {
                    DbError::Serialization(
                        "visible_text_event_count out of range in database".to_string(),
                    )
                })?,
                max_provider_gap_ms: opt_u64_from_db(
                    row.try_get("max_provider_gap_ms")?,
                    "max_provider_gap_ms",
                )?,
                max_generation_gap_ms: opt_u64_from_db(
                    row.try_get("max_generation_gap_ms")?,
                    "max_generation_gap_ms",
                )?,
                output_kind: stream_output_kind_from_db(&row.try_get::<String, _>("output_kind")?)?,
                completed: row.try_get("stream_completed")?,
            },
            outcome: llm_attempt_outcome_from_db(&row.try_get::<String, _>("outcome")?)?,
        },
    })
}

fn row_to_usage_recent_llm_metric(row: &SqliteRow) -> DbResult<UsageRecentLlmMetricRow> {
    let retry_attempt_i64: i64 = row.try_get("retry_attempt")?;
    let retry_attempt = u32::try_from(retry_attempt_i64).map_err(|_| {
        DbError::Serialization(format!(
            "retry_attempt out of range in database: {retry_attempt_i64}"
        ))
    })?;
    Ok(UsageRecentLlmMetricRow {
        request_id: row.try_get("request_id")?,
        retry_attempt,
        created_at: row.try_get("created_at")?,
        provider: row.try_get("provider")?,
        model: row.try_get("model")?,
        transport: llm_transport_from_db(&row.try_get::<String, _>("transport")?)?,
        outcome: llm_attempt_outcome_from_db(&row.try_get::<String, _>("outcome")?)?,
        dispatch_to_first_generation_event_ms: opt_u64_from_db(
            row.try_get("dispatch_to_first_generation_event_ms")?,
            "dispatch_to_first_generation_event_ms",
        )?,
    })
}

fn opt_u64_from_db(value: Option<i64>, field: &str) -> DbResult<Option<u64>> {
    value.map(|v| i64_to_u64(v, field)).transpose()
}

fn active_pr_provenance_from_db(
    value: &str,
) -> DbResult<phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance> {
    match value {
        "inferred" => {
            Ok(phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Inferred)
        }
        "pinned" => {
            Ok(phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned)
        }
        other => Err(DbError::Serialization(format!(
            "invalid active PR provenance in database: {other}"
        ))),
    }
}

fn row_to_work_scope_active_pr_selection(
    row: &SqliteRow,
) -> DbResult<WorkScopeActivePrSelectionRow> {
    let repo_owner: Option<String> = row.get("repo_owner");
    let repo_name: Option<String> = row.get("repo_name");
    let pr_number: Option<i64> = row.get("pr_number");
    let selection = match (repo_owner, repo_name, pr_number) {
        (Some(repo_owner), Some(repo_name), Some(pr_number)) => Some(
            phoenix_core::domain::active_pr_selection::ActivePrSelection {
                pr: phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner,
                    repo_name,
                    pr_number: pr_number.cast_unsigned(),
                },
                provenance: active_pr_provenance_from_db(&row.get::<String, _>("provenance"))?,
            },
        ),
        (None, None, None) => None,
        _ => {
            return Err(DbError::Serialization(
                "invalid active PR selection row: partial PR identity".to_string(),
            ));
        }
    };
    let latest_observed_repository_identity: Option<String> =
        row.get("latest_observed_repository_identity");
    let latest_observed_branch_name: Option<String> = row.get("latest_observed_branch_name");
    let latest_observed_branch = match (
        latest_observed_repository_identity,
        latest_observed_branch_name,
    ) {
        (Some(repository_identity), Some(branch_name)) => Some(
            phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                repository_identity,
                branch_name,
            },
        ),
        (None, None) => None,
        _ => {
            return Err(DbError::Serialization(
                "invalid active PR selection row: partial latest observed branch".to_string(),
            ));
        }
    };
    Ok(WorkScopeActivePrSelectionRow {
        work_scope_id: row_work_scope_id(row),
        selection,
        latest_observed_branch,
        inference_generation: row.get::<i64, _>("inference_generation").cast_unsigned(),
        updated_at: row.get("updated_at"),
    })
}

#[must_use]
pub fn qualifies_observed_branch(
    observed: &phoenix_core::domain::observed_branch::LocalGitHeadObservation,
    input: &ObservedBranchQualificationInput,
) -> Option<WorkScopeObservedBranchUpsert> {
    let phoenix_core::domain::observed_branch::LocalGitHeadObservation::NamedBranch {
        repository_identity,
        branch_name,
        head_oid,
    } = observed
    else {
        return None;
    };
    if branch_name == &input.conversation_base_branch {
        return None;
    }
    if head_oid == &input.task_relative_work_base_head_oid {
        return None;
    }
    Some(WorkScopeObservedBranchUpsert {
        repository_identity: repository_identity.clone(),
        branch_name: branch_name.clone(),
        head_oid: head_oid.clone(),
    })
}

fn row_to_work_scope_pr(row: &SqliteRow) -> DbResult<WorkScopePrAssociation> {
    let display_state: String = row.get("display_state");
    Ok(WorkScopePrAssociation {
        work_scope_id: row_work_scope_id(row),
        repo_owner: row.get("repo_owner"),
        repo_name: row.get("repo_name"),
        pr_number: row.get::<i64, _>("pr_number").cast_unsigned(),
        title: row.get("title"),
        url: row.get("url"),
        state: row.get("state"),
        draft: row.get::<bool, _>("draft"),
        display_state: pr_display_state_from_db(&display_state)?,
        base: row.get("base"),
        head: row.get("head"),
        github_updated_at: row.get("github_updated_at"),
        feedback_status: pr_feedback_status_from_db(&row.get::<String, _>("feedback_status"))?,
        first_seen_at: row.get("first_seen_at"),
        last_seen_at: row.get("last_seen_at"),
    })
}

pub fn sort_work_scope_pr_associations(prs: &mut [WorkScopePrAssociation]) {
    prs.sort_by(|a, b| {
        pr_association_rank(&a.display_state)
            .cmp(&pr_association_rank(&b.display_state))
            .then_with(|| b.github_updated_at.cmp(&a.github_updated_at))
            .then_with(|| b.last_seen_at.cmp(&a.last_seen_at))
            .then_with(|| b.pr_number.cmp(&a.pr_number))
    });
}

fn active_pr_identity_from_association(
    pr: &WorkScopePrAssociation,
) -> phoenix_core::domain::active_pr_selection::ActivePrIdentity {
    phoenix_core::domain::active_pr_selection::ActivePrIdentity {
        repo_owner: pr.repo_owner.clone(),
        repo_name: pr.repo_name.clone(),
        pr_number: pr.pr_number,
    }
}

fn is_actionable_pr(pr: &WorkScopePrAssociation) -> bool {
    matches!(
        pr.display_state,
        phoenix_core::domain::pr_display_state::PrDisplayState::Open
            | phoenix_core::domain::pr_display_state::PrDisplayState::Draft
    )
}

fn github_repository_identity(pr: &WorkScopePrAssociation) -> String {
    format!("{}/{}", pr.repo_owner, pr.repo_name)
}

fn github_repository_identity_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn repository_identity_is_structurally_local_path(identity: &str) -> bool {
    let path = std::path::Path::new(identity);
    path.is_absolute()
}

fn latest_branch_repository_matches_pr(
    prs: &[WorkScopePrAssociation],
    branch: &phoenix_core::domain::active_pr_selection::ActivePrBranchContext,
    pr: &WorkScopePrAssociation,
) -> bool {
    let pr_identity = github_repository_identity(pr);
    if github_repository_identity_eq(&branch.repository_identity, &pr_identity) {
        return true;
    }
    if !repository_identity_is_structurally_local_path(&branch.repository_identity) {
        return false;
    }

    let repo_identities: std::collections::BTreeSet<_> = prs
        .iter()
        .map(github_repository_identity)
        .map(|identity| identity.to_ascii_lowercase())
        .collect();
    repo_identities.len() == 1 && repo_identities.contains(&pr_identity.to_ascii_lowercase())
}

fn find_association_by_identity<'a>(
    prs: &'a [WorkScopePrAssociation],
    identity: &phoenix_core::domain::active_pr_selection::ActivePrIdentity,
) -> Option<&'a WorkScopePrAssociation> {
    prs.iter().find(|pr| {
        pr.repo_owner.eq_ignore_ascii_case(&identity.repo_owner)
            && pr.repo_name.eq_ignore_ascii_case(&identity.repo_name)
            && pr.pr_number == identity.pr_number
    })
}

fn infer_active_pr_selection(
    prs: &[WorkScopePrAssociation],
    latest_observed_branch: Option<
        &phoenix_core::domain::active_pr_selection::ActivePrBranchContext,
    >,
    prior_inferred: Option<&phoenix_core::domain::active_pr_selection::ActivePrIdentity>,
) -> Option<phoenix_core::domain::active_pr_selection::ActivePrIdentity> {
    let actionable: Vec<&WorkScopePrAssociation> =
        prs.iter().filter(|pr| is_actionable_pr(pr)).collect();

    if let Some(branch) = latest_observed_branch {
        let mut matching = actionable.iter().copied().filter(|pr| {
            pr.head == branch.branch_name && latest_branch_repository_matches_pr(prs, branch, pr)
        });
        let first = matching.next();
        if let Some(first) = first {
            if matching.next().is_none() {
                return Some(active_pr_identity_from_association(first));
            }
        }
        if actionable.len() == 1
            && actionable[0].head == branch.branch_name
            && latest_branch_repository_matches_pr(prs, branch, actionable[0])
        {
            return Some(active_pr_identity_from_association(actionable[0]));
        }
    }
    if actionable.len() == 1 {
        return Some(active_pr_identity_from_association(actionable[0]));
    }

    let prior_inferred = prior_inferred?;
    let retained = find_association_by_identity(prs, prior_inferred)?;
    if !is_actionable_pr(retained) {
        return None;
    }
    let contradicts_branch = latest_observed_branch.is_some_and(|branch| {
        !latest_branch_repository_matches_pr(prs, branch, retained)
            || retained.head != branch.branch_name
    });
    if contradicts_branch {
        return None;
    }
    Some(prior_inferred.clone())
}

fn pr_association_rank(state: &phoenix_core::domain::pr_display_state::PrDisplayState) -> u8 {
    match state {
        phoenix_core::domain::pr_display_state::PrDisplayState::Open => 0,
        phoenix_core::domain::pr_display_state::PrDisplayState::Draft => 1,
        phoenix_core::domain::pr_display_state::PrDisplayState::Merged => 2,
        phoenix_core::domain::pr_display_state::PrDisplayState::Closed => 3,
    }
}

fn row_work_scope_id(row: &SqliteRow) -> phoenix_core::work_scope::WorkScopeId {
    let raw: String = row.get("work_scope_id");
    phoenix_core::work_scope::WorkScopeId::parse(raw)
        .expect("database CHECK enforces non-empty work_scope_id")
}

fn product_root_reservation_reclaim_before(now: chrono::DateTime<Utc>) -> i64 {
    (now - PRODUCT_ROOT_RESERVATION_RECLAIM_AFTER).timestamp_micros()
}

/// Thread-safe database handle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationCommitOutcome {
    Applied,
    Duplicate,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupParentAction {
    Reconcile,
    Resume,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupParentActionRecord {
    pub action_id: i64,
    pub conversation_id: String,
    pub action: StartupParentAction,
    pub transcript_generation: i64,
    pub created_at: String,
    pub turn_id: Option<phoenix_workflow::TurnAuthorityId>,
    pub turn_generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupParentReconciliation {
    pub conversation_id: String,
}

#[cfg(test)]
#[derive(Debug)]
struct SubAgentCreationTestLatch {
    parent_read: tokio::sync::Notify,
    competing_write_observed: tokio::sync::Notify,
}

#[cfg(test)]
impl SubAgentCreationTestLatch {
    fn new() -> Self {
        Self {
            parent_read: tokio::sync::Notify::new(),
            competing_write_observed: tokio::sync::Notify::new(),
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct CloseFoundationTestLatch {
    pub transaction_entered: tokio::sync::Notify,
    pub release_transaction: tokio::sync::Notify,
}

#[cfg(test)]
impl CloseFoundationTestLatch {
    pub(crate) fn new() -> Self {
        Self {
            transaction_entered: tokio::sync::Notify::new(),
            release_transaction: tokio::sync::Notify::new(),
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
struct SteeringBeginTestLatch {
    before_begin: tokio::sync::Notify,
    allow_begin: tokio::sync::Notify,
    begin_called: tokio::sync::Notify,
}

#[cfg(test)]
impl SteeringBeginTestLatch {
    fn new() -> Self {
        Self {
            before_begin: tokio::sync::Notify::new(),
            allow_begin: tokio::sync::Notify::new(),
            begin_called: tokio::sync::Notify::new(),
        }
    }
}

pub struct Database {
    pool: SqlitePool,
    sqlite_workload_collector: SqliteWorkloadCollector,
    /// Filesystem path of the on-disk DB (empty for in-memory DBs). Retained so
    /// permissions can be re-tightened after migrations create the WAL sidecars.
    path: String,
    dormant_git_repository_catchup_authority_state:
        std::sync::Arc<git_repository_reconciliation::DormantGitRepositoryCatchupAuthorityState>,
    #[cfg(test)]
    sub_agent_creation_test_latch: Option<std::sync::Arc<SubAgentCreationTestLatch>>,
    #[cfg(test)]
    pub(crate) close_foundation_test_latch: Option<std::sync::Arc<CloseFoundationTestLatch>>,
    #[cfg(test)]
    steering_begin_test_latch: Option<std::sync::Arc<SteeringBeginTestLatch>>,
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            sqlite_workload_collector: self.sqlite_workload_collector.clone(),
            path: self.path.clone(),
            dormant_git_repository_catchup_authority_state: self
                .dormant_git_repository_catchup_authority_state
                .clone(),
            #[cfg(test)]
            sub_agent_creation_test_latch: self.sub_agent_creation_test_latch.clone(),
            #[cfg(test)]
            close_foundation_test_latch: self.close_foundation_test_latch.clone(),
            #[cfg(test)]
            steering_begin_test_latch: self.steering_begin_test_latch.clone(),
        }
    }
}

fn sqlite_constraint_code_is(code: Option<&str>, expected: &str) -> bool {
    code == Some(expected)
}

fn is_sqlite_unique_constraint(error: &dyn sqlx::error::DatabaseError) -> bool {
    sqlite_constraint_code_is(error.code().as_deref(), "2067")
}

fn is_sqlite_primary_key_constraint(error: &dyn sqlx::error::DatabaseError) -> bool {
    sqlite_constraint_code_is(error.code().as_deref(), "1555")
}

async fn insert_creation_job_files_tx(
    tx: &mut Transaction<'_, Sqlite>,
    job: &InsertConversationCreationJob,
) -> DbResult<()> {
    for (ordinal, file) in job.intent.files.iter().enumerate() {
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| DbError::Serialization("attachment ordinal exceeds i64".to_string()))?;
        let size_bytes = i64::try_from(file.size_bytes)
            .map_err(|_| DbError::Serialization("attachment size exceeds i64".to_string()))?;
        sqlx::query(
            "INSERT INTO conversation_creation_job_files (
                job_id, ordinal, original_name, media_type, size_bytes, stored_path
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&job.id)
        .bind(ordinal)
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(size_bytes)
        .bind(&file.stored_path)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn insert_creation_job_images_tx(
    tx: &mut Transaction<'_, Sqlite>,
    job: &InsertConversationCreationJob,
) -> DbResult<()> {
    for (ordinal, image) in job.intent.images.iter().enumerate() {
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| DbError::Serialization("image ordinal exceeds i64".to_string()))?;
        sqlx::query(
            "INSERT INTO conversation_creation_job_images (
                job_id, ordinal, media_type, data
             ) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&job.id)
        .bind(ordinal)
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

const GIT_POINTER_FORMAT_OVERHEAD: u64 = b"gitdir: \r\n".len() as u64;
const MAX_GIT_POINTER_BYTES: u64 = libc::PATH_MAX as u64 + GIT_POINTER_FORMAT_OVERHEAD;

enum ExpectedParentScope<'a> {
    NotChecked,
    Snapshot(Option<&'a WorkScopeId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalEvidenceTransactionCut {
    None,
    BeforeCommit,
    AfterCommit,
}

fn parent_creation_values(
    row: Option<SqliteRow>,
) -> DbResult<(
    Option<WorkScopeId>,
    Option<ModelEffort>,
    Option<phoenix_core::domain::product_conversation::ProductConversationId>,
)> {
    let Some(row) = row else {
        return Ok((None, None, None));
    };
    let scope = row
        .try_get::<Option<String>, _>("work_scope_id")?
        .map(WorkScopeId::parse)
        .transpose()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let effort = row
        .try_get::<Option<String>, _>("effort")?
        .map(|value| ModelEffort::from_str(&value))
        .transpose()
        .map_err(DbError::Serialization)?;
    let product_conversation_id = row
        .try_get::<String, _>("product_conversation_id")?
        .parse::<phoenix_core::domain::product_conversation::ProductConversationId>()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    Ok((scope, effort, Some(product_conversation_id)))
}

impl Database {
    /// Access the underlying connection pool (for migrations and testing).
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub fn fts_retriever(&self) -> retrieval::Fts5Retriever {
        retrieval::Fts5Retriever::new(self.pool.clone(), self.sqlite_workload_collector.clone())
    }

    #[must_use]
    pub fn workflow_repository(&self) -> workflow::WorkflowRepository {
        workflow::WorkflowRepository::with_sqlite_workload_collector(
            self.pool.clone(),
            self.sqlite_workload_collector.clone(),
        )
    }

    #[must_use]
    pub fn wake_repository(&self) -> workflow::wake::WakeRepository {
        workflow::wake::WakeRepository::with_sqlite_workload_collector(
            self.pool.clone(),
            self.sqlite_workload_collector.clone(),
        )
    }

    #[cfg(test)]
    #[must_use]
    pub fn sqlite_workload_aggregate_report(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadAggregateReport {
        self.sqlite_workload_collector
            .aggregate_report(window, now_unix_micros)
    }

    #[must_use]
    pub fn sample_sqlite_workload_aggregate_report(
        &self,
        window: SqliteSnapshotWindow,
    ) -> SampledSqliteWorkloadAggregateReport {
        self.sqlite_workload_collector.aggregate_report_now(window)
    }

    fn sqlite_telemetry(
        &self,
        operation: SqliteOperation,
        category: SqliteWorkloadCategory,
        access: SqliteAccessKind,
    ) -> SqliteTelemetry {
        SqliteTelemetry::with_collector(
            operation,
            category,
            access,
            self.sqlite_workload_collector.clone(),
        )
    }

    #[allow(
        dead_code,
        reason = "task 59004 consumes the existing dormant catch-up seam"
    )]
    pub(crate) async fn catch_up_dormant_git_repositories(
        &self,
        permit: DormantGitRepositoryCatchupPermit,
    ) -> DbResult<DormantGitRepositoryCatchupOutcome> {
        git_repository_reconciliation::catch_up_dormant_git_repositories(self, permit).await
    }

    #[allow(dead_code, reason = "task 59004 consumes the dormant readiness facade")]
    pub(crate) async fn validate_dormant_git_repository_readiness(
        &self,
        receipt: git_repository_reconciliation::DormantGitRepositoryCatchupReceipt,
    ) -> DbResult<git_repository_reconciliation::DormantGitRepositoryCanonicalReadinessEvidence>
    {
        git_repository_reconciliation::validate_dormant_git_repository_readiness(self, receipt)
            .await
    }

    pub(crate) fn dormant_git_repository_target_binding(
        &self,
    ) -> git_repository_reconciliation::DormantGitRepositoryTargetBinding {
        git_repository_reconciliation::DormantGitRepositoryTargetBinding::for_state(
            self.dormant_git_repository_catchup_authority_state.clone(),
        )
    }

    fn new_with_generated_target_binding(
        pool: SqlitePool,
        path: String,
        sqlite_workload_collector: SqliteWorkloadCollector,
    ) -> Self {
        Self {
            pool,
            sqlite_workload_collector,
            path,
            dormant_git_repository_catchup_authority_state: std::sync::Arc::new(
                git_repository_reconciliation::DormantGitRepositoryCatchupAuthorityState::default(),
            ),
            #[cfg(test)]
            sub_agent_creation_test_latch: None,
            #[cfg(test)]
            close_foundation_test_latch: None,
            #[cfg(test)]
            steering_begin_test_latch: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_pool_for_tests(pool: SqlitePool, path: String) -> Self {
        Self::new_with_generated_target_binding(pool, path, SqliteWorkloadCollector::new())
    }

    /// Re-tighten the on-disk DB file and its `-wal`/`-shm` sidecars to 0600.
    ///
    /// Idempotent and best-effort: a `chmod` failure is logged at debug and
    /// never fails. Call after migrations have run, since the numbered
    /// migrations are what create the WAL sidecars that an early chmod in
    /// `open` cannot see. A no-op for in-memory DBs (empty path) and on
    /// non-Unix platforms.
    pub fn restrict_file_permissions(&self) {
        if self.path.is_empty() {
            return;
        }
        restrict_db_permissions(&self.path);
    }

    fn observe_worktree_fingerprint(worktree_path: &str) -> Option<String> {
        use std::fmt::Write as _;
        use std::io::Read as _;
        use std::os::unix::fs::MetadataExt as _;

        let marker = std::path::Path::new(worktree_path).join(".git");
        let metadata = std::fs::symlink_metadata(&marker).ok()?;
        let marker_is_file = metadata.is_file();
        let marker_bytes = if marker_is_file {
            if metadata.len() > MAX_GIT_POINTER_BYTES {
                return None;
            }
            let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).ok()?);
            std::fs::File::open(&marker)
                .ok()?
                .take(MAX_GIT_POINTER_BYTES + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            if u64::try_from(bytes.len()).ok()? > MAX_GIT_POINTER_BYTES {
                return None;
            }
            let pointer = std::str::from_utf8(&bytes).ok()?;
            let git_dir = pointer
                .strip_suffix("\r\n")
                .or_else(|| pointer.strip_suffix('\n'))
                .unwrap_or(pointer)
                .strip_prefix("gitdir: ")?;
            if git_dir.is_empty() || git_dir.contains('\n') || git_dir.contains('\r') {
                return None;
            }
            git_dir.as_bytes().to_vec()
        } else if metadata.is_dir() {
            Vec::new()
        } else {
            return None;
        };
        let mut encoded = String::with_capacity(marker_bytes.len() * 2);
        for byte in marker_bytes {
            write!(&mut encoded, "{byte:02x}").ok()?;
        }
        let created_nanos = metadata
            .created()
            .ok()
            .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos().to_string());
        if created_nanos.is_none() && !marker_is_file {
            return None;
        }
        let created_nanos = created_nanos.unwrap_or_else(|| "unavailable".to_string());
        Some(format!(
            "git_admin_incarnation_v1:{}:{}:{created_nanos}:{encoded}",
            metadata.dev(),
            metadata.ino()
        ))
    }

    async fn insert_work_scope_tx(
        tx: &mut Transaction<'_, Sqlite>,
        scope_id: &WorkScopeId,
        authority_kind: AuthorityKind,
        context: EnvironmentContext,
        now: &str,
    ) -> DbResult<()> {
        let (kind, cwd, worktree_path, branch_name, base_branch) =
            Self::environment_columns(context);
        let observed_fingerprint = worktree_path
            .as_deref()
            .and_then(Self::observe_worktree_fingerprint);
        let prewrite_fingerprint = worktree_path
            .as_deref()
            .and_then(Self::observe_worktree_fingerprint);
        let worktree_fingerprint = if observed_fingerprint == prewrite_fingerprint {
            observed_fingerprint
        } else {
            None
        };
        let worktree_id = worktree_fingerprint
            .as_ref()
            .map(|_| uuid::Uuid::new_v4().to_string());
        sqlx::query(
            "INSERT INTO work_scopes (
                 id, authority_kind, lifecycle, environment_kind, cwd,
                 worktree_path, branch_name, base_branch, created_at, updated_at,
                 worktree_id, worktree_fingerprint
             ) VALUES (
                 ?1, ?2, 'active', ?3, ?4, ?5, ?6, ?7, ?8, ?8,
                 CASE WHEN ?10 IS NOT NULL AND NOT EXISTS (
                     SELECT 1 FROM work_scopes WHERE worktree_fingerprint = ?10
                 ) THEN ?9 END,
                 CASE WHEN ?10 IS NOT NULL AND NOT EXISTS (
                     SELECT 1 FROM work_scopes WHERE worktree_fingerprint = ?10
                 ) THEN ?10 END
             )",
        )
        .bind(scope_id.as_str())
        .bind(authority_kind.as_str())
        .bind(kind)
        .bind(cwd)
        .bind(&worktree_path)
        .bind(branch_name)
        .bind(base_branch)
        .bind(now)
        .bind(worktree_id)
        .bind(&worktree_fingerprint)
        .execute(&mut **tx)
        .await?;
        let commit_fingerprint = worktree_path
            .as_deref()
            .and_then(Self::observe_worktree_fingerprint);
        if worktree_fingerprint.is_some() && worktree_fingerprint != commit_fingerprint {
            sqlx::query(
                "UPDATE work_scopes
                 SET worktree_id = NULL, worktree_fingerprint = NULL, updated_at = ?2
                 WHERE id = ?1",
            )
            .bind(scope_id.as_str())
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    async fn update_work_scope_environment_tx(
        tx: &mut Transaction<'_, Sqlite>,
        scope_id: &WorkScopeId,
        context: EnvironmentContext,
        now: &str,
    ) -> DbResult<()> {
        let (kind, cwd, worktree_path, branch_name, base_branch) =
            Self::environment_columns(context);
        let observed_fingerprint = worktree_path
            .as_deref()
            .and_then(Self::observe_worktree_fingerprint);
        let existing =
            sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
                "SELECT environment_kind, worktree_path, worktree_id, worktree_fingerprint
             FROM work_scopes WHERE id = ?1",
            )
            .bind(scope_id.as_str())
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                DbError::Serialization(format!(
                    "work scope {scope_id} has no normalized environment"
                ))
            })?;
        let (mut worktree_id, mut observed_fingerprint) = match (kind, observed_fingerprint) {
            ("allocated_worktree", Some(fingerprint)) => {
                let id = if existing.3.as_ref() == Some(&fingerprint) {
                    existing
                        .2
                        .clone()
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
                } else {
                    uuid::Uuid::new_v4().to_string()
                };
                (Some(id), Some(fingerprint))
            }
            _ => (None, None),
        };
        let prewrite_fingerprint = worktree_path
            .as_deref()
            .and_then(Self::observe_worktree_fingerprint);
        if observed_fingerprint != prewrite_fingerprint {
            worktree_id = None;
            observed_fingerprint = None;
        }
        let result = sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = ?1, cwd = ?2, worktree_path = ?3,
                 branch_name = ?4, base_branch = ?5, updated_at = ?6,
                 worktree_id = CASE WHEN ?9 IS NULL OR NOT EXISTS (
                     SELECT 1 FROM work_scopes owner
                     WHERE owner.worktree_fingerprint = ?9 AND owner.id <> ?7
                 ) THEN ?8 END,
                 worktree_fingerprint = CASE WHEN ?9 IS NULL OR NOT EXISTS (
                     SELECT 1 FROM work_scopes owner
                     WHERE owner.worktree_fingerprint = ?9 AND owner.id <> ?7
                 ) THEN ?9 END
             WHERE id = ?7",
        )
        .bind(kind)
        .bind(cwd)
        .bind(&worktree_path)
        .bind(branch_name)
        .bind(base_branch)
        .bind(now)
        .bind(scope_id.as_str())
        .bind(worktree_id)
        .bind(observed_fingerprint)
        .execute(&mut **tx)
        .await?;
        let persisted_fingerprint: Option<String> =
            sqlx::query_scalar("SELECT worktree_fingerprint FROM work_scopes WHERE id = ?1")
                .bind(scope_id.as_str())
                .fetch_one(&mut **tx)
                .await?;
        let commit_fingerprint = worktree_path
            .as_deref()
            .and_then(Self::observe_worktree_fingerprint);
        if persisted_fingerprint.is_some() && persisted_fingerprint != commit_fingerprint {
            sqlx::query(
                "UPDATE work_scopes
                 SET worktree_id = NULL, worktree_fingerprint = NULL, updated_at = ?2
                 WHERE id = ?1",
            )
            .bind(scope_id.as_str())
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
        if result.rows_affected() != 1 {
            return Err(DbError::Serialization(format!(
                "work scope {scope_id} has no normalized environment"
            )));
        }
        Ok(())
    }

    fn environment_columns(
        context: EnvironmentContext,
    ) -> (
        &'static str,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        match context {
            EnvironmentContext::AllocatedWorktree {
                cwd,
                worktree_path,
                branch_name,
                base_branch,
            } => (
                "allocated_worktree",
                Some(cwd),
                Some(worktree_path),
                branch_name,
                base_branch,
            ),
            EnvironmentContext::UnownedCwd { cwd } => ("unowned_cwd", Some(cwd), None, None, None),
            EnvironmentContext::None => ("none", None, None, None, None),
        }
    }

    fn environment_for_mode(cwd: &str, cm: &ConvModeCols<'_>) -> EnvironmentContext {
        match (cm.kind, cm.worktree_path, cm.branch_name, cm.base_branch) {
            ("work" | "branch", Some(worktree_path), Some(branch_name), Some(base_branch)) => {
                EnvironmentContext::AllocatedWorktree {
                    cwd: cwd.to_string(),
                    worktree_path: worktree_path.to_string(),
                    branch_name: Some(branch_name.to_string()),
                    base_branch: Some(base_branch.to_string()),
                }
            }
            ("explore", Some(worktree_path), _, _) => EnvironmentContext::AllocatedWorktree {
                cwd: worktree_path.to_string(),
                worktree_path: worktree_path.to_string(),
                branch_name: None,
                base_branch: None,
            },
            _ if !cwd.is_empty() => EnvironmentContext::UnownedCwd {
                cwd: cwd.to_string(),
            },
            _ => EnvironmentContext::None,
        }
    }

    fn new_scope_for_conversation(
        cwd: &str,
        cm: &ConvModeCols<'_>,
    ) -> (WorkScopeId, AuthorityKind, EnvironmentContext) {
        let scope_id = WorkScopeId::new();
        let context = Self::environment_for_mode(cwd, cm);
        let authority_kind = Self::authority_for_mode(cm);
        (scope_id, authority_kind, context)
    }

    fn authority_for_mode(cm: &ConvModeCols<'_>) -> AuthorityKind {
        match cm.kind {
            "work" | "branch" => AuthorityKind::Work,
            _ => AuthorityKind::RestrictedExplore,
        }
    }

    async fn work_scope_exists(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<bool> {
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM work_scopes WHERE id = ?1)")
                .bind(scope.as_str())
                .fetch_one(&self.pool)
                .await?;
        Ok(exists != 0)
    }

    async fn conversation_retirement_blocker(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        scope_id: &WorkScopeId,
    ) -> DbResult<Option<WorkScopeRetirementBlocker>> {
        let owners = sqlx::query(
            "SELECT runtime_role, state, continued_in_conv_id
             FROM conversations
             WHERE work_scope_id = ?1 AND archived = 0",
        )
        .bind(scope_id.as_str())
        .fetch_all(&mut **tx)
        .await?;
        for owner in owners {
            let role_value: String = owner.try_get("runtime_role")?;
            let runtime_role = RuntimeRole::from_db_str(&role_value).ok_or_else(|| {
                DbError::Serialization(format!("unknown runtime role {role_value}"))
            })?;
            let state: ConvState = serde_json::from_str(&owner.try_get::<String, _>("state")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let continued_in_conv_id: Option<String> = owner.try_get("continued_in_conv_id")?;
            match runtime_role {
                RuntimeRole::User
                    if !state.is_terminal()
                        || (matches!(state, ConvState::ContextExhausted { .. })
                            && continued_in_conv_id.is_none()) =>
                {
                    return Ok(Some(WorkScopeRetirementBlocker::CurrentUserOwner));
                }
                RuntimeRole::SubAgent if !state.is_terminal() => {
                    return Ok(Some(WorkScopeRetirementBlocker::ActiveSubAgent));
                }
                RuntimeRole::User | RuntimeRole::SubAgent | RuntimeRole::Coordinator => {}
            }
        }
        Ok(None)
    }

    /// Retire a scope only when both runtime and durable ownership inventories
    /// prove that no obligation remains. Conversation history and scope-owned
    /// observations are preserved; retirement changes lifecycle only.
    /// # Errors
    /// Returns a [`DbError`] when the reason is empty, the scope is missing,
    /// or a durable predicate cannot be read or updated transactionally.
    pub async fn retire_work_scope(
        &self,
        precondition: WorkScopeRetirementPrecondition,
        reason: &str,
    ) -> DbResult<WorkScopeRetirementOutcome> {
        if reason.trim().is_empty() {
            return Err(DbError::Serialization(
                "work scope retirement reason must not be empty".to_string(),
            ));
        }
        let scope_id = precondition.scope_id();
        let mut tx = self.pool.begin().await?;
        let lifecycle =
            sqlx::query_scalar::<_, String>("SELECT lifecycle FROM work_scopes WHERE id = ?1")
                .bind(scope_id.as_str())
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| DbError::Serialization(format!("unknown work scope {scope_id}")))?;
        if lifecycle == "retired" {
            tx.rollback().await?;
            return Ok(WorkScopeRetirementOutcome::AlreadyRetired);
        }

        if let Some(blocker) = Self::conversation_retirement_blocker(&mut tx, scope_id).await? {
            tx.rollback().await?;
            return Ok(WorkScopeRetirementOutcome::Blocked(blocker));
        }

        let blockers = [
            (
                WorkScopeRetirementBlocker::UserSuccessor,
                "SELECT EXISTS(
                    SELECT 1 FROM conversations successor
                    JOIN conversations predecessor ON predecessor.continued_in_conv_id = successor.id
                    WHERE successor.work_scope_id = ?1 AND successor.runtime_role = 'user'
                      AND successor.archived = 0
                 )",
            ),
            (
                WorkScopeRetirementBlocker::PendingWakeOrWorkflow,
                "SELECT EXISTS(
                    SELECT 1 FROM wake_bindings b
                    JOIN workflows w ON w.workflow_id = b.workflow_id
                    WHERE b.work_scope_id = ?1
                      AND (
                        b.resolved_at IS NULL
                        OR w.status IN ('Active', 'Cancelling', 'ManualResolution', 'Incompatible', 'DeletionPending')
                        OR EXISTS (
                            SELECT 1 FROM workflow_deliveries d
                            WHERE d.workflow_id = b.workflow_id
                              AND (d.status IN ('Pending', 'Deferred') OR d.runtime_acceptance_status = 'Owed')
                        )
                      )
                 )",
            ),
        ];
        for (blocker, query) in blockers {
            if sqlx::query_scalar::<_, i64>(query)
                .bind(scope_id.as_str())
                .fetch_one(&mut *tx)
                .await?
                != 0
            {
                tx.rollback().await?;
                return Ok(WorkScopeRetirementOutcome::Blocked(blocker));
            }
        }

        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE work_scopes
             SET lifecycle = 'retired', retired_at = ?1, retired_reason = ?2, updated_at = ?1
             WHERE id = ?3 AND lifecycle = 'active'",
        )
        .bind(&now)
        .bind(reason)
        .bind(scope_id.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(WorkScopeRetirementOutcome::AlreadyRetired);
        }
        tx.commit().await?;
        Ok(WorkScopeRetirementOutcome::Retired)
    }

    async fn ensure_work_scope_id(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<phoenix_core::work_scope::WorkScopeId> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO work_scopes (id, authority_kind, lifecycle, created_at, updated_at)
             VALUES (?1, 'restricted_explore', 'active', ?2, ?2)
             ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at",
        )
        .bind(scope.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(scope.clone())
    }

    /// # Errors
    /// Returns a [`DbError`] when scope creation or PR observation persistence fails.
    pub async fn upsert_work_scope_pr_observations(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        observations: &[WorkScopePrObservation],
    ) -> DbResult<phoenix_core::work_scope::WorkScopeId> {
        let work_scope_id = self.ensure_work_scope_id(scope).await?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        for pr in observations {
            sqlx::query(
                "INSERT INTO work_scope_pr_associations (
                    work_scope_id, repo_owner, repo_name, pr_number, title, url, state, draft,
                    display_state, base, head, github_updated_at, feedback_status, first_seen_at, last_seen_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'open', ?13, ?13)
                 ON CONFLICT(work_scope_id, repo_owner, repo_name, pr_number) DO UPDATE SET
                    title = excluded.title,
                    url = excluded.url,
                    state = excluded.state,
                    draft = excluded.draft,
                    display_state = excluded.display_state,
                    base = excluded.base,
                    head = excluded.head,
                    github_updated_at = excluded.github_updated_at,
                    last_seen_at = excluded.last_seen_at",
            )
            .bind(work_scope_id.as_str())
            .bind(&pr.repo_owner)
            .bind(&pr.repo_name)
            .bind(pr.pr_number.cast_signed())
            .bind(&pr.title)
            .bind(&pr.url)
            .bind(&pr.state)
            .bind(pr.draft)
            .bind(pr_display_state_db(&pr.display_state))
            .bind(&pr.base)
            .bind(&pr.head)
            .bind(&pr.github_updated_at)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(work_scope_id)
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_work_scope_pr_associations(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<Vec<WorkScopePrAssociation>> {
        if !self.work_scope_exists(scope).await? {
            return Ok(Vec::new());
        }
        let work_scope_id = scope;
        let rows = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, title, url, state, draft,
                    display_state, base, head, github_updated_at, feedback_status, first_seen_at, last_seen_at
             FROM work_scope_pr_associations
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row_to_work_scope_pr(&row))
            .collect()
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn primary_work_scope_pr_association(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<Option<WorkScopePrAssociation>> {
        let mut prs = self.list_work_scope_pr_associations(scope).await?;
        sort_work_scope_pr_associations(&mut prs);
        Ok(prs.into_iter().next())
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn primary_work_scope_pr_associations(
        &self,
        scopes: &[phoenix_core::work_scope::WorkScopeId],
    ) -> DbResult<std::collections::HashMap<String, WorkScopePrAssociation>> {
        if scopes.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut query = sqlx::QueryBuilder::new(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, title, url, state, draft,
                    display_state, base, head, github_updated_at, feedback_status, first_seen_at, last_seen_at
             FROM work_scope_pr_associations
             WHERE work_scope_id IN ",
        );
        query.push_tuples(scopes.iter(), |mut tuple, scope| {
            tuple.push_bind(scope.as_str());
        });
        let rows = query.build().fetch_all(&self.pool).await?;
        let mut grouped: std::collections::HashMap<String, Vec<WorkScopePrAssociation>> =
            std::collections::HashMap::new();
        for row in rows {
            let scope_id =
                phoenix_core::work_scope::WorkScopeId::parse(row.get::<String, _>("work_scope_id"))
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            let stable_key =
                phoenix_core::work_scope::ResourceScopeKey::Work(scope_id).stable_key();
            grouped
                .entry(stable_key)
                .or_default()
                .push(row_to_work_scope_pr(&row)?);
        }
        let mut result = std::collections::HashMap::new();
        for (stable_key, mut prs) in grouped {
            sort_work_scope_pr_associations(&mut prs);
            if let Some(primary) = prs.into_iter().next() {
                result.insert(stable_key, primary);
            }
        }
        Ok(result)
    }

    /// # Errors
    /// Returns a [`DbError`] when scope lookup or feedback persistence fails.
    pub async fn update_work_scope_pr_feedback_status(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        repo_owner: &str,
        repo_name: &str,
        pr_number: u64,
        status: PrFeedbackStatus,
    ) -> DbResult<()> {
        if !self.work_scope_exists(scope).await? {
            return Ok(());
        }
        let work_scope_id = scope;
        sqlx::query(
            "UPDATE work_scope_pr_associations
             SET feedback_status = ?1
             WHERE work_scope_id = ?2 AND repo_owner = ?3 AND repo_name = ?4 AND pr_number = ?5",
        )
        .bind(pr_feedback_status_db(status))
        .bind(work_scope_id.as_str())
        .bind(repo_owner)
        .bind(repo_name)
        .bind(pr_number.cast_signed())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn upsert_work_scope_pr_feedback_baseline(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        baseline: &WorkScopePrFeedbackBaselineInput,
    ) -> DbResult<phoenix_core::work_scope::WorkScopeId> {
        let work_scope_id = self.ensure_work_scope_id(scope).await?;
        let mut tx = self.pool.begin().await?;
        let mut feedback_identities = baseline.feedback_identities.clone();
        feedback_identities.sort();
        feedback_identities.dedup();
        let identities = serde_json::to_string(&feedback_identities)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let mut feedback_fingerprints = baseline.feedback_fingerprints.clone();
        feedback_fingerprints.sort();
        feedback_fingerprints.dedup();
        let fingerprints = serde_json::to_string(&feedback_fingerprints)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        sqlx::query(
            "INSERT INTO work_scope_pr_feedback_baselines (
                work_scope_id, repo_owner, repo_name, pr_number, captured_at, github_updated_at, feedback_identities, feedback_fingerprints
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(work_scope_id, repo_owner, repo_name, pr_number) DO UPDATE SET
                captured_at = excluded.captured_at,
                github_updated_at = excluded.github_updated_at,
                feedback_identities = excluded.feedback_identities,
                feedback_fingerprints = excluded.feedback_fingerprints",
        )
        .bind(work_scope_id.as_str())
        .bind(&baseline.repo_owner)
        .bind(&baseline.repo_name)
        .bind(baseline.pr_number.cast_signed())
        .bind(&baseline.captured_at)
        .bind(&baseline.github_updated_at)
        .bind(identities)
        .bind(fingerprints)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(work_scope_id)
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn upsert_work_scope_observed_branch(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        observed: &WorkScopeObservedBranchUpsert,
    ) -> DbResult<phoenix_core::work_scope::WorkScopeId> {
        let work_scope_id = self.ensure_work_scope_id(scope).await?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO work_scope_observed_branches (
                work_scope_id, repository_identity, branch_name, first_observed_head_oid,
                last_observed_head_oid, first_observed_at, last_observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5)
             ON CONFLICT(work_scope_id, repository_identity, branch_name) DO UPDATE SET
                last_observed_head_oid = excluded.last_observed_head_oid,
                last_observed_at = excluded.last_observed_at",
        )
        .bind(work_scope_id.as_str())
        .bind(&observed.repository_identity)
        .bind(&observed.branch_name)
        .bind(&observed.head_oid)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(work_scope_id)
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_work_scope_observed_branches(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<Vec<WorkScopeObservedBranch>> {
        if !self.work_scope_exists(scope).await? {
            return Ok(Vec::new());
        }
        let work_scope_id = scope;
        let rows = sqlx::query(
            "SELECT work_scope_id, repository_identity, branch_name, first_observed_head_oid,
                    last_observed_head_oid, first_observed_at, last_observed_at
             FROM work_scope_observed_branches
             WHERE work_scope_id = ?1
             ORDER BY last_observed_at DESC, branch_name ASC",
        )
        .bind(work_scope_id.as_str())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row_to_work_scope_observed_branch(&row))
            .collect())
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn active_work_scope_pr_selection(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>> {
        if !self.work_scope_exists(scope).await? {
            return Ok(None);
        }
        let work_scope_id = scope;
        let row = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, provenance,
                    latest_observed_repository_identity, latest_observed_branch_name,
                    inference_generation, updated_at
             FROM work_scope_active_pr_selection
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            row_to_work_scope_active_pr_selection(&row).map(|row| {
                phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                    selection: row.selection,
                    inference_generation: row.inference_generation,
                }
            })
        })
        .transpose()
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn pin_active_work_scope_pr_selection(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        pr: &phoenix_core::domain::active_pr_selection::ActivePrIdentity,
    ) -> DbResult<phoenix_core::domain::active_pr_selection::ActivePrSelectionState> {
        let work_scope_id = self.ensure_work_scope_id(scope).await?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT 1
             FROM work_scope_pr_associations
             WHERE work_scope_id = ?1 AND repo_owner = ?2 AND repo_name = ?3 AND pr_number = ?4",
        )
        .bind(work_scope_id.as_str())
        .bind(&pr.repo_owner)
        .bind(&pr.repo_name)
        .bind(pr.pr_number.cast_signed())
        .fetch_one(&mut *tx)
        .await?;
        let latest = sqlx::query(
            "SELECT repository_identity, branch_name
             FROM work_scope_observed_branches
             WHERE work_scope_id = ?1
             ORDER BY last_observed_at DESC, branch_name ASC
             LIMIT 1",
        )
        .bind(work_scope_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .map(
            |row| phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                repository_identity: row.get("repository_identity"),
                branch_name: row.get("branch_name"),
            },
        );
        sqlx::query(
            "INSERT INTO work_scope_active_pr_selection (
                work_scope_id, repo_owner, repo_name, pr_number, provenance,
                latest_observed_repository_identity, latest_observed_branch_name,
                inference_generation, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'pinned', ?5, ?6, 2, ?7)
             ON CONFLICT(work_scope_id) DO UPDATE SET
                repo_owner = excluded.repo_owner,
                repo_name = excluded.repo_name,
                pr_number = excluded.pr_number,
                provenance = excluded.provenance,
                latest_observed_repository_identity = excluded.latest_observed_repository_identity,
                latest_observed_branch_name = excluded.latest_observed_branch_name,
                inference_generation = work_scope_active_pr_selection.inference_generation + 1,
                updated_at = excluded.updated_at",
        )
        .bind(work_scope_id.as_str())
        .bind(&pr.repo_owner)
        .bind(&pr.repo_name)
        .bind(pr.pr_number.cast_signed())
        .bind(latest.as_ref().map(|b| b.repository_identity.as_str()))
        .bind(latest.as_ref().map(|b| b.branch_name.as_str()))
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        let persisted = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, provenance,
                    latest_observed_repository_identity, latest_observed_branch_name,
                    inference_generation, updated_at
             FROM work_scope_active_pr_selection
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let persisted = row_to_work_scope_active_pr_selection(&persisted)?;
        Ok(
            phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                selection: persisted.selection,
                inference_generation: persisted.inference_generation,
            },
        )
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn clear_active_work_scope_pr_pin(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        input: &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput,
    ) -> DbResult<Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>> {
        if !self.work_scope_exists(scope).await? {
            return Ok(None);
        }
        let work_scope_id = scope;
        self.clear_active_work_scope_pr_pin_for_scope_id(work_scope_id, input)
            .await
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn derive_active_work_scope_pr_selection(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        input: &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput,
        expected_generation: Option<u64>,
    ) -> DbResult<Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>> {
        if !self.work_scope_exists(scope).await? {
            return Ok(None);
        }
        let work_scope_id = scope;
        self.derive_active_work_scope_pr_selection_for_scope_id(
            work_scope_id,
            input,
            expected_generation,
            None,
            true,
        )
        .await
    }
    async fn clear_active_work_scope_pr_pin_for_scope_id(
        &self,
        work_scope_id: &phoenix_core::work_scope::WorkScopeId,
        input: &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput,
    ) -> DbResult<Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>> {
        let mut tx = self.pool.begin().await?;
        let persisted = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, provenance,
                    latest_observed_repository_identity, latest_observed_branch_name,
                    inference_generation, updated_at
             FROM work_scope_active_pr_selection
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| row_to_work_scope_active_pr_selection(&row))
        .transpose()?;
        let clear_target = persisted.as_ref().and_then(|row| {
            row.selection.as_ref().and_then(|selection| {
                (selection.provenance
                    == phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned)
                    .then_some((selection.clone(), row.inference_generation))
            })
        });
        let durable_input = if input.latest_observed_branch.is_none() {
            let row = sqlx::query(
                "SELECT repository_identity, branch_name
                 FROM work_scope_observed_branches
                 WHERE work_scope_id = ?1
                 ORDER BY last_observed_at DESC, branch_name ASC
                 LIMIT 1",
            )
            .bind(work_scope_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                latest_observed_branch: row.map(|row| {
                    phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                        repository_identity: row.get("repository_identity"),
                        branch_name: row.get("branch_name"),
                    }
                }),
            }
        } else {
            input.clone()
        };
        tx.commit().await?;
        self.derive_active_work_scope_pr_selection_for_scope_id(
            work_scope_id,
            &durable_input,
            persisted.as_ref().map(|row| row.inference_generation),
            clear_target.as_ref().map(|(selection, _)| selection),
            false,
        )
        .await
    }

    async fn active_pr_selection_state_for_scope_id(
        &self,
        work_scope_id: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>> {
        let row = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, provenance,
                    latest_observed_repository_identity, latest_observed_branch_name,
                    inference_generation, updated_at
             FROM work_scope_active_pr_selection
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| row_to_work_scope_active_pr_selection(&row))
            .transpose()
            .map(|state| {
                state.map(
                    |row| phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                        selection: row.selection,
                        inference_generation: row.inference_generation,
                    },
                )
            })
    }

    #[allow(clippy::too_many_lines)]
    async fn derive_active_work_scope_pr_selection_for_scope_id(
        &self,
        work_scope_id: &phoenix_core::work_scope::WorkScopeId,
        input: &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput,
        expected_generation: Option<u64>,
        clear_pin_target: Option<&phoenix_core::domain::active_pr_selection::ActivePrSelection>,
        allow_pinned_short_circuit: bool,
    ) -> DbResult<Option<phoenix_core::domain::active_pr_selection::ActivePrSelectionState>> {
        let mut tx = self.pool.begin().await?;
        let persisted = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, provenance,
                    latest_observed_repository_identity, latest_observed_branch_name,
                    inference_generation, updated_at
             FROM work_scope_active_pr_selection
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| row_to_work_scope_active_pr_selection(&row))
        .transpose()?;

        if let Some(expected_generation) = expected_generation {
            let current_generation = persisted.as_ref().map_or(0, |row| row.inference_generation);
            if current_generation != expected_generation {
                tx.commit().await?;
                return Ok(persisted.map(|row| {
                    phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                        selection: row.selection,
                        inference_generation: row.inference_generation,
                    }
                }));
            }
        }

        if let Some(clear_pin_target) = clear_pin_target {
            let Some(current) = persisted.as_ref().and_then(|row| row.selection.as_ref()) else {
                tx.commit().await?;
                return Ok(persisted.map(|row| {
                    phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                        selection: row.selection,
                        inference_generation: row.inference_generation,
                    }
                }));
            };
            if current.provenance
                != phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned
                || current != clear_pin_target
            {
                tx.commit().await?;
                return Ok(persisted.map(|row| {
                    phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                        selection: row.selection,
                        inference_generation: row.inference_generation,
                    }
                }));
            }
        }

        let associations = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, title, url, state, draft,
                    display_state, base, head, github_updated_at, feedback_status, first_seen_at, last_seen_at
             FROM work_scope_pr_associations
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| row_to_work_scope_pr(&row))
        .collect::<DbResult<Vec<_>>>()?;

        let latest_observed_branch = input.latest_observed_branch.clone().or_else(|| {
            persisted
                .as_ref()
                .and_then(|row| row.latest_observed_branch.clone())
        });

        if persisted
            .as_ref()
            .and_then(|row| row.selection.as_ref())
            .is_some_and(|selection| {
                selection.provenance
                == phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned
                && allow_pinned_short_circuit
                && find_association_by_identity(&associations, &selection.pr).is_some()
            })
        {
            tx.commit().await?;
            return Ok(persisted.map(|row| {
                phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
                    selection: row.selection,
                    inference_generation: row.inference_generation,
                }
            }));
        }

        let prior_inferred = persisted.as_ref().and_then(|row| {
            row.selection.as_ref().and_then(|selection| {
                (selection.provenance
                    == phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Inferred)
                    .then_some(&selection.pr)
            })
        });
        let inferred = infer_active_pr_selection(
            &associations,
            latest_observed_branch.as_ref(),
            prior_inferred,
        );
        let current_generation = persisted.as_ref().map_or(0, |row| row.inference_generation);
        let next_generation = persisted
            .as_ref()
            .map_or(1, |row| row.inference_generation + 1);
        let now = Utc::now().to_rfc3339();
        let write = sqlx::query(
            "INSERT INTO work_scope_active_pr_selection (
                work_scope_id, repo_owner, repo_name, pr_number, provenance,
                latest_observed_repository_identity, latest_observed_branch_name,
                inference_generation, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'inferred', ?5, ?6, ?7, ?8)
             ON CONFLICT(work_scope_id) DO UPDATE SET
                repo_owner = excluded.repo_owner,
                repo_name = excluded.repo_name,
                pr_number = excluded.pr_number,
                provenance = excluded.provenance,
                latest_observed_repository_identity = excluded.latest_observed_repository_identity,
                latest_observed_branch_name = excluded.latest_observed_branch_name,
                inference_generation = excluded.inference_generation,
                updated_at = excluded.updated_at
             WHERE work_scope_active_pr_selection.inference_generation = ?9",
        )
        .bind(work_scope_id.as_str())
        .bind(inferred.as_ref().map(|pr| pr.repo_owner.as_str()))
        .bind(inferred.as_ref().map(|pr| pr.repo_name.as_str()))
        .bind(inferred.as_ref().map(|pr| pr.pr_number.cast_signed()))
        .bind(
            latest_observed_branch
                .as_ref()
                .map(|b| b.repository_identity.as_str()),
        )
        .bind(
            latest_observed_branch
                .as_ref()
                .map(|b| b.branch_name.as_str()),
        )
        .bind(next_generation.cast_signed())
        .bind(&now)
        .bind(current_generation.cast_signed())
        .execute(&mut *tx)
        .await?;
        if write.rows_affected() == 0 {
            tx.rollback().await?;
            return self
                .active_pr_selection_state_for_scope_id(work_scope_id)
                .await;
        }
        tx.commit().await?;

        Ok(Some(phoenix_core::domain::active_pr_selection::ActivePrSelectionState {
            selection: inferred.map(|pr| phoenix_core::domain::active_pr_selection::ActivePrSelection {
                pr,
                provenance:
                    phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Inferred,
            }),
            inference_generation: next_generation,
        }))
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn work_scope_pr_feedback_baseline(
        &self,
        scope: &phoenix_core::work_scope::WorkScopeId,
        repo_owner: &str,
        repo_name: &str,
        pr_number: u64,
    ) -> DbResult<Option<WorkScopePrFeedbackBaseline>> {
        if !self.work_scope_exists(scope).await? {
            return Ok(None);
        }
        let work_scope_id = scope;
        let row = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, captured_at, github_updated_at, feedback_identities, feedback_fingerprints
             FROM work_scope_pr_feedback_baselines
             WHERE work_scope_id = ?1 AND repo_owner = ?2 AND repo_name = ?3 AND pr_number = ?4",
        )
        .bind(work_scope_id.as_str())
        .bind(repo_owner)
        .bind(repo_name)
        .bind(pr_number.cast_signed())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let raw: String = row.get("feedback_identities");
            let feedback_identities =
                serde_json::from_str(&raw).map_err(|e| DbError::Serialization(e.to_string()))?;
            let raw_fingerprints: String = row.get("feedback_fingerprints");
            let feedback_fingerprints = serde_json::from_str(&raw_fingerprints)
                .map_err(|e| DbError::Serialization(e.to_string()))?;
            Ok(WorkScopePrFeedbackBaseline {
                work_scope_id: row_work_scope_id(&row),
                repo_owner: row.get("repo_owner"),
                repo_name: row.get("repo_name"),
                pr_number: row.get::<i64, _>("pr_number").cast_unsigned(),
                captured_at: row.get("captured_at"),
                github_updated_at: row.get("github_updated_at"),
                feedback_identities,
                feedback_fingerprints,
            })
        })
        .transpose()
    }

    /// Reconcile persisted worktree identity with the current Git administrative marker.
    ///
    /// Inaccessible worktrees remain unresolved instead of receiving fabricated continuity.
    /// # Errors
    /// Returns a database error when an observed identity cannot be persisted.
    #[allow(clippy::too_many_lines)]
    pub async fn reconcile_worktree_identities(&self) -> DbResult<()> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        let scopes = sqlx::query_as::<_, (String, String)>(
            "SELECT id, worktree_path
             FROM work_scopes
             WHERE lifecycle = 'active'
               AND environment_kind = 'allocated_worktree'
               AND worktree_path IS NOT NULL
             ORDER BY id",
        )
        .fetch_all(&mut *tx)
        .await?;
        let observations = scopes
            .into_iter()
            .map(|(scope_id, path)| {
                let observed_fingerprint = Self::observe_worktree_fingerprint(&path);
                (scope_id, path, observed_fingerprint)
            })
            .collect::<Vec<_>>();

        for (scope_id, path, observed_fingerprint) in &observations {
            let revalidated = Self::observe_worktree_fingerprint(path);
            let stable_fingerprint = match (observed_fingerprint, revalidated) {
                (Some(observed), Some(revalidated)) if observed == &revalidated => Some(observed),
                _ => None,
            };
            sqlx::query(
                "UPDATE work_scopes
                 SET worktree_id = NULL, worktree_fingerprint = NULL
                 WHERE id = ?1
                   AND lifecycle = 'active'
                   AND environment_kind = 'allocated_worktree'
                   AND worktree_path = ?2
                   AND (?3 IS NULL OR NOT (worktree_id IS NOT NULL AND worktree_fingerprint = ?3))
                   AND NOT EXISTS (
                       SELECT 1
                       FROM close_attempt_scopes captured
                       JOIN close_obligations obligation
                         ON obligation.attempt_id = captured.attempt_id
                       WHERE captured.scope = work_scopes.id
                         AND obligation.phase <> 'completed'
                         AND obligation.topology_sealed = 1
                   )",
            )
            .bind(scope_id)
            .bind(path)
            .bind(stable_fingerprint)
            .execute(&mut *tx)
            .await?;
        }

        for (scope_id, path, observed_fingerprint) in &observations {
            let Some(observed_fingerprint) = observed_fingerprint else {
                continue;
            };
            let Some(revalidated_fingerprint) = Self::observe_worktree_fingerprint(path) else {
                continue;
            };
            if &revalidated_fingerprint != observed_fingerprint {
                continue;
            }
            let result = sqlx::query(
                "UPDATE work_scopes
                 SET worktree_id = ?1, worktree_fingerprint = ?2
                 WHERE id = ?3
                   AND lifecycle = 'active'
                   AND environment_kind = 'allocated_worktree'
                   AND worktree_path = ?4
                   AND worktree_id IS NULL
                   AND worktree_fingerprint IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM work_scopes owner
                       WHERE owner.worktree_fingerprint = ?2 AND owner.id <> ?3
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM close_attempt_scopes captured
                       JOIN close_obligations obligation
                         ON obligation.attempt_id = captured.attempt_id
                       WHERE captured.scope = work_scopes.id
                         AND obligation.phase <> 'completed'
                         AND obligation.topology_sealed = 1
                   )",
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(observed_fingerprint)
            .bind(scope_id)
            .bind(path)
            .execute(&mut *tx)
            .await?;
            if result.rows_affected() == 0 {
                tracing::debug!(
                    work_scope_id = %scope_id,
                    "worktree identity reconciliation deferred or unresolved"
                );
            }
        }

        for (scope_id, path, observed_fingerprint) in &observations {
            if Self::observe_worktree_fingerprint(path).as_ref() == observed_fingerprint.as_ref() {
                continue;
            }
            sqlx::query(
                "UPDATE work_scopes
                 SET worktree_id = NULL, worktree_fingerprint = NULL
                 WHERE id = ?1
                   AND lifecycle = 'active'
                   AND environment_kind = 'allocated_worktree'
                   AND worktree_path = ?2
                   AND NOT EXISTS (
                       SELECT 1
                       FROM close_attempt_scopes captured
                       JOIN close_obligations obligation
                         ON obligation.attempt_id = captured.attempt_id
                       WHERE captured.scope = work_scopes.id
                         AND obligation.phase <> 'completed'
                         AND obligation.topology_sealed = 1
                   )",
            )
            .bind(scope_id)
            .bind(path)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Open or create database at the given path
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn open(path: &str) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))?
            .journal_mode(SqliteJournalMode::Wal)
            // synchronous=NORMAL is safe under WAL: commits are durable across
            // process crashes (the WAL append is what makes them durable),
            // only a power-failure between WAL append and the next checkpoint
            // fsync can lose the last commit. Default FULL fsyncs every
            // commit, which under concurrent I/O load (e.g. ./dev.py check)
            // can stretch single-row INSERTs to 1+s. See task 13042.
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(5))
            .foreign_keys(true);
        let sqlite_workload_collector = SqliteWorkloadCollector::new();
        let pool = SqlitePoolOptions::new()
            .after_connect({
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                move |conn, _meta| {
                    let sqlite_workload_collector = sqlite_workload_collector.clone();
                    Box::pin(async move {
                        install_native_statement_baseline(conn, sqlite_workload_collector).await
                    })
                }
            })
            .connect_with(opts)
            .await?;
        // The DB (and its WAL sidecars) holds conversation history — command
        // output, secrets the agent saw. On a multi-user host the default umask
        // can leave it world-readable, so tighten to owner-only. Best-effort:
        // a chmod failure is logged, never fatal to startup.
        restrict_db_permissions(path);
        let db = Self::new_with_generated_target_binding(
            pool,
            path.to_string(),
            sqlite_workload_collector,
        );
        db.run_migrations().await?;
        // `run_migrations` may have created the `-wal`/`-shm` sidecars that the
        // early chmod above could not see. Re-tighten now they exist. The prod
        // path runs numbered migrations after `open` returns, so it must call
        // `restrict_file_permissions` again afterward.
        db.restrict_file_permissions();
        Ok(db)
    }

    /// Execute one bounded Coordinator read query against a separate read-only connection.
    ///
    /// # Errors
    ///
    /// Returns a policy, budget, or `SQLite` error without exposing the database path.
    pub async fn coordinator_query(
        &self,
        sql: &str,
    ) -> Result<CoordinatorQueryResult, CoordinatorQueryError> {
        let path = self.path.clone();
        let sql = sql.to_string();
        tokio::task::spawn_blocking(move || execute_coordinator_query(&path, &sql))
            .await
            .map_err(|_| CoordinatorQueryError::WorkerFailed)?
    }

    /// Open an in-memory database (for testing).
    ///
    /// Runs both the legacy idempotent ALTER TABLEs (`run_migrations`) and the
    /// numbered migrations (`run_pending_migrations`), mirroring the production
    /// startup sequence in `main.rs`. Without this, tests that exercise columns
    /// added by numbered migrations would fail against a half-initialized DB.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Used in tests
    pub async fn open_in_memory() -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(5))
            .foreign_keys(true);
        // In-memory SQLite DBs are per-connection, so limit to 1 connection
        let sqlite_workload_collector = SqliteWorkloadCollector::new();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect({
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                move |conn, _meta| {
                    let sqlite_workload_collector = sqlite_workload_collector.clone();
                    Box::pin(async move {
                        install_native_statement_baseline(conn, sqlite_workload_collector).await
                    })
                }
            })
            .connect_with(opts)
            .await?;
        let db =
            Self::new_with_generated_target_binding(pool, String::new(), sqlite_workload_collector);
        db.run_migrations().await?;
        migrations::run_pending_migrations(&db.pool).await?;
        Ok(db)
    }

    async fn run_migrations(&self) -> DbResult<()> {
        sqlx::raw_sql(ddl::SCHEMA).execute(&self.pool).await?;
        sqlx::raw_sql(ddl::MIGRATION_TYPED_STATE)
            .execute(&self.pool)
            .await?;

        // Drop the dead state_data column (task 02667). Ignored on fresh DBs
        // where SCHEMA no longer creates it; drops it on upgraded DBs that
        // still carry it from the pre-typed-state schema. Never read or
        // written by any query.
        // DROP COLUMN needs SQLite >= 3.35. SQLite is bundled via sqlx's
        // `sqlite` feature (libsqlite3-sys, build-controlled and modern), so
        // the host SQLite version is not a factor here. The benign case is a
        // fresh DB where the column never existed ("no such column"); a real
        // failure on an upgraded DB leaves the dead column in place (harmless
        // — never read/written — but worth a warn so it's not invisible).
        if let Err(e) = sqlx::raw_sql("ALTER TABLE conversations DROP COLUMN state_data")
            .execute(&self.pool)
            .await
        {
            if e.to_string().contains("no such column") {
                tracing::debug!("state_data column already absent; nothing to drop");
            } else {
                tracing::warn!(
                    error = %e,
                    "Failed to drop dead state_data column on an upgraded DB; it will remain (unused)"
                );
            }
        }

        // Try to add model column - ignore error if it already exists
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN model TEXT")
            .execute(&self.pool)
            .await;

        // Rename id -> message_id for searchability (ignore error if already done)
        let _ = sqlx::raw_sql(ddl::MIGRATION_RENAME_MESSAGE_ID)
            .execute(&self.pool)
            .await;

        // Replace "unknown" error_kind with "server_error" in stored conversation state
        let _ = sqlx::raw_sql(ddl::MIGRATION_REMOVE_UNKNOWN_ERROR_KIND)
            .execute(&self.pool)
            .await;

        // Create projects table (idempotent via IF NOT EXISTS)
        let _ = sqlx::raw_sql(ddl::MIGRATION_CREATE_PROJECTS)
            .execute(&self.pool)
            .await;

        // Add project_id column to conversations
        // Each ALTER TABLE is independent; ignore errors if columns already exist
        let _ = sqlx::raw_sql(
            "ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id)",
        )
        .execute(&self.pool)
        .await;
        // NOTE: conv_mode (the legacy ConvMode JSON blob) is intentionally not
        // ADDed here. It lives in the base schema and is DROPped by migration
        // 029 after being normalized into the cm_* columns; re-adding it via
        // this idempotent bootstrap would resurrect the dropped column.

        // Add title column for human-readable conversation names
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN title TEXT")
            .execute(&self.pool)
            .await;

        // Add desired_base_branch for Managed mode branch selection
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN desired_base_branch TEXT")
            .execute(&self.pool)
            .await;

        // Create mcp_disabled_servers table (idempotent via IF NOT EXISTS)
        let _ = sqlx::raw_sql(ddl::MIGRATION_CREATE_MCP_DISABLED_SERVERS)
            .execute(&self.pool)
            .await;

        // Create share_tokens table (REQ-AUTH-008, idempotent via IF NOT EXISTS)
        let _ = sqlx::raw_sql(ddl::MIGRATION_CREATE_SHARE_TOKENS)
            .execute(&self.pool)
            .await;

        // Seeded conversations: decorative parent link and label
        // (REQ-SEED-003, REQ-SEED-004). Nullable, no foreign key — the link
        // is advisory-only and if the parent is deleted the UI handles it.
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN seed_parent_id TEXT")
            .execute(&self.pool)
            .await;
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN seed_label TEXT")
            .execute(&self.pool)
            .await;

        // DEPRECATED: the steering queue is normalized into the
        // `steering_messages` (+ attachment) tables; this column is no longer
        // read or written (it defaults to '[]'). It is retained because the
        // idempotent legacy-ALTER bootstrap would resurrect a dropped column on
        // the next boot, and every historical migration replays against it. A
        // physical column drop is a separate, careful follow-up.
        let _ = sqlx::raw_sql(
            "ALTER TABLE conversations ADD COLUMN steering_queue TEXT NOT NULL DEFAULT '[]'",
        )
        .execute(&self.pool)
        .await;

        Ok(())
    }

    // ==================== Notification Settings ====================

    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_notification_settings(&self) -> DbResult<NotificationSettings> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM notification_settings")
                .fetch_all(&self.pool)
                .await?;
        let mut settings = NotificationSettings::default();
        for (key, value) in rows {
            let parsed = value == "true";
            match key.as_str() {
                "notifications_enabled" => settings.enabled = parsed.into(),
                "notify_task_approval" => settings.events.task_approval = parsed.into(),
                "notify_question" => settings.events.question = parsed.into(),
                "notify_error" => settings.events.error = parsed.into(),
                "notify_idle" => settings.events.idle = parsed.into(),
                _ => {}
            }
        }
        Ok(settings)
    }

    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn set_notification_settings(&self, settings: &NotificationSettings) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        let pairs = [
            ("notifications_enabled", settings.enabled.as_bool()),
            (
                "notify_task_approval",
                settings.events.task_approval.as_bool(),
            ),
            ("notify_question", settings.events.question.as_bool()),
            ("notify_error", settings.events.error.as_bool()),
            ("notify_idle", settings.events.idle.as_bool()),
        ];
        for (key, enabled) in pairs {
            sqlx::query(
                "INSERT INTO notification_settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(key)
            .bind(if enabled { "true" } else { "false" })
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ==================== App Settings (generic key/value) ====================

    /// Read a single global app setting by key, returning `None` if unset.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_app_setting(&self, key: &str) -> DbResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM app_settings WHERE key = ?1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(v,)| v))
    }

    /// Write a single global app setting (upsert).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn set_app_setting(&self, key: &str, value: &str) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return the global default `LlmLanguage` for new conversations.
    /// Falls back to `LlmLanguage::default()` when the row is missing or
    /// holds a value this build doesn't recognize.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_default_llm_language(
        &self,
    ) -> DbResult<phoenix_core::llm_language::LlmLanguage> {
        Ok(self
            .get_app_setting("default_llm_language")
            .await?
            .as_deref()
            .map(phoenix_core::llm_language::LlmLanguage::parse_or_default)
            .unwrap_or_default())
    }

    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn set_default_llm_language(
        &self,
        lang: phoenix_core::llm_language::LlmLanguage,
    ) -> DbResult<()> {
        self.set_app_setting("default_llm_language", lang.as_str())
            .await
    }

    // ==================== Sub-Agent Personas ====================

    /// Persist a named-agent persona for a sub-agent conversation so it
    /// survives runtime recreation (REQ-AG-006). Upserts.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn set_sub_agent_persona(
        &self,
        conversation_id: &str,
        persona: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO sub_agent_personas (conversation_id, persona) VALUES (?1, ?2) \
             ON CONFLICT(conversation_id) DO UPDATE SET persona = excluded.persona",
        )
        .bind(conversation_id)
        .bind(persona)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Read a sub-agent conversation's persisted persona, if any.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_sub_agent_persona(&self, conversation_id: &str) -> DbResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT persona FROM sub_agent_personas WHERE conversation_id = ?1")
                .bind(conversation_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(p,)| p))
    }

    // ==================== MCP Disabled Servers ====================

    /// Return the set of MCP server names that have been disabled.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_disabled_mcp_servers(&self) -> DbResult<std::collections::HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT server_name FROM mcp_disabled_servers")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }

    /// Mark an MCP server as disabled (idempotent).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn disable_mcp_server(&self, name: &str) -> DbResult<()> {
        sqlx::query("INSERT OR IGNORE INTO mcp_disabled_servers (server_name) VALUES (?1)")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Re-enable an MCP server by removing it from the disabled set.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn enable_mcp_server(&self, name: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM mcp_disabled_servers WHERE server_name = ?1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== MCP OAuth Store (REQ-MCP-010, REQ-MCP-012) ====================

    /// Fetch the persisted OAuth client registration for an authorization
    /// server, if one exists.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_mcp_oauth_registration(
        &self,
        auth_server: &str,
    ) -> DbResult<Option<McpOAuthRegistrationRow>> {
        let row: Option<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
            "SELECT client_id, client_secret, token_endpoint_auth_method, redirect_uri \
             FROM mcp_oauth_registrations WHERE auth_server = ?1",
        )
        .bind(auth_server)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(client_id, client_secret, token_endpoint_auth_method, redirect_uri)| {
                McpOAuthRegistrationRow {
                    auth_server: auth_server.to_string(),
                    client_id,
                    client_secret,
                    token_endpoint_auth_method,
                    redirect_uri,
                }
            },
        ))
    }

    /// Insert or replace the OAuth client registration for an authorization
    /// server.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn upsert_mcp_oauth_registration(
        &self,
        registration: &McpOAuthRegistrationRow,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO mcp_oauth_registrations \
             (auth_server, client_id, client_secret, token_endpoint_auth_method, redirect_uri) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&registration.auth_server)
        .bind(&registration.client_id)
        .bind(&registration.client_secret)
        .bind(&registration.token_endpoint_auth_method)
        .bind(&registration.redirect_uri)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetch the persisted OAuth token for an MCP server, if one exists.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_mcp_oauth_token(
        &self,
        server_name: &str,
    ) -> DbResult<Option<McpOAuthTokenRow>> {
        let row: Option<(String, String, String, Option<String>, i64)> = sqlx::query_as(
            "SELECT resource_uri, scopes, access_token, refresh_token, expires_at \
             FROM mcp_oauth_tokens WHERE server_name = ?1",
        )
        .bind(server_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(resource_uri, scopes, access_token, refresh_token, expires_at)| McpOAuthTokenRow {
                server_name: server_name.to_string(),
                resource_uri,
                scopes,
                access_token,
                refresh_token,
                expires_at,
            },
        ))
    }

    /// Insert or replace the OAuth token for an MCP server. The primary key on
    /// `server_name` keeps at most one token per server.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn upsert_mcp_oauth_token(&self, token: &McpOAuthTokenRow) -> DbResult<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO mcp_oauth_tokens \
             (server_name, resource_uri, scopes, access_token, refresh_token, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&token.server_name)
        .bind(&token.resource_uri)
        .bind(&token.scopes)
        .bind(&token.access_token)
        .bind(&token.refresh_token)
        .bind(token.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete the OAuth token for an MCP server (idempotent).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn delete_mcp_oauth_token(&self, server_name: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM mcp_oauth_tokens WHERE server_name = ?1")
            .bind(server_name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== Share Token Operations (REQ-AUTH-008) ====================

    /// Create a share token for a conversation, or return existing one.
    ///
    /// Returns the token string. If a token already exists for this conversation,
    /// returns it instead of creating a duplicate.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn create_share_token(&self, conversation_id: &str) -> DbResult<String> {
        // Check for existing token first
        if let Some(existing) = self
            .get_share_token_by_conversation(conversation_id)
            .await?
        {
            return Ok(existing);
        }

        let id = uuid::Uuid::new_v4().to_string();
        let token = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO share_tokens (id, conversation_id, token, created_at) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&id)
        .bind(conversation_id)
        .bind(&token)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(token)
    }

    /// Look up a share token record by its token string.
    ///
    /// Returns `(conversation_id, token)` if found, `None` otherwise.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_share_token_by_token(
        &self,
        token: &str,
    ) -> DbResult<Option<(String, String)>> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT conversation_id, token FROM share_tokens WHERE token = ?1")
                .bind(token)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    /// Get the share token for a conversation, if one exists.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_share_token_by_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT token FROM share_tokens WHERE conversation_id = ?1")
                .bind(conversation_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(t,)| t))
    }

    /// Delete the share token for a conversation (revoke sharing).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Will be used by future revoke-share endpoint
    pub async fn delete_share_token(&self, conversation_id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM share_tokens WHERE conversation_id = ?1")
            .bind(conversation_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== Auth Session Operations ====================

    /// Persist a freshly minted browser session token with its lifetime.
    ///
    /// The token is generated by the caller (opaque random bytes); this only
    /// records it so it survives a restart. `ttl` sets `expires_at`, which
    /// [`Self::is_auth_session_valid`] enforces. `password_fingerprint` binds the
    /// token to the password it was minted under so a password rotation can
    /// invalidate it.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn insert_auth_session(
        &self,
        token: &str,
        password_fingerprint: &str,
        ttl: chrono::Duration,
    ) -> DbResult<()> {
        let now = Utc::now();
        let expires_at = now + ttl;
        sqlx::query(
            "INSERT INTO auth_sessions (token, password_fingerprint, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(token)
        .bind(password_fingerprint)
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether `token` names a session that exists, has not expired, and was
    /// minted under the currently-configured password (`password_fingerprint`).
    ///
    /// `expires_at` is stored as RFC 3339 UTC, so the lexicographic string
    /// comparison below orders identically to chronological order.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn is_auth_session_valid(
        &self,
        token: &str,
        password_fingerprint: &str,
    ) -> DbResult<bool> {
        let now = Utc::now().to_rfc3339();
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM auth_sessions \
             WHERE token = ?1 AND password_fingerprint = ?2 AND expires_at > ?3",
        )
        .bind(token)
        .bind(password_fingerprint)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Delete expired session rows. Called opportunistically on login so the
    /// table cannot grow without bound; lookups already ignore expired rows.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn delete_expired_auth_sessions(&self) -> DbResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("DELETE FROM auth_sessions WHERE expires_at <= ?1")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ==================== Project Operations ====================

    /// Find or create a project by its canonical git repo root path.
    ///
    /// REQ-PROJ-001: Projects are keyed by resolved repo root.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn find_or_create_project(&self, canonical_path: &str) -> DbResult<Project> {
        // Try to find existing project
        let existing = sqlx::query(
            "SELECT id, canonical_path, main_ref, created_at,
                    (SELECT COUNT(*) FROM conversations c WHERE c.project_id = p.id AND c.archived = 0) as conversation_count
             FROM projects p WHERE canonical_path = ?1",
        )
        .bind(canonical_path)
        .try_map(parse_project_row)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(project) = existing {
            return Ok(project);
        }

        // Create new project. main_ref is the resolved default branch
        // (REQ-PROJ-034a / Allium GitDirectoryDetected: the remote default when
        // detectable, else the checked-out branch), not a hardcoded literal — a
        // repo whose default is `master`/`develop` must not be sent to `main`.
        // `main` is the fallback only when neither can be resolved (e.g. a
        // detached HEAD with no remote), and startup reconciliation corrects it
        // later if the repo becomes resolvable.
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let main_ref =
            phoenix_core::git::resolve_default_branch(std::path::Path::new(canonical_path))
                .unwrap_or_else(|| "main".to_string());

        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&id)
        .bind(canonical_path)
        .bind(&main_ref)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(Project {
            id,
            canonical_path: canonical_path.to_string(),
            main_ref,
            created_at: now,
            conversation_count: 0,
        })
    }

    /// Get a project by ID.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_project(&self, id: &str) -> DbResult<Project> {
        let project = sqlx::query(
            "SELECT id, canonical_path, main_ref, created_at,
                    (SELECT COUNT(*) FROM conversations c WHERE c.project_id = p.id AND c.archived = 0) as conversation_count
             FROM projects p WHERE id = ?1",
        )
        .bind(id)
        .try_map(parse_project_row)
        .fetch_optional(&self.pool)
        .await?;

        project.ok_or_else(|| DbError::ConversationNotFound(format!("project {id}")))
    }

    /// List all projects with conversation counts
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_projects(&self) -> DbResult<Vec<Project>> {
        let rows = sqlx::query(
            "SELECT p.id, p.canonical_path, p.main_ref, p.created_at,
                    (SELECT COUNT(*) FROM conversations c WHERE c.project_id = p.id AND c.archived = 0) as conversation_count
             FROM projects p
             ORDER BY p.created_at DESC",
        )
        .try_map(parse_project_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Update a project's `main_ref` to the resolved default branch
    /// (REQ-PROJ-034a). Used by startup reconciliation to backfill rows whose
    /// `main_ref` was defaulted to a literal.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_project_main_ref(&self, project_id: &str, main_ref: &str) -> DbResult<()> {
        let result = sqlx::query("UPDATE projects SET main_ref = ?1 WHERE id = ?2")
            .bind(main_ref)
            .bind(project_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(format!(
                "project {project_id}"
            )));
        }
        Ok(())
    }

    // ==================== Conversation Operations ====================

    /// Create a conversation with explore-mode defaults — a test convenience
    /// wrapper around [`Database::create_conversation_with_project`].
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn create_conversation(
        &self,
        id: &str,
        slug: &str,
        cwd: &str,
        user_initiated: bool,
        parent_id: Option<&str>,
        model: Option<&str>,
    ) -> DbResult<Conversation> {
        self.create_conversation_with_project(
            id,
            slug,
            cwd,
            user_initiated,
            parent_id,
            model,
            None,
            &ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            },
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
    }

    /// Create a new conversation, optionally associated with a project.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_conversation_with_project(
        &self,
        id: &str,
        slug: &str,
        cwd: &str,
        user_initiated: bool,
        parent_id: Option<&str>,
        model: Option<&str>,
        project_id: Option<&str>,
        conv_mode: &ConvMode,
        desired_base_branch: Option<&str>,
        seed_parent_id: Option<&str>,
        seed_label: Option<&str>,
        llm_language: phoenix_core::llm_language::LlmLanguage,
    ) -> DbResult<Conversation> {
        self.create_conversation_with_project_inner(
            id,
            slug,
            cwd,
            user_initiated,
            parent_id,
            model,
            project_id,
            conv_mode,
            desired_base_branch,
            seed_parent_id,
            seed_label,
            llm_language,
            ExpectedParentScope::NotChecked,
        )
        .await
    }

    /// Creates a sub-agent conversation attached to the exact parent scope captured at spawn.
    ///
    /// # Errors
    /// Returns [`DbError`] when the parent no longer owns the captured scope or persistence fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_subagent_conversation(
        &self,
        id: &str,
        slug: &str,
        cwd: &str,
        parent_id: &str,
        model: &str,
        conv_mode: &ConvMode,
        llm_language: phoenix_core::llm_language::LlmLanguage,
        parent_scope: Option<&WorkScopeId>,
    ) -> DbResult<Conversation> {
        self.create_conversation_with_project_inner(
            id,
            slug,
            cwd,
            false,
            Some(parent_id),
            Some(model),
            None,
            conv_mode,
            None,
            None,
            None,
            llm_language,
            ExpectedParentScope::Snapshot(parent_scope),
        )
        .await
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn create_conversation_with_project_inner(
        &self,
        id: &str,
        slug: &str,
        cwd: &str,
        user_initiated: bool,
        parent_id: Option<&str>,
        model: Option<&str>,
        project_id: Option<&str>,
        conv_mode: &ConvMode,
        desired_base_branch: Option<&str>,
        seed_parent_id: Option<&str>,
        seed_label: Option<&str>,
        llm_language: phoenix_core::llm_language::LlmLanguage,
        expected_parent_scope: ExpectedParentScope<'_>,
    ) -> DbResult<Conversation> {
        let now = Utc::now();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();
        let cm = conv_mode_columns(conv_mode);
        let now_str = now.to_rfc3339();
        let unchecked_parent_values = if matches!(
            expected_parent_scope,
            ExpectedParentScope::NotChecked
        ) {
            if let Some(parent_id) = parent_id {
                let row = sqlx::query(
                        "SELECT work_scope_id, effort, product_conversation_id FROM conversations WHERE id = ?1",
                    )
                    .bind(parent_id)
                    .fetch_optional(&self.pool)
                    .await?;
                Some(parent_creation_values(row)?)
            } else {
                Some((None, None, None))
            }
        } else {
            None
        };
        let runtime_role = if parent_id.is_some() {
            RuntimeRole::SubAgent
        } else {
            RuntimeRole::User
        };

        // Retry with a random suffix on slug collision (UNIQUE constraint).
        let mut actual_slug = slug.to_string();
        let mut attempts = 0u8;
        let preserve_unattached_parent =
            matches!(expected_parent_scope, ExpectedParentScope::Snapshot(None));
        let (created_work_scope_id, inherited_effort, product_conversation_id) = loop {
            let title_str = schema::title_from_slug(&actual_slug);
            let mut tx = match &expected_parent_scope {
                ExpectedParentScope::NotChecked => self.pool.begin().await?,
                ExpectedParentScope::Snapshot(_) => self.pool.begin_with("BEGIN IMMEDIATE").await?,
            };
            let (inherited_scope, inherited_effort, inherited_product_conversation_id) =
                match &expected_parent_scope {
                    ExpectedParentScope::NotChecked => unchecked_parent_values
                        .clone()
                        .expect("unchecked parent values are loaded before the transaction"),
                    ExpectedParentScope::Snapshot(expected_scope) => {
                        let parent_id = parent_id.expect("scope snapshots require a parent");
                        let row = sqlx::query(
                        "SELECT work_scope_id, effort, product_conversation_id FROM conversations WHERE id = ?1",
                    )
                    .bind(parent_id)
                    .fetch_optional(&mut *tx)
                    .await?;
                        let Some(row) = row else {
                            tx.rollback().await?;
                            return Err(DbError::CloseFoundationConflict(format!(
                                "parent conversation {parent_id} no longer exists"
                            )));
                        };
                        let values = parent_creation_values(Some(row))?;
                        #[cfg(test)]
                        if let Some(latch) = &self.sub_agent_creation_test_latch {
                            latch.parent_read.notify_one();
                            latch.competing_write_observed.notified().await;
                        }
                        if values.0.as_ref() != *expected_scope {
                            tx.rollback().await?;
                            return Err(DbError::CloseFoundationConflict(format!(
                            "parent conversation {parent_id} no longer owns captured WorkScope {expected_scope:?}"
                        )));
                        }
                        values
                    }
                };
            let product_conversation_id =
                if let Some(product_conversation_id) = inherited_product_conversation_id {
                    product_conversation_id
                } else {
                    let product_conversation_id =
                        phoenix_core::domain::product_conversation::ProductConversationId::new();
                    sqlx::query(
                        "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
                     VALUES (?1, 'ordinary', 'open')",
                    )
                    .bind(product_conversation_id.as_str())
                    .execute(&mut *tx)
                    .await?;
                    product_conversation_id
                };
            let generated_scope =
                (inherited_scope.is_none() && !preserve_unattached_parent).then(|| {
                    let (scope_id, authority_kind, environment) =
                        Self::new_scope_for_conversation(cwd, &cm);
                    (scope_id, authority_kind, environment)
                });
            let work_scope_id = inherited_scope.clone().or_else(|| {
                generated_scope
                    .as_ref()
                    .map(|(scope_id, _, _)| scope_id.clone())
            });
            if let Some((scope_id, authority_kind, environment)) = generated_scope {
                Self::insert_work_scope_tx(
                    &mut tx,
                    &scope_id,
                    authority_kind,
                    environment,
                    &now_str,
                )
                .await?;
            }
            let result = sqlx::query(
                "INSERT INTO conversations (id, product_conversation_id, slug, title, parent_conversation_id, user_initiated, state, state_kind, state_updated_at, created_at, updated_at, archived, transcript_generation, model, effort, project_id, desired_base_branch, seed_parent_id, seed_label, llm_language, cm_kind, cm_task_id, cm_task_title, cm_next_taskmd_id_hint, runtime_role, work_scope_id, sub_agent_cwd_override, service_tier)
                 VALUES (?1, ?24, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8, 0, 1, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            )
            .bind(id)
            .bind(&actual_slug)
            .bind(&title_str)
            .bind(parent_id)
            .bind(user_initiated)
            .bind(&idle_state)
            .bind(conv_state_kind(&ConvState::Idle))
            .bind(&now_str)
            .bind(model)
            .bind(inherited_effort.map(ModelEffort::as_wire_name))
            .bind(project_id)
            .bind(desired_base_branch)
            .bind(seed_parent_id)
            .bind(seed_label)
            .bind(llm_language.as_str())
            .bind(cm.kind)
            .bind(cm.task_id)
            .bind(cm.task_title)
            .bind(cm.next_taskmd_id_hint)
            .bind(runtime_role.as_str())
            .bind(work_scope_id.as_ref().map(WorkScopeId::as_str))
            .bind(parent_id.map(|_| cwd))
            .bind(ServiceTier::Standard.as_wire_name())
            .bind(product_conversation_id.as_str())
            .execute(&mut *tx)
            .await;

            match result {
                Ok(_) => {
                    tx.commit().await?;
                    break (work_scope_id, inherited_effort, product_conversation_id);
                }
                Err(sqlx::Error::Database(ref e))
                    if (is_sqlite_unique_constraint(e.as_ref())
                        || is_sqlite_primary_key_constraint(e.as_ref()))
                        && e.message().contains("conversations.id") =>
                {
                    tx.rollback().await?;
                    return Err(DbError::ConversationAlreadyExists(id.to_string()));
                }
                Err(sqlx::Error::Database(ref e)) if is_sqlite_unique_constraint(e.as_ref()) => {
                    tx.rollback().await?;
                    attempts += 1;
                    if attempts >= 10 {
                        // Last resort: full UUID fragment (UUIDs are ASCII, first 8 bytes always valid)
                        let uuid_str = uuid::Uuid::new_v4().to_string();
                        actual_slug = format!("{slug}-{}", uuid_str.get(..8).unwrap_or(&uuid_str));
                    } else {
                        actual_slug = format!("{slug}-{:04x}", rand::random::<u16>());
                    }
                }
                Err(e) => {
                    tx.rollback().await?;
                    return Err(DbError::Sqlx(e));
                }
            }
        };

        let title = schema::title_from_slug(&actual_slug);
        Ok(Conversation {
            id: id.to_string(),
            product_conversation_id,
            slug: Some(actual_slug),
            title: Some(title),
            cwd: cwd.to_string(),
            parent_conversation_id: parent_id.map(String::from),
            user_initiated,
            state: ConvState::Idle,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: model.map(String::from),
            project_id: project_id.map(String::from),
            effort: inherited_effort,
            service_tier: ServiceTier::Standard,
            conv_mode: conv_mode.clone(),
            runtime_role,
            attached_work_scope_id: created_work_scope_id,
            desired_base_branch: desired_base_branch.map(String::from),
            message_count: 0,
            transcript_generation: 1,
            seed_parent_id: seed_parent_id.map(String::from),
            seed_label: seed_label.map(String::from),
            // REQ-BED-030: fresh conversations have not been continued.
            continued_in_conv_id: None,
            // REQ-CHN-007: fresh conversations have no user-set chain name.
            chain_name: None,
            llm_language,
            spawned_from_conversation_id: None,
        })
    }

    /// Resolve the singleton Coordinator, creating its standard-runtime row atomically.
    ///
    /// # Errors
    /// Returns an error when the transaction, insert, or conversation reload fails.
    pub async fn get_or_create_coordinator(
        &self,
        model: Option<&str>,
        llm_language: phoenix_core::llm_language::LlmLanguage,
    ) -> DbResult<Conversation> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let result: DbResult<String> = async {
            if let Some(id) = sqlx::query_scalar(
                "SELECT id FROM conversations WHERE coordinator_head = 1",
            )
            .fetch_optional(&mut *conn)
            .await?
            {
                return Ok(id);
            }

            let id = uuid::Uuid::new_v4().to_string();
            let slug = format!("coordinator-{}", id.get(..8).unwrap_or(&id));
            let product_conversation_id =
                phoenix_core::domain::product_conversation::ProductConversationId::new();
            sqlx::query(
                "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
                 VALUES (?1, 'coordinator', NULL)",
            )
            .bind(product_conversation_id.as_str())
            .execute(&mut *conn)
            .await?;
            let now = Utc::now().to_rfc3339();
            let idle = serde_json::to_string(&ConvState::Idle)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            sqlx::query(
                "INSERT INTO conversations (id, product_conversation_id, slug, title, coordinator_head, user_initiated, state, state_kind, state_updated_at, created_at, updated_at, archived, transcript_generation, model, llm_language, cm_kind, runtime_role, work_scope_id)
                 VALUES (?1, ?8, ?2, 'Coordinator', 1, 0, ?3, ?4, ?5, ?5, ?5, 0, 1, ?6, ?7, 'explore', 'coordinator', NULL)",
            )
            .bind(&id)
            .bind(slug)
            .bind(idle)
            .bind(conv_state_kind(&ConvState::Idle))
            .bind(now)
            .bind(model)
            .bind(llm_language.as_str())
            .bind(product_conversation_id.as_str())
            .execute(&mut *conn)
            .await?;
            Ok(id)
        }
        .await;

        match result {
            Ok(conversation_id) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                drop(conn);
                self.get_conversation(&conversation_id).await
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(error)
            }
        }
    }

    /// Return the Coordinator conversation id when the singleton has been created.
    ///
    /// # Errors
    /// Returns an error when the singleton relation cannot be queried.
    pub async fn coordinator_conversation_id(&self) -> DbResult<Option<String>> {
        sqlx::query_scalar("SELECT id FROM conversations WHERE coordinator_head = 1")
            .fetch_optional(&self.pool)
            .await
            .map_err(DbError::Sqlx)
    }

    /// Whether this conversation belongs to the durable Coordinator chain.
    ///
    /// # Errors
    /// Returns an error when the singleton relation cannot be queried.
    pub async fn is_coordinator_conversation(&self, conversation_id: &str) -> DbResult<bool> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM conversations WHERE runtime_role = 'coordinator' AND id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(found.is_some())
    }

    /// Returns the durable opening-handoff intent for a continuation parent.
    ///
    /// # Errors
    /// Returns a database error when the query fails.
    pub async fn continuation_dispatch_intent(
        &self,
        parent_id: &str,
    ) -> DbResult<Option<ContinuationDispatchIntent>> {
        let row = sqlx::query(
            "SELECT parent_conversation_id, successor_conversation_id, message_id, handoff, user_agent FROM continuation_dispatch_intents WHERE parent_conversation_id = ?1",
        )
        .bind(parent_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| ContinuationDispatchIntent {
            parent_conversation_id: row.get("parent_conversation_id"),
            successor_conversation_id: row.get("successor_conversation_id"),
            message_id: row.get("message_id"),
            handoff: row.get("handoff"),
            user_agent: row.get("user_agent"),
        }))
    }

    /// Deletes a continuation intent after its message is durably represented elsewhere.
    ///
    /// # Errors
    /// Returns a database error when the delete fails.
    pub async fn delete_continuation_dispatch_intent(&self, parent_id: &str) -> DbResult<()> {
        sqlx::query("DELETE FROM continuation_dispatch_intents WHERE parent_conversation_id = ?1")
            .bind(parent_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get conversation by ID
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_conversation(&self, id: &str) -> DbResult<Conversation> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id WHERE c.id = ?1",
        )
        .bind(id)
        .try_map(parse_conversation_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if matches!(e, sqlx::Error::RowNotFound) {
                DbError::ConversationNotFound(id.to_string())
            } else {
                DbError::Sqlx(e)
            }
        })
    }

    /// Get conversation by slug
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_conversation_by_slug(&self, slug: &str) -> DbResult<Conversation> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id WHERE c.slug = ?1",
        )
        .bind(slug)
        .try_map(parse_conversation_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if matches!(e, sqlx::Error::RowNotFound) {
                DbError::ConversationNotFound(slug.to_string())
            } else {
                DbError::Sqlx(e)
            }
        })
    }

    /// Load the navigation metadata for a bounded set of conversation search hits.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_conversation_search_metadata(
        &self,
        conversation_ids: &[String],
    ) -> DbResult<std::collections::HashMap<String, ConversationSearchMetadata>> {
        if conversation_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let mut query = sqlx::QueryBuilder::new(
            "SELECT c.id, c.slug, c.archived FROM conversations c WHERE c.id IN ",
        );
        query.push_tuples(conversation_ids.iter(), |mut tuple, conversation_id| {
            tuple.push_bind(conversation_id);
        });
        query.push(
            " AND c.user_initiated = 1 AND c.runtime_role = 'user' \
              AND c.parent_conversation_id IS NULL \
              AND NOT (c.archived = 1 AND EXISTS (\
                  SELECT 1 FROM conversation_creation_jobs j \
                  WHERE j.conversation_id = c.id AND j.status = 'deletion_pending'\
              ))",
        );

        let rows = query.build().fetch_all(&self.pool).await?;
        let mut metadata = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            let slug: String = row.try_get("slug")?;
            let archived: bool = row.try_get("archived")?;
            metadata.insert(id, ConversationSearchMetadata { slug, archived });
        }
        Ok(metadata)
    }

    /// Return current transcript generations for a bounded conversation set.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn conversation_transcript_generations(
        &self,
        conversation_ids: &[String],
    ) -> DbResult<std::collections::HashMap<String, (i64, i64)>> {
        if conversation_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut query = sqlx::QueryBuilder::new(
            "SELECT c.id, c.transcript_generation, COUNT(m.message_id) AS message_count \
             FROM conversations c LEFT JOIN messages m ON m.conversation_id = c.id \
             WHERE c.id IN ",
        );
        query.push_tuples(conversation_ids.iter(), |mut tuple, id| {
            tuple.push_bind(id);
        });
        query.push(" GROUP BY c.id, c.transcript_generation");
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("id")?,
                    (
                        row.try_get("transcript_generation")?,
                        row.try_get("message_count")?,
                    ),
                ))
            })
            .collect()
    }

    /// Return the subset of message ids that remain visible to retrieval callers.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn visible_retrieval_message_ids(
        &self,
        message_ids: &[String],
    ) -> DbResult<std::collections::HashSet<String>> {
        if message_ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let mut query = sqlx::QueryBuilder::new(
            "SELECT message_id FROM messages WHERE COALESCE(json_extract(display_data, '$.hidden'), 0) != 1 AND message_id IN ",
        );
        query.push_tuples(message_ids.iter(), |mut tuple, message_id| {
            tuple.push_bind(message_id);
        });
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| row.try_get("message_id").map_err(DbError::from))
            .collect()
    }

    /// List conversation ids eligible for command-palette content search.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_conversation_search_ids(&self) -> DbResult<Vec<String>> {
        let rows = sqlx::query(
            "SELECT c.id FROM conversations c \
             WHERE c.user_initiated = 1 AND c.runtime_role = 'user' \
               AND c.parent_conversation_id IS NULL \
               AND NOT (c.archived = 1 AND EXISTS (\
                   SELECT 1 FROM conversation_creation_jobs j \
                   WHERE j.conversation_id = c.id AND j.status = 'deletion_pending'\
               ))",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row.try_get("id").map_err(DbError::from))
            .collect()
    }

    /// List active (non-archived) user-initiated conversations.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_conversations(&self) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.archived = 0 AND c.user_initiated = 1
               AND c.runtime_role = 'user'
             ORDER BY c.updated_at DESC",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Top-level conversations parked in a usage-limit error that carries a
    /// reset time, projected to `(id, state)`.
    ///
    /// Pre-filtered in SQL to the usage-limit error shape so the auto-clear
    /// sweep does not hydrate every active conversation — nor run the per-row
    /// `message_count` subquery that `list_conversations` carries — on every
    /// tick. The `resets_at <= now` comparison is left to the caller against
    /// the parsed state. `json_extract` on the `state` column mirrors
    /// `materialize_in_flight_tool_rounds` and `reset_all_to_idle`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_usage_limit_errors(&self) -> DbResult<Vec<(String, schema::ConvState)>> {
        let rows: Vec<(String, String)> = sqlx::query(
            "SELECT id, state FROM conversations
             WHERE archived = 0
               AND (user_initiated = 1 OR runtime_role = 'coordinator')
               AND state_kind = 'error'
               AND json_extract(state, '$.error_kind') = 'usage_limit_reached'
               AND json_extract(state, '$.resets_at') IS NOT NULL",
        )
        .try_map(|row: SqliteRow| Ok((row.try_get("id")?, row.try_get("state")?)))
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (id, state_json) in rows {
            match serde_json::from_str::<schema::ConvState>(&state_json) {
                Ok(state) => out.push((id, state)),
                Err(e) => tracing::warn!(
                    conv_id = %id, error = %e,
                    "list_usage_limit_errors: skipping conversation with unparseable state"
                ),
            }
        }
        Ok(out)
    }

    /// Working directories Phoenix is serving, for `/preview` path containment.
    ///
    /// Returns every conversation's `cwd` and managed `worktree_path` (archived
    /// included — the directory may still exist on disk). A preview request is
    /// only honoured for files resolving inside one of these roots, bounding the
    /// preview surface to the same filesystem scope the agent's own tools reach.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn preview_roots(&self) -> DbResult<Vec<String>> {
        let rows = sqlx::query_scalar::<_, Option<String>>(
            "WITH RECURSIVE coordinator_chain(id) AS (
                 SELECT id FROM conversations WHERE coordinator_head = 1
                 UNION
                 SELECT c.id
                 FROM conversations c
                 JOIN coordinator_chain cc ON c.continued_in_conv_id = cc.id
                 UNION
                 SELECT c.continued_in_conv_id
                 FROM conversations c
                 JOIN coordinator_chain cc ON c.id = cc.id
                 WHERE c.continued_in_conv_id IS NOT NULL
             )
             SELECT e.cwd
             FROM work_scope_environments e
             JOIN conversations c ON c.work_scope_id = e.work_scope_id
               WHERE e.cwd IS NOT NULL AND e.cwd != ''
                 AND c.id NOT IN (SELECT id FROM coordinator_chain)
             UNION
             SELECT e.worktree_path
             FROM work_scope_environments e
             JOIN conversations c ON c.work_scope_id = e.work_scope_id
               WHERE e.environment_kind = 'allocated_worktree'
                 AND c.id NOT IN (SELECT id FROM coordinator_chain)",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().flatten().collect())
    }

    /// Distinct Phoenix-created worktree paths known to the database.
    ///
    /// This is a disk-accounting inventory, not a liveness/ownership query: terminal
    /// and archived rows are included because an existing directory at a persisted
    /// managed path is still Phoenix-created disk usage, even when normal lifecycle
    /// cleanup should already have removed it.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn managed_worktree_paths(&self) -> DbResult<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            "SELECT worktree_path FROM work_scope_environments
              WHERE environment_kind = 'allocated_worktree'
              ORDER BY worktree_path",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// List conversation-to-scope attachments for one `WorkScope`.
    ///
    /// Each returned row is a conversation attachment projected by the schema view.
    ///
    /// # Errors
    ///
    /// Returns a database error when the attachment projection cannot be read or a
    /// persisted conversation row cannot be decoded.
    pub async fn conversation_work_scope_attachments(
        &self,
        work_scope_id: &WorkScopeId,
    ) -> DbResult<Vec<Conversation>> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, attachment.work_scope_id, c.transcript_generation,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             JOIN conversation_work_scope_attachments attachment
               ON attachment.conversation_id = c.id
             LEFT JOIN work_scope_environments e ON e.work_scope_id = attachment.work_scope_id
             WHERE attachment.work_scope_id = ?1
             ORDER BY c.created_at, c.id",
        )
        .bind(work_scope_id.as_str())
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    async fn reclaim_abandoned_product_root_reservations_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        now: chrono::DateTime<Utc>,
    ) -> DbResult<()> {
        sqlx::query(
            "DELETE FROM product_root_reservations
             WHERE status = 'reserved'
               AND created_at_unix_micros < ?1",
        )
        .bind(product_root_reservation_reclaim_before(now))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Persist one server-owned pre-creation root reservation.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when persistence fails.
    pub async fn insert_product_root_reservation(
        &self,
        reservation: &ProductRootReservationRecord,
    ) -> DbResult<()> {
        let now = Utc::now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::reclaim_abandoned_product_root_reservations_tx(&mut tx, now).await?;
        sqlx::query(
            "INSERT INTO product_root_reservations
             (id, cwd, kind, repo_root, repository_id, exact_checkout_oid, logical_base, freshness, unresolved_reason, status, created_at_unix_micros)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'reserved', ?10)",
        )
        .bind(&reservation.id)
        .bind(&reservation.cwd)
        .bind(&reservation.kind)
        .bind(&reservation.repo_root)
        .bind(&reservation.repository_id)
        .bind(&reservation.exact_checkout_oid)
        .bind(&reservation.logical_base)
        .bind(&reservation.freshness)
        .bind(&reservation.unresolved_reason)
        .bind(now.timestamp_micros())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Load one server-owned reservation by its opaque identity and canonical cwd.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the reservation query fails.
    pub async fn get_product_root_reservation(
        &self,
        reservation_id: &str,
        cwd: &str,
    ) -> DbResult<Option<ProductRootReservationRecord>> {
        let now = Utc::now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        Self::reclaim_abandoned_product_root_reservations_tx(&mut tx, now).await?;
        let reservation = sqlx::query(
            "SELECT id, cwd, kind, repo_root, repository_id, exact_checkout_oid, logical_base, freshness, unresolved_reason
               FROM product_root_reservations
              WHERE id = ?1 AND cwd = ?2",
        )
        .bind(reservation_id)
        .bind(cwd)
        .try_map(|row| {
            Ok(ProductRootReservationRecord {
                id: row.get("id"),
                cwd: row.get("cwd"),
                kind: row.get("kind"),
                repo_root: row.get("repo_root"),
                repository_id: row.get("repository_id"),
                exact_checkout_oid: row.get("exact_checkout_oid"),
                logical_base: row.get("logical_base"),
                freshness: row.get("freshness"),
                unresolved_reason: row.get("unresolved_reason"),
            })
        })
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(reservation)
    }

    /// Attach normalized hidden repository authority to an existing conversation `WorkScope`.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when attachment validation or the transaction fails.
    #[allow(clippy::too_many_lines)]
    pub async fn attach_hidden_git_repository_to_conversation_work_scope(
        &self,
        input: &AttachHiddenGitRepositoryInput,
        job_id: &str,
        claim: &CreationClaim,
    ) -> DbResult<(CreationCasOutcome, Option<AttachedHiddenGitRepository>)> {
        let normalized_common_dir = normalize_hidden_git_repository_path(&input.common_dir)?;
        let normalized_management_root =
            normalize_hidden_git_repository_path(&input.management_root)?;
        let observed_at_unix_micros = datetime_to_unix_micros(input.observed_at);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let owns_claim: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversation_creation_jobs
             WHERE id = ?1 AND status = 'claimed' AND generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4 AND lease_until > ?5",
        )
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&mut *tx)
        .await?;
        if owns_claim == 0 {
            tx.rollback().await?;
            return Ok((CreationCasOutcome::ClaimLost, None));
        }
        let row = sqlx::query(
            "SELECT c.cm_kind, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.work_scope_id,
                    e.branch_name AS env_branch_name,
                    e.worktree_path AS env_worktree_path,
                    e.base_branch AS env_base_branch
               FROM conversations c
               LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
              WHERE c.id = ?1",
        )
        .bind(&input.conversation_id)
        .fetch_one(&mut *tx)
        .await?;
        let work_scope_id =
            if let Some(existing_scope_id) = row.get::<Option<String>, _>("work_scope_id") {
                WorkScopeId::parse(existing_scope_id)
                    .map_err(|error| DbError::Serialization(error.to_string()))?
            } else {
                let cm_kind = row.get::<String, _>("cm_kind");
                let cm_task_id = row.get::<Option<String>, _>("cm_task_id");
                let cm_task_title = row.get::<Option<String>, _>("cm_task_title");
                let cm_next_hint = row
                    .get::<Option<i64>, _>("cm_next_taskmd_id_hint")
                    .map(|value| value.to_string());
                let env_worktree_path = row.get::<Option<String>, _>("env_worktree_path");
                let env_branch_name = row.get::<Option<String>, _>("env_branch_name");
                let env_base_branch = row.get::<Option<String>, _>("env_base_branch");
                let cm = ConvModeCols {
                    kind: match cm_kind.as_str() {
                        "direct" => "direct",
                        "work" => "work",
                        "explore" => "explore",
                        other => {
                            return Err(DbError::Serialization(format!(
                                "invalid deferred scope mode {other}"
                            )))
                        }
                    },
                    task_id: cm_task_id.as_deref(),
                    task_title: cm_task_title.as_deref(),
                    next_taskmd_id_hint: cm_next_hint.as_deref(),
                    worktree_path: env_worktree_path.as_deref(),
                    branch_name: env_branch_name.as_deref(),
                    base_branch: env_base_branch.as_deref(),
                };
                let (scope_id, authority_kind, environment) =
                    Self::new_scope_for_conversation(&input.materialized_worktree, &cm);
                let now = Utc::now().to_rfc3339();
                Self::insert_work_scope_tx(&mut tx, &scope_id, authority_kind, environment, &now)
                    .await?;
                sqlx::query(
                    "UPDATE conversations
                    SET work_scope_id = ?1,
                        updated_at = ?3
                  WHERE id = ?2
                    AND work_scope_id IS NULL",
                )
                .bind(scope_id.as_str())
                .bind(&input.conversation_id)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
                scope_id
            };

        let repository_id = if let Some(existing_id) = sqlx::query_scalar::<_, String>(
            "SELECT wr.repository_id
               FROM conversation_creation_jobs job
               JOIN conversations c ON c.id = job.conversation_id
               JOIN work_scope_git_repositories wr ON wr.work_scope_id = c.work_scope_id
              WHERE job.id = ?1 AND job.status = 'claimed' AND job.generation = ?2
                AND job.claim_worker_id = ?3 AND job.claim_token = ?4 AND job.lease_until > ?5",
        )
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(&mut *tx)
        .await?
        {
            phoenix_core::git_repository::GitRepositoryId::parse(existing_id)
                .map_err(|error| DbError::Serialization(error.to_string()))?
        } else {
            let repository_id = phoenix_core::git_repository::GitRepositoryId::parse(
                uuid::Uuid::new_v4().to_string(),
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?;
            sqlx::query("INSERT INTO git_repositories (id) VALUES (?1)")
                .bind(repository_id.as_str())
                .execute(&mut *tx)
                .await?;
            repository_id
        };

        for (locator_kind, path) in [
            ("common_dir", normalized_common_dir.as_str()),
            ("management_root", normalized_management_root.as_str()),
        ] {
            sqlx::query(
                "INSERT INTO git_repository_locator_observations (
                    repository_id, locator_kind, status, path, observed_at_unix_micros
                 ) VALUES (?1, ?2, 'present', ?3, ?4)
                 ON CONFLICT(repository_id, locator_kind)
                 DO UPDATE SET status = excluded.status,
                               path = excluded.path,
                               observed_at_unix_micros = excluded.observed_at_unix_micros",
            )
            .bind(repository_id.as_str())
            .bind(locator_kind)
            .bind(path)
            .bind(observed_at_unix_micros)
            .execute(&mut *tx)
            .await?;
        }

        let next_default_branch_generation: i64 = sqlx::query_scalar(
            "SELECT COALESCE(generation, 0) + 1
               FROM git_repository_default_branch_observations
              WHERE repository_id = ?1",
        )
        .bind(repository_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(1);

        let (status, branch, provenance) = match &input.default_branch {
            GitRepositoryDefaultBranchObservation::Resolved { branch, provenance } => {
                ("resolved", Some(branch.as_str()), Some(provenance.as_str()))
            }
            GitRepositoryDefaultBranchObservation::Unresolved => ("unresolved", None, None),
        };
        sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(repository_id)
             DO UPDATE SET generation = excluded.generation,
                           status = excluded.status,
                           branch = excluded.branch,
                           provenance = excluded.provenance,
                           observed_at_unix_micros = excluded.observed_at_unix_micros
                   WHERE excluded.observed_at_unix_micros > git_repository_default_branch_observations.observed_at_unix_micros
                      OR (
                          excluded.observed_at_unix_micros = git_repository_default_branch_observations.observed_at_unix_micros
                          AND excluded.status = git_repository_default_branch_observations.status
                          AND excluded.branch IS git_repository_default_branch_observations.branch
                          AND excluded.provenance IS git_repository_default_branch_observations.provenance
                      )",
        )
        .bind(repository_id.as_str())
        .bind(next_default_branch_generation)
        .bind(status)
        .bind(branch)
        .bind(provenance)
        .bind(observed_at_unix_micros)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id)
             VALUES (?1, ?2)
             ON CONFLICT(work_scope_id)
             DO UPDATE SET repository_id = excluded.repository_id",
        )
        .bind(work_scope_id.as_str())
        .bind(repository_id.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((
            CreationCasOutcome::Applied,
            Some(AttachedHiddenGitRepository {
                work_scope_id,
                repository_id,
            }),
        ))
    }

    /// Resolve retained hidden-repository identity evidence for a repository management root.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the locator lookup fails or the retained repository id is invalid.
    pub async fn retained_hidden_repository_id_for_management_root(
        &self,
        management_root: &str,
    ) -> DbResult<Option<phoenix_core::git_repository::GitRepositoryId>> {
        let normalized_management_root = normalize_hidden_git_repository_path(management_root)?;
        let row = sqlx::query(
            "SELECT repository_id
               FROM git_repository_locator_observations
              WHERE locator_kind = 'management_root'
                AND status = 'present'
                AND path = ?1
              ORDER BY observed_at_unix_micros DESC, repository_id DESC
              LIMIT 1",
        )
        .bind(normalized_management_root)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            phoenix_core::git_repository::GitRepositoryId::parse(
                row.get::<String, _>("repository_id"),
            )
            .map_err(|error| DbError::Serialization(error.to_string()))
        })
        .transpose()
    }

    /// List one present management-root observation per hidden repository by recency.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the query or timestamp conversion fails.
    pub async fn list_recent_hidden_repository_management_roots(
        &self,
    ) -> DbResult<Vec<RecentHiddenRepositoryManagementRoot>> {
        let rows = sqlx::query(
            "WITH latest_management_root AS (
                 SELECT repository_id, path, observed_at_unix_micros,
                        ROW_NUMBER() OVER (
                            PARTITION BY repository_id
                            ORDER BY observed_at_unix_micros DESC, path DESC
                        ) AS row_num
                   FROM git_repository_locator_observations
                  WHERE locator_kind = 'management_root' AND status = 'present'
             ),
             active_repository_evidence AS (
                 SELECT DISTINCT wr.repository_id,
                        c.product_conversation_id,
                        MAX(MAX(c.updated_at, c.state_updated_at)) AS updated_at
                   FROM work_scope_git_repositories wr
                   JOIN conversation_work_scope_attachments cwa
                     ON cwa.work_scope_id = wr.work_scope_id
                   JOIN conversations c ON c.id = cwa.conversation_id
                   JOIN product_conversations pc ON pc.id = c.product_conversation_id
                  WHERE pc.kind = 'ordinary'
                    AND pc.ordinary_lifecycle IN ('open', 'history')
                  GROUP BY wr.repository_id, c.product_conversation_id
                 UNION
                 SELECT DISTINCT wr.repository_id,
                        c.product_conversation_id,
                        MAX(MAX(c.updated_at, c.state_updated_at)) AS updated_at
                   FROM work_scope_git_repositories wr
                   JOIN conversations c ON c.id = (
                        SELECT reservation.consumed_by_conversation_id
                          FROM product_root_reservations reservation
                         WHERE reservation.status = 'consumed'
                           AND reservation.unresolved_reason IS NOT NULL
                           AND reservation.repo_root IS NOT NULL
                           AND reservation.repo_root = (
                               SELECT management.path
                                 FROM git_repository_locator_observations management
                                WHERE management.repository_id = wr.repository_id
                                  AND management.locator_kind = 'management_root'
                                  AND management.status = 'present'
                                ORDER BY management.observed_at_unix_micros DESC, management.path DESC
                                LIMIT 1
                           )
                         ORDER BY reservation.consumed_at_unix_micros DESC, reservation.id DESC
                         LIMIT 1
                   )
                   JOIN product_conversations pc ON pc.id = c.product_conversation_id
                  WHERE pc.kind = 'ordinary'
                    AND pc.ordinary_lifecycle IN ('open', 'history')
                  GROUP BY wr.repository_id, c.product_conversation_id

             )
             SELECT repository_id, path, observed_at_unix_micros
               FROM latest_management_root roots
              WHERE row_num = 1
                AND EXISTS (
                    SELECT 1
                      FROM active_repository_evidence evidence
                     WHERE evidence.repository_id = roots.repository_id
                )
              ORDER BY (
                    SELECT COUNT(*)
                      FROM active_repository_evidence evidence
                     WHERE evidence.repository_id = roots.repository_id
                ) DESC,
                (
                    SELECT MAX(evidence.updated_at)
                      FROM active_repository_evidence evidence
                     WHERE evidence.repository_id = roots.repository_id
                ) DESC,
                observed_at_unix_micros DESC, repository_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let repository_id = phoenix_core::git_repository::GitRepositoryId::parse(
                    row.get::<String, _>("repository_id"),
                )
                .map_err(|error| DbError::Serialization(error.to_string()))?;
                let management_root = row.get::<String, _>("path");
                let observed_at = unix_micros_to_datetime(
                    row.get::<i64, _>("observed_at_unix_micros"),
                    "observed_at_unix_micros",
                )?;
                Ok(RecentHiddenRepositoryManagementRoot {
                    repository_id,
                    management_root,
                    observed_at,
                })
            })
            .collect()
    }

    /// Conversations with persisted Phoenix-created worktree paths, including
    /// archived and terminal rows for disk disposition.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn managed_worktree_conversations(&self) -> DbResult<Vec<Conversation>> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE e.environment_kind = 'allocated_worktree'
             ORDER BY e.worktree_path, c.updated_at DESC",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// List every conversation, including archived and non-user-initiated rows.
    /// This is used when durable topology must be reconstructed before applying
    /// feature-specific visibility policy.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_all_conversations(&self) -> DbResult<Vec<Conversation>> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             ORDER BY c.updated_at DESC",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// List archived conversations
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_archived_conversations(&self) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.archived = 1 AND c.user_initiated = 1 AND c.runtime_role = 'user'
               AND NOT EXISTS (
                   SELECT 1 FROM conversation_creation_jobs j
                   WHERE j.conversation_id = c.id AND j.status = 'deletion_pending'
               )
             ORDER BY c.updated_at DESC",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Insert a durable async conversation-creation job.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the insert fails, the intent cannot be serialized,
    /// or the inserted row cannot be read back.
    pub async fn insert_conversation_creation_job(
        &self,
        job: &InsertConversationCreationJob,
    ) -> DbResult<ConversationCreationJob> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let intent_json = serde_json::to_string(&job.intent)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                id, conversation_id, message_id, status, stage, attempt, generation,
                intent_json, error, accepted_at, provisioning_started_at, completed_at,
                failed_at, cancelled_at, deletion_requested_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'accepted', 'validate_intent', 0, 0,
                       ?4, NULL, ?5, NULL, NULL, NULL, NULL, NULL, ?5, ?5)",
        )
        .bind(&job.id)
        .bind(&job.conversation_id)
        .bind(&job.message_id)
        .bind(intent_json)
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;
        insert_creation_job_files_tx(&mut tx, job).await?;
        insert_creation_job_images_tx(&mut tx, job).await?;
        tx.commit().await?;
        self.get_conversation_creation_job(&job.id).await
    }

    /// Create the visible async-conversation shell and its durable replay job atomically.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if either row cannot be inserted or the job intent cannot be serialized.
    ///
    /// # Panics
    ///
    /// Panics if the initial provisioning state cannot be serialized.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn create_conversation_with_creation_job(
        &self,
        id: &str,
        slug: &str,
        cwd: &str,
        user_initiated: bool,
        model: Option<&str>,
        conv_mode: &ConvMode,
        desired_base_branch: Option<&str>,
        seed_parent_id: Option<&str>,
        seed_label: Option<&str>,
        llm_language: phoenix_core::llm_language::LlmLanguage,
        job: &InsertConversationCreationJob,
        root_reservation_id: Option<&str>,
    ) -> DbResult<Conversation> {
        let now = Utc::now();
        let creation_state = ConvState::Provisioning {
            job_id: job.id.clone(),
            phase: ConversationCreationPhase::Accepted,
        };
        let creation_state_json = serde_json::to_string(&creation_state).unwrap();
        let cm = conv_mode_columns(conv_mode);
        let now_str = now.to_rfc3339();
        let intent_json = serde_json::to_string(&job.intent)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let mut tx = self.pool.begin().await?;
        if let Some(reservation_id) = root_reservation_id {
            let consumed = sqlx::query(
                "UPDATE product_root_reservations
                 SET status = 'consumed', consumed_by_conversation_id = ?1,
                     consumed_at_unix_micros = ?2
                 WHERE id = ?3
                   AND (status = 'reserved'
                        OR (status = 'consumed' AND consumed_by_conversation_id = ?1))",
            )
            .bind(id)
            .bind(now.timestamp_micros())
            .bind(reservation_id)
            .execute(&mut *tx)
            .await?;
            if consumed.rows_affected() != 1 {
                return Err(DbError::Serialization(
                    "product root reservation is missing or belongs to another conversation"
                        .to_string(),
                ));
            }
        }
        let deferred_scope = root_reservation_id.is_some();
        let scope = (!deferred_scope).then(|| Self::new_scope_for_conversation(cwd, &cm));
        let product_conversation_id =
            phoenix_core::domain::product_conversation::ProductConversationId::new();
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES (?1, 'ordinary', 'open')",
        )
        .bind(product_conversation_id.as_str())
        .execute(&mut *tx)
        .await?;
        if let Some((scope_id, authority_kind, environment)) = scope.as_ref() {
            Self::insert_work_scope_tx(
                &mut tx,
                scope_id,
                *authority_kind,
                environment.clone(),
                &now_str,
            )
            .await?;
        }

        let mut actual_slug = slug.to_string();
        let mut attempts = 0u8;
        loop {
            let title_str = schema::title_from_slug(&actual_slug);
            let result = sqlx::query(
                "INSERT INTO conversations (id, product_conversation_id, slug, title, parent_conversation_id, user_initiated, state, state_kind, state_updated_at, created_at, updated_at, archived, model, effort, project_id, desired_base_branch, seed_parent_id, seed_label, llm_language, cm_kind, cm_task_id, cm_task_title, cm_next_taskmd_id_hint, runtime_role, work_scope_id)
                 VALUES (?1, ?19, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?7, ?7, 0, ?8, ?9, NULL, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 'user', ?18)",
            )
            .bind(id)
            .bind(&actual_slug)
            .bind(&title_str)
            .bind(user_initiated)
            .bind(&creation_state_json)
            .bind(conv_state_kind(&creation_state))
            .bind(&now_str)
            .bind(model)
            .bind(job.intent.effort.map(ModelEffort::as_wire_name))
            .bind(desired_base_branch)
            .bind(seed_parent_id)
            .bind(seed_label)
            .bind(llm_language.as_str())
            .bind(cm.kind)
            .bind(cm.task_id)
            .bind(cm.task_title)
            .bind(cm.next_taskmd_id_hint)
            .bind(scope.as_ref().map(|(scope_id, _, _)| scope_id.as_str()))
            .bind(product_conversation_id.as_str())
            .execute(&mut *tx)
            .await;

            match result {
                Ok(_) => break,
                Err(sqlx::Error::Database(ref e))
                    if (is_sqlite_unique_constraint(e.as_ref())
                        || is_sqlite_primary_key_constraint(e.as_ref()))
                        && e.message().contains("conversations.id") =>
                {
                    return Err(DbError::ConversationAlreadyExists(id.to_string()));
                }
                Err(sqlx::Error::Database(ref e)) if is_sqlite_unique_constraint(e.as_ref()) => {
                    attempts += 1;
                    if attempts >= 10 {
                        let uuid_str = uuid::Uuid::new_v4().to_string();
                        actual_slug = format!("{slug}-{}", uuid_str.get(..8).unwrap_or(&uuid_str));
                    } else {
                        actual_slug = format!("{slug}-{:04x}", rand::random::<u16>());
                    }
                }
                Err(e) => return Err(DbError::Sqlx(e)),
            }
        }

        sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                id, conversation_id, message_id, status, stage, attempt, generation,
                intent_json, error, accepted_at, provisioning_started_at, completed_at,
                failed_at, cancelled_at, deletion_requested_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'accepted', 'validate_intent', 0, 0,
                       ?4, NULL, ?5, NULL, NULL, NULL, NULL, NULL, ?5, ?5)",
        )
        .bind(&job.id)
        .bind(&job.conversation_id)
        .bind(&job.message_id)
        .bind(intent_json)
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;
        insert_creation_job_files_tx(&mut tx, job).await?;
        insert_creation_job_images_tx(&mut tx, job).await?;

        tx.commit().await?;
        let title = schema::title_from_slug(&actual_slug);
        Ok(Conversation {
            id: id.to_string(),
            product_conversation_id,
            slug: Some(actual_slug),
            title: Some(title),
            cwd: cwd.to_string(),
            parent_conversation_id: None,
            user_initiated,
            state: creation_state,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: model.map(String::from),
            project_id: None,
            conv_mode: conv_mode.clone(),
            runtime_role: RuntimeRole::User,
            effort: job.intent.effort,
            service_tier: ServiceTier::Standard,
            attached_work_scope_id: scope.map(|(scope_id, _, _)| scope_id),
            desired_base_branch: desired_base_branch.map(String::from),
            message_count: 0,
            seed_parent_id: seed_parent_id.map(String::from),
            seed_label: seed_label.map(String::from),
            continued_in_conv_id: None,
            chain_name: None,
            llm_language,
            spawned_from_conversation_id: None,
            transcript_generation: 1,
        })
    }

    /// Load a creation job by id.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the database read fails or no row exists for `job_id`.
    pub async fn get_conversation_creation_job(
        &self,
        job_id: &str,
    ) -> DbResult<ConversationCreationJob> {
        sqlx::query(
            "SELECT id, conversation_id, message_id, status, stage, attempt, generation,
                    claim_worker_id, claim_token, lease_until, next_attempt_at,
                    intent_json, error, accepted_at, provisioning_started_at,
                    completed_at, failed_at, created_at, updated_at
             FROM conversation_creation_jobs WHERE id = ?1",
        )
        .bind(job_id)
        .try_map(parse_conversation_creation_job_row)
        .fetch_one(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// Load the creation job for a conversation, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the database read fails.
    pub async fn get_conversation_creation_job_for_conversation(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<ConversationCreationJob>> {
        sqlx::query(
            "SELECT id, conversation_id, message_id, status, stage, attempt, generation,
                    claim_worker_id, claim_token, lease_until, next_attempt_at,
                    intent_json, error, accepted_at, provisioning_started_at,
                    completed_at, failed_at, created_at, updated_at
             FROM conversation_creation_jobs WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .try_map(parse_conversation_creation_job_row)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// Load the creation job for an initial message id, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the database read fails.
    pub async fn get_conversation_creation_job_for_message(
        &self,
        message_id: &str,
    ) -> DbResult<Option<ConversationCreationJob>> {
        sqlx::query(
            "SELECT id, conversation_id, message_id, status, stage, attempt, generation,
                    claim_worker_id, claim_token, lease_until, next_attempt_at,
                    intent_json, error, accepted_at, provisioning_started_at,
                    completed_at, failed_at, created_at, updated_at
             FROM conversation_creation_jobs WHERE message_id = ?1",
        )
        .bind(message_id)
        .try_map(parse_conversation_creation_job_row)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// Load normalized file attachments for a creation job.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the query fails or a stored size is invalid.
    pub async fn get_conversation_creation_job_files(
        &self,
        job_id: &str,
    ) -> DbResult<Vec<FileAttachment>> {
        let rows = sqlx::query(
            "SELECT original_name, media_type, size_bytes, stored_path
             FROM conversation_creation_job_files
             WHERE job_id = ?1
             ORDER BY ordinal ASC",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let size = row.get::<i64, _>("size_bytes");
                let size_bytes = u64::try_from(size).map_err(|_| {
                    DbError::Serialization(
                        "negative attachment size in creation job file".to_string(),
                    )
                })?;
                Ok(FileAttachment {
                    original_name: row.get("original_name"),
                    media_type: row.get("media_type"),
                    size_bytes,
                    stored_path: row.get("stored_path"),
                })
            })
            .collect()
    }

    /// Load normalized image attachments for a creation job.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the query fails.
    pub async fn get_conversation_creation_job_images(
        &self,
        job_id: &str,
    ) -> DbResult<Vec<ImageData>> {
        let rows = sqlx::query(
            "SELECT media_type, data
             FROM conversation_creation_job_images
             WHERE job_id = ?1
             ORDER BY ordinal ASC",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ImageData {
                    media_type: row.get("media_type"),
                    data: row.get("data"),
                })
            })
            .collect()
    }

    /// Atomically claim the oldest eligible creation job.
    ///
    /// Accepted jobs consume attempt one, due retries consume the next attempt,
    /// and expired-lease takeover only increments the fencing generation.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the atomic claim transaction or row decoding fails.
    pub async fn claim_next_conversation_creation_job(
        &self,
        worker_id: &CreationWorkerId,
        token: &CreationClaimToken,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<CreationClaimOutcome> {
        let now_str = now.to_rfc3339();
        let lease_until = (now + lease_duration).to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let candidate: Option<(String, String)> = sqlx::query_as(
            "SELECT id, status
             FROM conversation_creation_jobs
             WHERE status = 'accepted'
                OR (status = 'retry_scheduled' AND next_attempt_at <= ?1)
                OR (status = 'claimed' AND lease_until <= ?1)
             ORDER BY accepted_at ASC, id ASC
             LIMIT 1",
        )
        .bind(&now_str)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((job_id, prior_status)) = candidate else {
            tx.rollback().await?;
            return Ok(CreationClaimOutcome::NoEligibleJob);
        };

        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'claimed',
                 attempt = CASE
                     WHEN status = 'accepted' THEN 1
                     WHEN status = 'retry_scheduled' THEN attempt + 1
                     ELSE attempt
                 END,
                 generation = generation + 1,
                 claim_worker_id = ?1,
                 claim_token = ?2,
                 lease_until = ?3,
                 next_attempt_at = NULL,
                 provisioning_started_at = COALESCE(provisioning_started_at, ?4),
                 updated_at = ?4
             WHERE id = ?5
               AND status = ?6
               AND (
                   status = 'accepted'
                   OR (status = 'retry_scheduled' AND next_attempt_at <= ?4)
                   OR (status = 'claimed' AND lease_until <= ?4)
               )",
        )
        .bind(&worker_id.0)
        .bind(&token.0)
        .bind(&lease_until)
        .bind(&now_str)
        .bind(&job_id)
        .bind(&prior_status)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(CreationClaimOutcome::NoEligibleJob);
        }
        tx.commit().await?;
        self.get_conversation_creation_job(&job_id)
            .await
            .map(Box::new)
            .map(CreationClaimOutcome::Claimed)
    }

    /// Return the earliest durable scheduler deadline, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the database read fails.
    pub async fn next_conversation_creation_deadline(&self) -> DbResult<Option<DateTime<Utc>>> {
        let deadline: Option<String> = sqlx::query_scalar(
            "SELECT MIN(deadline) FROM (
                 SELECT next_attempt_at AS deadline
                 FROM conversation_creation_jobs
                 WHERE status = 'retry_scheduled'
                 UNION ALL
                 SELECT lease_until AS deadline
                 FROM conversation_creation_jobs
                 WHERE status = 'claimed'
                 UNION ALL
                 SELECT CASE
                            WHEN cleanup_lease_until IS NOT NULL
                            THEN cleanup_lease_until
                            ELSE updated_at
                        END AS deadline
                 FROM conversation_creation_jobs
                 WHERE status IN ('cancelling', 'deletion_pending')
                    OR (status = 'failed' AND EXISTS (
                        SELECT 1 FROM conversation_creation_resource_reservations r
                        WHERE r.job_id = conversation_creation_jobs.id
                          AND r.status != 'released'
                    ))
             )",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(deadline.as_deref().map(parse_datetime))
    }

    /// Revoke provisioning authority and preserve a visible cancelled record.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the transaction fails or no creation job exists.
    pub async fn cancel_conversation_creation(
        &self,
        conversation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<()> {
        let now = now.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let job: Option<(String, i64)> = sqlx::query_as(
            "SELECT id, generation FROM conversation_creation_jobs
             WHERE conversation_id = ?1
               AND status IN ('accepted', 'claimed', 'retry_scheduled')",
        )
        .bind(conversation_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((job_id, generation)) = job else {
            tx.rollback().await?;
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        };
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'cancelling', generation = generation + 1,
                 claim_worker_id = NULL, claim_token = NULL, lease_until = NULL,
                 cleanup_worker_id = NULL, cleanup_token = NULL, cleanup_lease_until = NULL,
                 next_attempt_at = NULL, updated_at = ?1
             WHERE id = ?2 AND generation = ?3
               AND status IN ('accepted', 'claimed', 'retry_scheduled')",
        )
        .bind(&now)
        .bind(&job_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        let state = serde_json::to_string(&ConvState::CreationCancelled {
            job_id: job_id.clone(),
        })
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
             WHERE id = ?4",
        )
        .bind(state)
        .bind(conv_state_kind(&ConvState::CreationCancelled {
            job_id: job_id.clone(),
        }))
        .bind(&now)
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE conversation_creation_resource_reservations
             SET generation = ?3, status = 'cleanup_required', updated_at = ?1
             WHERE job_id = ?2 AND status IN ('reserved', 'present')",
        )
        .bind(now)
        .bind(job_id)
        .bind(generation + 1)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Revoke provisioning authority and hide a deletion tombstone.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the transaction fails or no creation job exists.
    pub async fn request_conversation_creation_deletion(
        &self,
        conversation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<()> {
        let now = now.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let job: Option<(String, i64)> = sqlx::query_as(
            "SELECT id, generation FROM conversation_creation_jobs
             WHERE conversation_id = ?1
               AND status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'failed')",
        )
        .bind(conversation_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((job_id, generation)) = job else {
            tx.rollback().await?;
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        };
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'deletion_pending', generation = generation + 1,
                 claim_worker_id = NULL, claim_token = NULL, lease_until = NULL,
                 cleanup_worker_id = NULL, cleanup_token = NULL, cleanup_lease_until = NULL,
                 next_attempt_at = NULL, error = NULL, failed_at = NULL,
                 cancelled_at = NULL, deletion_requested_at = ?1, updated_at = ?1
             WHERE id = ?2 AND generation = ?3
               AND status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'failed')",
        )
        .bind(&now)
        .bind(&job_id)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        sqlx::query("UPDATE conversations SET archived = 1, updated_at = ?1 WHERE id = ?2")
            .bind(&now)
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE conversation_creation_resource_reservations
             SET generation = ?3, status = 'cleanup_required', updated_at = ?1
             WHERE job_id = ?2 AND status IN ('reserved', 'present')",
        )
        .bind(now)
        .bind(job_id)
        .bind(generation + 1)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Persist a claimed creation's resolved intent before external materialization.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if serialization or the update fails.
    pub async fn update_conversation_creation_job_intent(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        intent: &ConversationCreationIntent,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let intent_json = serde_json::to_string(intent)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let now_micros = now.timestamp_micros();
        let now = now.to_rfc3339();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET intent_json = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'claimed' AND generation = ?4
               AND claim_worker_id = ?5 AND claim_token = ?6 AND lease_until > ?2",
        )
        .bind(intent_json)
        .bind(&now)
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(CreationCasOutcome::ClaimLost);
        }
        if let (Some(oid), Some(branch)) = (
            intent.reserved_checkout_oid.as_deref(),
            intent.base_branch.as_deref(),
        ) {
            sqlx::query(
                "UPDATE product_root_reservations
                 SET kind = 'exact_committed_tree', exact_checkout_oid = ?1,
                     logical_base = ?2, freshness = ?3, unresolved_reason = NULL, consumed_at_unix_micros = ?4
                 WHERE consumed_by_conversation_id = (
                     SELECT conversation_id FROM conversation_creation_jobs WHERE id = ?5
                 ) AND status = 'consumed'",
            )
            .bind(oid)
            .bind(branch)
            .bind(intent.reserved_root_freshness.as_deref().unwrap_or("stale_cached"))
            .bind(now_micros)
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(CreationCasOutcome::Applied)
    }

    /// Atomically transition a seeded-empty creation to Idle and ready.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if serialization or the transaction fails.
    pub async fn complete_seeded_empty_conversation_creation(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        conversation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let idle = serde_json::to_string(&ConvState::Idle)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let now = now.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'ready', stage = 'finalize', completed_at = ?1, updated_at = ?1,
                 claim_worker_id = NULL, claim_token = NULL, lease_until = NULL
             WHERE id = ?2 AND conversation_id = ?3 AND status = 'claimed'
               AND generation = ?4 AND claim_worker_id = ?5 AND claim_token = ?6
               AND lease_until > ?1",
        )
        .bind(&now)
        .bind(job_id)
        .bind(conversation_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(CreationCasOutcome::ClaimLost);
        }
        let state_updated = sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
             WHERE id = ?4",
        )
        .bind(idle)
        .bind(conv_state_kind(&ConvState::Idle))
        .bind(&now)
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
        if state_updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(DbError::ConversationNotFound(conversation_id.to_string()));
        }
        tx.commit().await?;
        Ok(CreationCasOutcome::Applied)
    }

    /// Load one creation tombstone that requires resource reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the read fails.
    pub async fn claim_next_conversation_creation_cleanup(
        &self,
        worker_id: &str,
        token: &str,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<Option<CreationCleanupJob>> {
        let now_str = now.to_rfc3339();
        let lease_until = now + lease_duration;
        let mut tx = self.pool.begin().await?;
        let row: Option<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT id, conversation_id, intent_json, status, generation
             FROM conversation_creation_jobs
             WHERE updated_at <= ?1
               AND (cleanup_lease_until IS NULL OR cleanup_lease_until <= ?1)
               AND (
                   status IN ('cancelling', 'deletion_pending')
                   OR (status = 'failed' AND EXISTS (
                       SELECT 1 FROM conversation_creation_resource_reservations r
                       WHERE r.job_id = conversation_creation_jobs.id
                         AND r.status != 'released'
                   ))
               )
             ORDER BY updated_at, id LIMIT 1",
        )
        .bind(&now_str)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((job_id, conversation_id, intent_json, status, generation)) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let claimed = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET cleanup_worker_id = ?1, cleanup_token = ?2,
                 cleanup_lease_until = ?3
             WHERE id = ?4 AND generation = ?5
               AND (cleanup_lease_until IS NULL OR cleanup_lease_until <= ?6)",
        )
        .bind(worker_id)
        .bind(token)
        .bind(lease_until.to_rfc3339())
        .bind(&job_id)
        .bind(generation)
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        tx.commit().await?;
        let reservations = self.get_creation_resource_reservations(&job_id).await?;
        let intent = serde_json::from_str(&intent_json)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        Ok(Some(CreationCleanupJob {
            job_id,
            conversation_id,
            intent,
            status,
            generation: u64::try_from(generation)
                .map_err(|_| DbError::Serialization("negative cleanup generation".to_string()))?,
            worker_id: worker_id.to_string(),
            token: token.to_string(),
            lease_until,
            reservations,
        }))
    }

    /// Schedule another cleanup reconciliation attempt.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails.
    pub async fn schedule_conversation_creation_cleanup_retry(
        &self,
        cleanup: &CreationCleanupJob,
        next_attempt_at: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET updated_at = ?1, cleanup_worker_id = NULL, cleanup_token = NULL,
                 cleanup_lease_until = NULL
             WHERE id = ?2 AND generation = ?3
               AND status IN ('cancelling', 'deletion_pending', 'failed')
               AND cleanup_worker_id = ?4 AND cleanup_token = ?5",
        )
        .bind(next_attempt_at.to_rfc3339())
        .bind(&cleanup.job_id)
        .bind(i64::try_from(cleanup.generation).map_err(|_| {
            DbError::Serialization("cleanup generation exceeds SQLite integer".to_string())
        })?)
        .bind(&cleanup.worker_id)
        .bind(&cleanup.token)
        .execute(&self.pool)
        .await?;
        Ok(if result.rows_affected() == 1 {
            CreationCasOutcome::Applied
        } else {
            CreationCasOutcome::ClaimLost
        })
    }

    /// Mark one reserved resource released after reconciliation.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails.
    pub async fn release_creation_resource(
        &self,
        cleanup: &CreationCleanupJob,
        reservation_id: &str,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let result = sqlx::query(
            "UPDATE conversation_creation_resource_reservations
             SET status = 'released', updated_at = ?1
             WHERE id = ?2 AND job_id = ?3 AND generation = ?4
               AND status = 'cleanup_required'
               AND EXISTS (
                   SELECT 1 FROM conversation_creation_jobs j
                   WHERE j.id = ?3 AND j.generation = ?4
                     AND j.cleanup_worker_id = ?5 AND j.cleanup_token = ?6
                     AND j.cleanup_lease_until > ?1
               )",
        )
        .bind(now.to_rfc3339())
        .bind(reservation_id)
        .bind(&cleanup.job_id)
        .bind(i64::try_from(cleanup.generation).map_err(|_| {
            DbError::Serialization("cleanup generation exceeds SQLite integer".to_string())
        })?)
        .bind(&cleanup.worker_id)
        .bind(&cleanup.token)
        .execute(&self.pool)
        .await?;
        Ok(if result.rows_affected() == 1 {
            CreationCasOutcome::Applied
        } else {
            CreationCasOutcome::ClaimLost
        })
    }

    /// Finish cancellation or physically remove a reconciled deletion tombstone.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if cleanup is incomplete or the transaction fails.
    #[allow(clippy::too_many_lines)]
    pub async fn finish_conversation_creation_cleanup(
        &self,
        cleanup: &CreationCleanupJob,
        now: DateTime<Utc>,
    ) -> DbResult<()> {
        let now_str = now.to_rfc3339();
        let generation = i64::try_from(cleanup.generation).map_err(|_| {
            DbError::Serialization("cleanup generation exceeds SQLite integer".to_string())
        })?;
        let mut tx = self.pool.begin().await?;
        let authoritative: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM conversation_creation_jobs
             WHERE id = ?1 AND status = ?2 AND generation = ?3
               AND cleanup_worker_id = ?4 AND cleanup_token = ?5
               AND cleanup_lease_until > ?6",
        )
        .bind(&cleanup.job_id)
        .bind(&cleanup.status)
        .bind(generation)
        .bind(&cleanup.worker_id)
        .bind(&cleanup.token)
        .bind(&now_str)
        .fetch_optional(&mut *tx)
        .await?;
        if authoritative.is_none() {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "creation cleanup claim was lost".to_string(),
            ));
        }
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversation_creation_resource_reservations
             WHERE job_id = ?1 AND status != 'released'",
        )
        .bind(&cleanup.job_id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining != 0 {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "creation cleanup still has unreconciled resources".to_string(),
            ));
        }
        if cleanup.status == "cancelling" {
            let updated = sqlx::query(
                "UPDATE conversation_creation_jobs
                 SET status = 'cancelled', cancelled_at = ?1, updated_at = ?1,
                     cleanup_worker_id = NULL, cleanup_token = NULL,
                     cleanup_lease_until = NULL
                 WHERE id = ?2 AND status = 'cancelling' AND generation = ?3
                   AND cleanup_worker_id = ?4 AND cleanup_token = ?5
                   AND cleanup_lease_until > ?1",
            )
            .bind(&now_str)
            .bind(&cleanup.job_id)
            .bind(generation)
            .bind(&cleanup.worker_id)
            .bind(&cleanup.token)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                return Err(DbError::Serialization(
                    "creation cleanup claim was lost".to_string(),
                ));
            }
        } else if cleanup.status == "deletion_pending" {
            let work_scope_id: Option<String> =
                sqlx::query_scalar("SELECT work_scope_id FROM conversations WHERE id = ?1")
                    .bind(&cleanup.conversation_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let deleted = Self::hard_delete_conversation_tx(
                &mut tx,
                &cleanup.conversation_id,
                self.sqlite_telemetry(
                    SqliteOperation::ConversationDelete,
                    SqliteWorkloadCategory::MessagePersistence,
                    SqliteAccessKind::Write,
                )
                .parent_observer(),
                Some((cleanup, generation, &now_str)),
            )
            .await?;
            if !deleted {
                tx.rollback().await?;
                return Err(DbError::Serialization(
                    "creation cleanup claim was lost".to_string(),
                ));
            }
            if let Some(work_scope_id) = work_scope_id.as_deref() {
                Self::delete_work_scope_if_empty(&mut tx, work_scope_id).await?;
            }
        } else if cleanup.status == "failed" {
            sqlx::query(
                "UPDATE conversation_creation_jobs
                 SET cleanup_worker_id = NULL, cleanup_token = NULL,
                     cleanup_lease_until = NULL
                 WHERE id = ?1 AND status = 'failed' AND generation = ?2
                   AND cleanup_worker_id = ?3 AND cleanup_token = ?4
                   AND cleanup_lease_until > ?5",
            )
            .bind(&cleanup.job_id)
            .bind(generation)
            .bind(&cleanup.worker_id)
            .bind(&cleanup.token)
            .bind(&now_str)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Reserve an external creation resource under the current claim.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when authority was lost or the insert fails.
    pub async fn reserve_conversation_creation_resource(
        &self,
        reservation_id: &str,
        job_id: &str,
        claim: &CreationClaim,
        repository_identity: &str,
        resource_identity: &str,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let mut tx = self.pool.begin().await?;
        let authoritative: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM conversation_creation_jobs
             WHERE id = ?1 AND status = 'claimed' AND generation = ?2
               AND claim_worker_id = ?3 AND claim_token = ?4 AND lease_until > ?5",
        )
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .bind(now.to_rfc3339())
        .fetch_optional(&mut *tx)
        .await?;
        if authoritative.is_none() {
            tx.rollback().await?;
            return Ok(CreationCasOutcome::ClaimLost);
        }
        sqlx::query(
            "INSERT INTO conversation_creation_resource_reservations (
                id, job_id, generation, repository_identity, resource_identity,
                status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'reserved', ?6, ?6)
             ON CONFLICT(job_id, resource_identity) DO UPDATE SET
                generation = excluded.generation,
                repository_identity = excluded.repository_identity,
                status = CASE
                    WHEN conversation_creation_resource_reservations.status = 'present'
                        THEN 'present'
                    ELSE 'reserved'
                END,
                updated_at = excluded.updated_at",
        )
        .bind(reservation_id)
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(repository_identity)
        .bind(resource_identity)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(CreationCasOutcome::Applied)
    }

    /// Mark a reservation present while its generation remains current.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the guarded update fails.
    pub async fn mark_creation_resource_present(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        resource_identity: &str,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let result = sqlx::query(
            "UPDATE conversation_creation_resource_reservations
             SET status = 'present', updated_at = ?1
             WHERE job_id = ?2 AND generation = ?3 AND resource_identity = ?4
               AND EXISTS (
                   SELECT 1 FROM conversation_creation_jobs j
                   WHERE j.id = ?2 AND j.status = 'claimed' AND j.generation = ?3
                     AND j.claim_worker_id = ?5 AND j.claim_token = ?6
                     AND j.lease_until > ?1
               )",
        )
        .bind(now.to_rfc3339())
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(resource_identity)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&self.pool)
        .await?;
        Ok(if result.rows_affected() == 1 {
            CreationCasOutcome::Applied
        } else {
            CreationCasOutcome::ClaimLost
        })
    }

    /// Load durable resource reservations for one creation job.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the read fails or a generation is invalid.
    pub async fn get_creation_resource_reservations(
        &self,
        job_id: &str,
    ) -> DbResult<Vec<CreationResourceReservation>> {
        let rows: Vec<(String, String, i64, String, String, String)> = sqlx::query_as(
            "SELECT id, job_id, generation, repository_identity, resource_identity, status
             FROM conversation_creation_resource_reservations
             WHERE job_id = ?1 ORDER BY id",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(id, job_id, generation, repository_identity, resource_identity, status)| {
                    Ok(CreationResourceReservation {
                        id,
                        job_id,
                        generation: u64::try_from(generation).map_err(|_| {
                            DbError::Serialization("negative reservation generation".to_string())
                        })?,
                        repository_identity,
                        resource_identity,
                        status,
                    })
                },
            )
            .collect()
    }

    /// Renew a creation lease only while the supplied claim remains current.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails.
    pub async fn renew_conversation_creation_claim(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        now: DateTime<Utc>,
        lease_duration: chrono::Duration,
    ) -> DbResult<CreationCasOutcome> {
        let now_str = now.to_rfc3339();
        let lease_until = (now + lease_duration).to_rfc3339();
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET lease_until = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'claimed' AND generation = ?4
               AND claim_worker_id = ?5 AND claim_token = ?6 AND lease_until > ?2",
        )
        .bind(lease_until)
        .bind(now_str)
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&self.pool)
        .await?;
        Ok(if result.rows_affected() == 1 {
            CreationCasOutcome::Applied
        } else {
            CreationCasOutcome::ClaimLost
        })
    }

    /// Schedule the next bounded retry while the supplied claim is current.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails.
    pub async fn schedule_conversation_creation_retry(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        error: &str,
        now: DateTime<Utc>,
        next_attempt_at: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let now = now.to_rfc3339();
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'retry_scheduled', error = ?1, next_attempt_at = ?2,
                 updated_at = ?3, claim_worker_id = NULL, claim_token = NULL,
                 lease_until = NULL
             WHERE id = ?4 AND status = 'claimed' AND attempt < 4
               AND generation = ?5 AND claim_worker_id = ?6 AND claim_token = ?7
               AND lease_until > ?3",
        )
        .bind(error)
        .bind(next_attempt_at.to_rfc3339())
        .bind(now)
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&self.pool)
        .await?;
        Ok(if result.rows_affected() == 1 {
            CreationCasOutcome::Applied
        } else {
            CreationCasOutcome::ClaimLost
        })
    }

    /// Advance a stage only while the supplied claim remains authoritative.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails.
    pub async fn advance_conversation_creation_stage(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        expected: CreationStage,
        next: CreationStage,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET stage = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'claimed' AND stage = ?4
               AND generation = ?5 AND claim_worker_id = ?6 AND claim_token = ?7
               AND lease_until > ?2",
        )
        .bind(creation_stage_db_str(next))
        .bind(now.to_rfc3339())
        .bind(job_id)
        .bind(creation_stage_db_str(expected))
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&self.pool)
        .await?;
        Ok(if result.rows_affected() == 1 {
            CreationCasOutcome::Applied
        } else {
            CreationCasOutcome::ClaimLost
        })
    }

    /// Mark a creation job failed only while the supplied claim is current.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails.
    pub async fn fail_conversation_creation_job(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        error: &str,
        error_kind: &ErrorKind,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let failed_state = serde_json::to_string(&ConvState::CreationFailed {
            job_id: job_id.to_string(),
            error: error.to_string(),
            error_kind: error_kind.clone(),
        })
        .map_err(|serialization_error| DbError::Serialization(serialization_error.to_string()))?;
        let now = now.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'failed', error = ?1, updated_at = ?2, failed_at = ?2,
                 claim_worker_id = NULL, claim_token = NULL, lease_until = NULL
             WHERE id = ?3 AND status = 'claimed' AND generation = ?4
               AND claim_worker_id = ?5 AND claim_token = ?6 AND lease_until > ?2",
        )
        .bind(error)
        .bind(&now)
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 1 {
            sqlx::query(
                "UPDATE conversation_creation_resource_reservations
                 SET status = 'cleanup_required', generation = ?1, updated_at = ?2
                 WHERE job_id = ?3 AND status IN ('reserved', 'present')",
            )
            .bind(claim_generation_i64(claim)?)
            .bind(&now)
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            let conversation_updated = sqlx::query(
                "UPDATE conversations
                 SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
                 WHERE id = (
                     SELECT conversation_id FROM conversation_creation_jobs WHERE id = ?4
                 )",
            )
            .bind(failed_state)
            .bind(conv_state_kind(&ConvState::CreationFailed {
                job_id: job_id.to_string(),
                error: error.to_string(),
                error_kind: error_kind.clone(),
            }))
            .bind(&now)
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
            if conversation_updated.rows_affected() != 1 {
                tx.rollback().await?;
                return Err(DbError::ConversationNotFound(job_id.to_string()));
            }
            let cleanup_required: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM conversation_creation_resource_reservations
                 WHERE job_id = ?1 AND status = 'cleanup_required'",
            )
            .bind(job_id)
            .fetch_one(&mut *tx)
            .await?;
            if cleanup_required > 0 {
                let stale_scope_id: Option<String> = sqlx::query_scalar(
                    "SELECT work_scope_id FROM conversations WHERE id = (
                         SELECT conversation_id FROM conversation_creation_jobs WHERE id = ?1
                     )",
                )
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
                sqlx::query(
                    "UPDATE conversations
                     SET work_scope_id = NULL, cm_kind = 'direct', cm_task_id = NULL,
                         cm_task_title = NULL, cm_next_taskmd_id_hint = NULL
                     WHERE id = (SELECT conversation_id FROM conversation_creation_jobs WHERE id = ?1)",
                )
                .bind(job_id)
                .execute(&mut *tx)
                .await?;
                if let Some(scope_id) = stale_scope_id {
                    sqlx::query("DELETE FROM work_scopes WHERE id = ?1")
                        .bind(scope_id)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            tx.commit().await?;
            Ok(CreationCasOutcome::Applied)
        } else {
            tx.rollback().await?;
            Ok(CreationCasOutcome::ClaimLost)
        }
    }

    /// Mark a creation job ready only while the supplied claim is current.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update transaction fails.
    pub async fn complete_conversation_creation_job(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        now: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let cleared_intent = serde_json::json!({
            "cwd": "",
            "model": null,
            "text": "",
            "expansion_preflighted": false,
            "llm_text": null,
            "skill_invocation": null,
            "images": [],
            "mode": null,
            "base_branch": null,
            "checkout_ref": null,
            "seed_parent_id": null,
            "seed_label": null
        })
        .to_string();
        let now = now.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'ready', intent_json = ?1, updated_at = ?2, completed_at = ?2,
                 claim_worker_id = NULL, claim_token = NULL, lease_until = NULL
             WHERE id = ?3 AND status = 'claimed' AND generation = ?4
               AND claim_worker_id = ?5 AND claim_token = ?6 AND lease_until > ?2",
        )
        .bind(cleared_intent)
        .bind(&now)
        .bind(job_id)
        .bind(claim_generation_i64(claim)?)
        .bind(&claim.worker_id.0)
        .bind(&claim.token.0)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(CreationCasOutcome::ClaimLost);
        }
        sqlx::query("DELETE FROM conversation_creation_job_files WHERE job_id = ?1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM conversation_creation_job_images WHERE job_id = ?1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(CreationCasOutcome::Applied)
    }

    /// Atomically persist the initial message, complete its claimed creation job,
    /// and commit the dispatchable runtime state.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the transaction cannot be committed.
    #[allow(clippy::too_many_arguments)]
    pub async fn materialize_conversation_creation_runtime(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        conversation_id: &str,
        message_id: &str,
        allocate_sequence: impl FnOnce(i64) -> i64,
        content: &MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<CreationRuntimeMaterialization> {
        let cleared_intent = cleared_creation_intent_json();
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let state_updated_at = state_updated_at.to_rfc3339();
        let state_json = serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let content_json = serde_json::to_string(&content.to_stored_json())
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let display_json = display_data
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let usage_json = usage_data
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let mut tx = self.pool.begin().await?;
        let job_update = update_claimed_creation_job_ready(
            &mut tx,
            job_id,
            conversation_id,
            claim,
            &cleared_intent,
            &now_text,
        )
        .await?;
        if job_update == 0 {
            tx.rollback().await?;
            return Ok(CreationRuntimeMaterialization::ClaimLost);
        }
        let persisted_sequence_max: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0)
             FROM messages WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_one(&mut *tx)
        .await?;
        let sequence_id = allocate_sequence(persisted_sequence_max);
        if sequence_id <= persisted_sequence_max {
            return Err(DbError::Sqlx(sqlx::Error::Protocol(format!(
                "creation sequence allocator returned {sequence_id} at or below persisted maximum {persisted_sequence_max}"
            ))));
        }
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(sequence_id)
        .bind(content.message_type().to_string())
        .bind(&content_json)
        .bind(&display_json)
        .bind(&usage_json)
        .bind(&now_text)
        .execute(&mut *tx)
        .await?;
        insert_message_attachments(&mut tx, message_id, content).await?;
        update_creation_runtime_state(
            &mut tx,
            conversation_id,
            state,
            &state_json,
            &state_updated_at,
            &now_text,
        )
        .await?;
        clear_creation_job_attachments(&mut tx, job_id).await?;
        tx.commit().await?;
        let message = Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: content.message_type(),
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: now,
        };
        if let Err(error) =
            retrieval::fts_upsert(&self.pool, &message, self.sqlite_workload_collector.clone())
                .await
        {
            tracing::warn!(message_id, %error, "failed to index creation message; startup reconcile will repair");
        }
        Ok(CreationRuntimeMaterialization::Materialized(Box::new(
            message,
        )))
    }

    /// Atomically complete a claimed creation job and commit its runtime state.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if either write cannot be committed.
    pub async fn settle_conversation_creation_runtime(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        conversation_id: &str,
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<CreationCasOutcome> {
        let cleared_intent = cleared_creation_intent_json();
        let now = Utc::now().to_rfc3339();
        let state_updated_at = state_updated_at.to_rfc3339();
        let state_json = serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let mut tx = self.pool.begin().await?;
        if update_claimed_creation_job_ready(
            &mut tx,
            job_id,
            conversation_id,
            claim,
            &cleared_intent,
            &now,
        )
        .await?
            == 0
        {
            tx.rollback().await?;
            return Ok(CreationCasOutcome::ClaimLost);
        }
        update_creation_runtime_state(
            &mut tx,
            conversation_id,
            state,
            &state_json,
            &state_updated_at,
            &now,
        )
        .await?;
        clear_creation_job_attachments(&mut tx, job_id).await?;
        tx.commit().await?;
        Ok(CreationCasOutcome::Applied)
    }

    async fn persist_continuation_start(
        &self,
        conversation_id: &str,
        operation_id: &str,
        message: &Message,
        target_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<ContinuationCommitOutcome> {
        let mut tx = self.pool.begin().await?;
        let outcome = persist_continuation_start_tx(
            &mut tx,
            conversation_id,
            operation_id,
            message,
            target_state,
            state_updated_at,
        )
        .await?;
        match outcome {
            ContinuationCommitOutcome::Applied => tx.commit().await?,
            ContinuationCommitOutcome::Duplicate | ContinuationCommitOutcome::Stale => {
                tx.rollback().await?;
            }
        }
        Ok(outcome)
    }

    /// Atomically persist the threshold-crossing response and continuation
    /// operation before summary generation begins.
    ///
    /// # Errors
    ///
    /// Returns an error when the conversation or message cannot be read or
    /// atomically written.
    pub async fn begin_continuation(
        &self,
        conversation_id: &str,
        operation_id: &str,
        message: &Message,
        awaiting_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<ContinuationCommitOutcome> {
        self.persist_continuation_start(
            conversation_id,
            operation_id,
            message,
            awaiting_state,
            state_updated_at,
        )
        .await
    }

    /// Atomically retain the threshold response with a recoverable start failure.
    ///
    /// # Errors
    ///
    /// Returns an error when the conversation or message cannot be read or
    /// atomically written.
    pub async fn recover_continuation_start(
        &self,
        conversation_id: &str,
        operation_id: &str,
        message: &Message,
        failure_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<ContinuationCommitOutcome> {
        self.persist_continuation_start(
            conversation_id,
            operation_id,
            message,
            failure_state,
            state_updated_at,
        )
        .await
    }

    /// Return every conversation with continuation work that must be
    /// materialized at startup, including coordinator-owned conversations.
    ///
    /// # Errors
    ///
    /// Returns an error when candidate rows cannot be queried or decoded.
    pub async fn list_pending_continuation_conversation_ids(&self) -> DbResult<Vec<String>> {
        let rows = sqlx::query(
            "SELECT id, state FROM conversations
             WHERE archived = 0
               AND state_kind IN (
                   'awaiting_continuation',
                   'recoverable_continuation_failure',
                   'awaiting_recovery'
               )",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut ids = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let state_json: String = row.try_get("state")?;
            let state: ConvState = serde_json::from_str(&state_json)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            if matches!(
                state,
                ConvState::AwaitingContinuation { .. }
                    | ConvState::RecoverableContinuationFailure { .. }
                    | ConvState::AwaitingRecovery {
                    resume:
                        phoenix_core::domain::sm_state::RecoveryResumeTarget::ContinuationSummary { .. },
                    ..
                }
            ) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Atomically commit a generated continuation summary when the persisted
    /// continuation operation still matches `operation_id`.
    ///
    /// # Errors
    ///
    /// Returns an error when the conversation is missing, persisted state cannot
    /// be decoded, or the transactional message/state write fails.
    pub async fn commit_continuation(
        &self,
        conversation_id: &str,
        operation_id: &str,
        message: &Message,
        completed_state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<ContinuationCommitOutcome> {
        let mut tx = self.pool.begin().await?;
        let outcome = commit_continuation_tx(
            &mut tx,
            conversation_id,
            operation_id,
            message,
            completed_state,
            state_updated_at,
        )
        .await?;
        match outcome {
            ContinuationCommitOutcome::Applied => tx.commit().await?,
            ContinuationCommitOutcome::Duplicate | ContinuationCommitOutcome::Stale => {
                tx.rollback().await?;
            }
        }
        Ok(outcome)
    }

    /// Update conversation state, stamping `state_updated_at = now()`.
    /// Callers that own the authoritative entry timestamp (the runtime
    /// executor) should use [`Self::update_conversation_state_at`] so the
    /// persisted stamp matches the one carried on the `StateChange` SSE.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_conversation_state(&self, id: &str, state: &ConvState) -> DbResult<()> {
        self.update_conversation_state_at(id, state, Utc::now())
            .await
    }

    /// Update conversation state with an explicit `state_updated_at`. The
    /// runtime threads its in-memory entry timestamp here so the DB row and
    /// the `StateChange` wire event share one value (REQ-WPV-001) — no
    /// clock-drift between the two `now()` reads. `updated_at` stays `now()`
    /// (NOT the phase-entry time): it is a monotonic last-modified marker, and
    /// effects can persist other rows before `PersistState` runs, so binding
    /// it to the (earlier) phase-entry stamp would let `updated_at` regress.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    pub async fn update_conversation_state_at(
        &self,
        id: &str,
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<()> {
        let state_json = serde_json::to_string(state).unwrap();

        let result = sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4 WHERE id = ?5",
        )
        .bind(&state_json)
        .bind(conv_state_kind(state))
        .bind(state_updated_at.to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Replace a conversation's pending steering queue with `queue` (FIFO),
    /// persisted across the normalized `steering_messages` + grandchild
    /// attachment tables. Replace-all semantics: the existing rows are deleted
    /// (cascading their attachments) and re-inserted in order inside one
    /// transaction, so a reader never observes a torn queue.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_steering_queue(
        &self,
        id: &str,
        queue: &[phoenix_core::domain::sm_event::SteerEntry],
    ) -> DbResult<()> {
        let now = Utc::now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_product_conversation_admission_tx(&mut tx, id).await?;
        if queue.len() > MAX_STEERING_QUEUE_DEPTH {
            tx.rollback().await?;
            return Err(DbError::SteeringQueueFull);
        }

        let exists = sqlx::query("SELECT 1 FROM conversations WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }

        sqlx::query("DELETE FROM steering_messages WHERE conversation_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (ordinal, entry) in queue.iter().enumerate() {
            sqlx::query(
                "INSERT OR IGNORE INTO steering_acceptance_receipts
                    (conversation_id, message_id, request_fingerprint)
                 VALUES (?1, ?2, NULL)",
            )
            .bind(id)
            .bind(&entry.message_id)
            .execute(&mut *tx)
            .await?;
            insert_steering_entry_tx(
                &mut tx,
                id,
                i64::try_from(ordinal).unwrap_or(i64::MAX),
                entry,
            )
            .await?;
        }

        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Return the current steering queue depth.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the queue cannot be read or its count cannot be
    /// represented as `usize`.
    pub async fn steering_queue_depth(&self, id: &str) -> DbResult<usize> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages WHERE conversation_id = ?1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        usize::try_from(count)
            .map_err(|_| DbError::Serialization("steering queue depth overflow".to_string()))
    }

    /// Append one steering entry atomically and return its committed zero-based
    /// queue position. Existing rows are never rewritten, so a concurrent drain
    /// cannot be resurrected by a stale read-modify-write snapshot.
    ///
    /// # Errors
    ///
    /// Returns a database or serialization error if the transaction cannot be
    /// committed or its queue position cannot fit in `usize`.
    pub async fn append_steering_entry(
        &self,
        id: &str,
        entry: &phoenix_core::domain::sm_event::SteerEntry,
        request_fingerprint: &str,
    ) -> DbResult<usize> {
        let now = Utc::now();
        #[cfg(test)]
        if let Some(latch) = &self.steering_begin_test_latch {
            latch.before_begin.notify_waiters();
            latch.allow_begin.notified().await;
            latch.begin_called.notify_waiters();
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_product_conversation_admission_tx(&mut tx, id).await?;

        let (queue_position, ordinal): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(MAX(ordinal), -1) + 1
             FROM steering_messages
             WHERE conversation_id = ?1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;

        let queue_depth = usize::try_from(queue_position)
            .map_err(|_| DbError::Serialization("steering queue depth overflow".to_string()))?;
        if queue_depth >= MAX_STEERING_QUEUE_DEPTH {
            tx.rollback().await?;
            return Err(DbError::SteeringQueueFull);
        }

        sqlx::query(
            "INSERT INTO steering_acceptance_receipts
                (conversation_id, message_id, request_fingerprint)
             VALUES (?1, ?2, ?3)",
        )
        .bind(id)
        .bind(&entry.message_id)
        .bind(request_fingerprint)
        .execute(&mut *tx)
        .await?;
        insert_steering_entry_tx(&mut tx, id, ordinal, entry).await?;
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        usize::try_from(queue_position)
            .map_err(|_| DbError::Serialization("steering queue position overflow".to_string()))
    }

    /// Load the immutable request fingerprint recorded when a steering
    /// identity was accepted. A legacy receipt has no reconstructable
    /// fingerprint and is represented explicitly.
    ///
    /// # Errors
    ///
    /// Returns a database error if the lookup fails.
    pub async fn get_steering_acceptance_fingerprint(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> DbResult<Option<SteeringAcceptanceFingerprint>> {
        let fingerprint: Option<Option<String>> = sqlx::query_scalar(
            "SELECT request_fingerprint
             FROM steering_acceptance_receipts
             WHERE conversation_id = ?1 AND message_id = ?2",
        )
        .bind(conversation_id)
        .bind(message_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(fingerprint.map(|fingerprint| match fingerprint {
            Some(exact) => SteeringAcceptanceFingerprint::Exact(exact),
            None => SteeringAcceptanceFingerprint::LegacyUnknown,
        }))
    }

    /// Report whether the newest transcript row is a committed steering input
    /// message whose exact queue row has already been consumed.
    ///
    /// # Errors
    ///
    /// Returns a database error if the lookup fails.
    pub async fn has_committed_steering_turn(&self, conversation_id: &str) -> DbResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1
                 FROM messages m
                 JOIN steering_acceptance_receipts r
                   ON r.conversation_id = m.conversation_id
                  AND r.message_id = m.message_id
                 WHERE m.conversation_id = ?1
                   AND m.message_type IN ('user', 'skill')
                   AND m.sequence_id = (
                       SELECT MAX(latest.sequence_id)
                       FROM messages latest
                       WHERE latest.conversation_id = m.conversation_id
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM steering_messages queued
                       WHERE queued.conversation_id = m.conversation_id
                         AND queued.message_id = m.message_id
                   )
             )",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Remove one steering entry and report whether this call removed a row.
    /// The boolean is the publication fence for cancellation SSE: an
    /// idempotent retry succeeds but must not announce a second mutation.
    ///
    /// # Errors
    ///
    /// Returns a database error if the delete transaction cannot be committed.
    pub async fn remove_steering_entry(&self, id: &str, message_id: &str) -> DbResult<bool> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let removed = sqlx::query(
            "DELETE FROM steering_messages WHERE conversation_id = ?1 AND message_id = ?2",
        )
        .bind(id)
        .bind(message_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        if removed {
            sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    /// Remove the steering entries with the given `message_ids` from a
    /// conversation. A plain `DELETE` on `steering_messages` (cascading the
    /// grandchild attachment rows) — no read-modify-write window, so a
    /// concurrent enqueue cannot be clobbered.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn remove_steering_entries(&self, id: &str, message_ids: &[String]) -> DbResult<()> {
        if message_ids.is_empty() {
            return Ok(());
        }
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        for message_id in message_ids {
            sqlx::query(
                "DELETE FROM steering_messages WHERE conversation_id = ?1 AND message_id = ?2",
            )
            .bind(id)
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically materialize one reducer-selected steering batch, persist its
    /// supplied next state, and remove exactly that batch from the queue.
    /// Matching pre-existing message identities are accepted as a bounded
    /// legacy-partial recovery case; a missing queue row or cross-conversation
    /// identity conflict rolls back the full transaction.
    ///
    /// # Errors
    ///
    /// Returns a database or serialization error when any message, state, or
    /// exact queue deletion cannot be committed as one transaction.
    pub async fn commit_steering_drain(
        &self,
        id: &str,
        messages: &[Message],
        state: &ConvState,
        state_updated_at: DateTime<Utc>,
    ) -> DbResult<Vec<SteeringDrainMessageStatus>> {
        if messages.is_empty() {
            return Err(DbError::Serialization(
                "steering drain conflict: atomic batch cannot be empty".to_string(),
            ));
        }

        let state_json = serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let mut statuses = Vec::with_capacity(messages.len());

        for message in messages {
            if message.conversation_id != id {
                return Err(DbError::Serialization(format!(
                    "steering drain conflict: message {} targets conversation {} instead of {id}",
                    message.message_id, message.conversation_id
                )));
            }
            match steering_message_matches_tx(&mut tx, message).await? {
                Some(true) => {
                    statuses.push(SteeringDrainMessageStatus::LegacyAlreadyMaterialized);
                }
                Some(false) => {
                    return Err(DbError::Serialization(format!(
                        "steering drain conflict: message {} already exists with different data",
                        message.message_id
                    )));
                }
                None => {
                    insert_message_tx(&mut tx, message).await?;
                    if steering_message_matches_tx(&mut tx, message).await? != Some(true) {
                        return Err(DbError::Serialization(format!(
                            "steering drain conflict: message {} was not inserted exactly",
                            message.message_id
                        )));
                    }
                    statuses.push(SteeringDrainMessageStatus::Inserted);
                }
            }
        }

        let updated = sqlx::query(
            "UPDATE conversations
             SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4
             WHERE id = ?5",
        )
        .bind(&state_json)
        .bind(conv_state_kind(state))
        .bind(state_updated_at.to_rfc3339())
        .bind(&now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }

        for message in messages {
            let removed = sqlx::query(
                "DELETE FROM steering_messages WHERE conversation_id = ?1 AND message_id = ?2",
            )
            .bind(id)
            .bind(&message.message_id)
            .execute(&mut *tx)
            .await?;
            if removed.rows_affected() != 1 {
                return Err(DbError::Serialization(format!(
                    "steering drain conflict: queue entry {} was no longer pending",
                    message.message_id
                )));
            }
        }

        tx.commit().await?;
        Ok(statuses)
    }

    /// Return the owning conversation for a globally unique steering message id.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn steering_conversation_id_for_message(
        &self,
        message_id: &str,
    ) -> DbResult<Option<String>> {
        sqlx::query_scalar("SELECT conversation_id FROM steering_messages WHERE message_id = ?1")
            .bind(message_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Into::into)
    }

    /// Report whether a conversation has any durable pending steering work.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn has_steering_entries(&self, conversation_id: &str) -> DbResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM steering_messages WHERE conversation_id = ?1)",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Load a conversation's pending steering queue (FIFO) from the normalized
    /// tables, rehydrating each entry's attachments and skill invocation.
    ///
    /// All reads run in one transaction so the parent and child rows come from a
    /// single consistent snapshot — a concurrent `update_steering_queue` /
    /// `remove_steering_entries` commit cannot produce a torn queue.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_steering_queue(
        &self,
        id: &str,
    ) -> DbResult<Vec<phoenix_core::domain::sm_event::SteerEntry>> {
        use phoenix_core::domain::db_schema::{FileAttachment, ImageData};
        use phoenix_core::domain::skill_invocation::SkillInvocation;
        use phoenix_core::domain::sm_event::SteerEntry;

        let mut tx = self.pool.begin().await?;

        let rows = sqlx::query(
            "SELECT message_id, text, llm_text, user_agent, skill_name, skill_body, skill_dir
             FROM steering_messages WHERE conversation_id = ?1 ORDER BY ordinal ASC",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;

        let mut queue = Vec::with_capacity(rows.len());
        for row in rows {
            let message_id: String = row.try_get("message_id")?;
            let files = sqlx::query(
                "SELECT original_name, media_type, size_bytes, stored_path
                 FROM steering_message_files WHERE message_id = ?1 ORDER BY file_ordinal",
            )
            .bind(&message_id)
            .map(|r: SqliteRow| FileAttachment {
                original_name: r.get("original_name"),
                media_type: r.get("media_type"),
                size_bytes: u64::try_from(r.get::<i64, _>("size_bytes")).unwrap_or(0),
                stored_path: r.get("stored_path"),
            })
            .fetch_all(&mut *tx)
            .await?;
            let images = sqlx::query(
                "SELECT media_type, data FROM steering_message_images
                 WHERE message_id = ?1 ORDER BY image_ordinal",
            )
            .bind(&message_id)
            .map(|r: SqliteRow| ImageData {
                data: r.get("data"),
                media_type: r.get("media_type"),
            })
            .fetch_all(&mut *tx)
            .await?;
            // The CHECK constraint guarantees the skill_* trio is all-or-nothing.
            let skill_invocation =
                row.try_get::<Option<String>, _>("skill_name")?
                    .map(|name| SkillInvocation {
                        name,
                        body: row
                            .try_get::<Option<String>, _>("skill_body")
                            .ok()
                            .flatten()
                            .unwrap_or_default(),
                        skill_dir: row
                            .try_get::<Option<String>, _>("skill_dir")
                            .ok()
                            .flatten()
                            .unwrap_or_default(),
                    });
            queue.push(SteerEntry {
                text: row.try_get("text")?,
                llm_text: row.try_get("llm_text")?,
                images,
                files,
                message_id,
                user_agent: row.try_get("user_agent")?,
                skill_invocation,
            });
        }
        tx.commit().await?;
        Ok(queue)
    }

    /// Update conversation mode (e.g., Explore -> Work on task approval)
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    pub async fn update_conversation_mode(&self, id: &str, mode: &ConvMode) -> DbResult<()> {
        self.update_conversation_mode_inner(id, mode, None).await
    }

    /// Atomically update a conversation's mode, cwd, and normalized environment.
    ///
    /// # Errors
    /// Returns a [`DbError`] if the conversation is missing or persistence fails.
    pub async fn update_conversation_mode_and_cwd(
        &self,
        id: &str,
        mode: &ConvMode,
        cwd: &str,
    ) -> DbResult<()> {
        self.update_conversation_mode_inner(id, mode, Some(cwd))
            .await
    }

    async fn update_conversation_mode_inner(
        &self,
        id: &str,
        mode: &ConvMode,
        new_cwd: Option<&str>,
    ) -> DbResult<()> {
        let now = Utc::now().to_rfc3339();
        let cm = conv_mode_columns(mode);
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT c.work_scope_id AS work_scope_id, e.cwd
             FROM conversations c
             JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.id = ?1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| DbError::ConversationNotFound(id.to_string()))?;
        let persisted_cwd: String = row.get("cwd");
        let cwd = new_cwd.unwrap_or(&persisted_cwd);
        let scope_id = WorkScopeId::parse(row.get::<String, _>("work_scope_id"))
            .map_err(|error| DbError::Serialization(error.to_string()))?;

        let result = sqlx::query(
            "UPDATE conversations
             SET cm_kind = ?1, cm_task_id = ?2, cm_task_title = ?3,
                 cm_next_taskmd_id_hint = ?4, updated_at = ?5
             WHERE id = ?6 AND work_scope_id = ?7",
        )
        .bind(cm.kind)
        .bind(cm.task_id)
        .bind(cm.task_title)
        .bind(cm.next_taskmd_id_hint)
        .bind(&now)
        .bind(id)
        .bind(scope_id.as_str())
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        let authority = match mode {
            ConvMode::Explore { .. } => AuthorityKind::RestrictedExplore,
            ConvMode::Direct | ConvMode::Work { .. } | ConvMode::Branch { .. } => {
                AuthorityKind::Work
            }
        };
        sqlx::query("UPDATE work_scopes SET authority_kind = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(authority.as_str())
            .bind(&now)
            .bind(scope_id.as_str())
            .execute(&mut *tx)
            .await?;
        Self::update_work_scope_environment_tx(
            &mut tx,
            &scope_id,
            Self::environment_for_mode(cwd, &cm),
            &now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Check if any non-archived conversation for a project is in Work mode
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // May be used for future project-level queries
    pub async fn has_active_work_conversation(&self, project_id: &str) -> DbResult<bool> {
        let row = sqlx::query(
            "SELECT COUNT(*) FROM conversations
             WHERE project_id = ?1 AND archived = 0
             AND cm_kind = 'work'",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        let count: i64 = row.get(0);
        Ok(count > 0)
    }

    /// List non-archived conversations whose `conv_mode.worktree_path` equals
    /// `worktree_path`, regardless of `user_initiated`.
    ///
    /// Used by the resource-cleanup cascade to decide whether a `WorkScope`
    /// is still owned after a conversation is deleted: a worktree-scoped
    /// resource (bash/tmux/browser/terminal handles, the shared git worktree
    /// and task branch) may only be torn down once *no* non-terminal,
    /// non-archived conversation still resolves to that scope. The query
    /// deliberately omits the `user_initiated = 1` filter that
    /// [`Self::list_conversations`] applies, because the surviving sibling
    /// owner is frequently a non-user-initiated Work sub-agent (or its
    /// parent). Terminality is a `ConvState`-domain concept, so it is filtered
    /// by the caller after parsing rather than in SQL.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_conversations_for_worktree(
        &self,
        worktree_path: &str,
    ) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.archived = 0
               AND e.environment_kind = 'allocated_worktree'
               AND e.worktree_path = ?1",
        )
        .bind(worktree_path)
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// List conversations that share one persisted work-scope identity.
    ///
    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_conversations_for_work_scope(
        &self,
        work_scope_id: &phoenix_core::work_scope::WorkScopeId,
    ) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.work_scope_id = ?1",
        )
        .bind(work_scope_id.as_str())
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Update the owning `WorkScope`'s environment cwd during recovery.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_conversation_cwd_recovery_only(&self, id: &str, cwd: &str) -> DbResult<()> {
        // Expected invariant: callers (repo_root, worktree_path — both
        // git-derived) pass a non-empty absolute path. This is the only
        // mutation path and runs during recovery/teardown, so a panic here
        // would be worse than tolerating an unexpected value — log loudly
        // and still perform the write rather than crashing recovery.
        if cwd.is_empty() || !std::path::Path::new(cwd).is_absolute() {
            tracing::error!(
                conv_id = %id,
                cwd,
                "update_conversation_cwd_recovery_only called with a non-absolute or empty cwd; \
                 proceeding but this violates the cwd contract (task 13012)"
            );
        }
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE work_scopes
             SET cwd = ?1, updated_at = ?2
             WHERE id = (SELECT work_scope_id FROM conversations WHERE id = ?3)",
        )
        .bind(cwd)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Read the conversation's clear watermark (stale tool-result clearing,
    /// specs/stale-tool-results). Returns 0 for a conversation with nothing
    /// cleared yet, or one that does not exist.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_clear_watermark(&self, id: &str) -> DbResult<i64> {
        let watermark: Option<i64> =
            sqlx::query_scalar("SELECT clear_watermark FROM conversations WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(watermark.unwrap_or(0))
    }

    /// Advance the conversation's clear watermark. The write is structurally
    /// monotonic: `MAX(clear_watermark, ?1)` can never move the persisted value
    /// backward, so a caller that passes a stale-low value (e.g. after a failed
    /// watermark read) cannot regress it and re-expose already-cleared results
    /// (specs/stale-tool-results, REQ-STR-007). The column is `NOT NULL DEFAULT
    /// 0`, so `MAX` never sees a NULL operand.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the conversation does not exist or the
    /// underlying database operation fails.
    pub async fn update_clear_watermark(&self, id: &str, watermark: i64) -> DbResult<()> {
        let result = sqlx::query(
            "UPDATE conversations SET clear_watermark = MAX(clear_watermark, ?1) WHERE id = ?2",
        )
        .bind(watermark)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Return the normalized hidden repository attached to the conversation's current `WorkScope`.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the attachment query or identity decoding fails.
    pub async fn current_work_scope_hidden_repository_attachment(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<AttachedHiddenGitRepository>> {
        let row = sqlx::query(
            "SELECT c.work_scope_id, wr.repository_id
               FROM conversations c
               JOIN work_scope_git_repositories wr ON wr.work_scope_id = c.work_scope_id
              WHERE c.id = ?1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let work_scope_id = WorkScopeId::parse(row.get::<String, _>("work_scope_id"))
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let repository_id = phoenix_core::git_repository::GitRepositoryId::parse(
            row.get::<String, _>("repository_id"),
        )
        .map_err(|error| DbError::Serialization(error.to_string()))?;
        Ok(Some(AttachedHiddenGitRepository {
            work_scope_id,
            repository_id,
        }))
    }

    /// Derive the approved-task immutable root from normalized `WorkScope` repository evidence.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when repository evidence cannot be queried or decoded.
    pub async fn root_reservation_for_attached_hidden_repository(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<ApprovedTaskRootReservationInput>> {
        let row = sqlx::query(
            "SELECT reservation.repository_id AS repository_id,
                    reservation.repo_root AS repository_root,
                    reservation.logical_base AS logical_base,
                    reservation.exact_checkout_oid AS exact_checkout_oid
               FROM conversations needle
               JOIN product_root_reservations reservation
                 ON reservation.status = 'consumed'
                AND reservation.kind = 'exact_committed_tree'
               JOIN conversations c ON c.id = reservation.consumed_by_conversation_id
              WHERE needle.id = ?1
                AND c.product_conversation_id = needle.product_conversation_id
              ORDER BY c.created_at ASC, c.id ASC
              LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        let row = if let Some(row) = row {
            row
        } else {
            let retained = sqlx::query(
                "SELECT wr.repository_id,
                        management.path AS repository_root,
                        branch.branch AS logical_base,
                        observed.last_observed_head_oid AS exact_checkout_oid
                   FROM conversations needle
                   JOIN conversations c
                     ON c.product_conversation_id = needle.product_conversation_id
                   JOIN work_scope_git_repositories wr ON wr.work_scope_id = c.work_scope_id
                   JOIN git_repository_locator_observations management
                     ON management.repository_id = wr.repository_id
                    AND management.locator_kind = 'management_root'
                    AND management.status = 'present'
                   JOIN git_repository_default_branch_observations branch
                     ON branch.repository_id = wr.repository_id
                    AND branch.status = 'resolved'
                   JOIN work_scope_observed_branches observed
                     ON observed.work_scope_id = c.work_scope_id
                    AND observed.repository_identity = wr.repository_id
                    AND observed.branch_name = branch.branch
                  WHERE needle.id = ?1
                  ORDER BY observed.last_observed_at DESC, c.created_at ASC
                  LIMIT 1",
            )
            .bind(conversation_id)
            .fetch_optional(&self.pool)
            .await?;
            let Some(retained) = retained else {
                return Ok(None);
            };
            retained
        };
        Ok(Some(ApprovedTaskRootReservationInput {
            repository_id: phoenix_core::git_repository::GitRepositoryId::parse(
                row.get::<String, _>("repository_id"),
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?,
            repository_root: row.get("repository_root"),
            exact_checkout_oid: row.get("exact_checkout_oid"),
            logical_base: row.get("logical_base"),
        }))
    }

    /// Create a fresh Work conversation and `ProductConversation` for an approved task.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// Atomically creates an approved-task successor shell and its durable provisioning job.
    ///
    /// # Panics
    ///
    /// Panics if the fixed provisioning state cannot be serialized.
    #[allow(clippy::too_many_lines)]
    pub async fn create_task_approval_handoff_creation_job(
        &self,
        parent_id: &str,
        approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
    ) -> DbResult<Conversation> {
        let _telemetry = self.sqlite_telemetry(
            SqliteOperation::CreateTaskApprovalHandoff,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
        );
        let parent = self.get_conversation(parent_id).await?;
        let snapshot = phoenix_core::task_handoff::ApprovedTaskSnapshot::from(approval);
        let approved_root_reservation = self
            .root_reservation_for_attached_hidden_repository(parent_id)
            .await?;
        let new_id = uuid::Uuid::new_v4().to_string();
        let job_id = uuid::Uuid::new_v4().to_string();
        let message_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let state = ConvState::Provisioning {
            job_id: job_id.clone(),
            phase: ConversationCreationPhase::Accepted,
        };
        let state_json = serde_json::to_string(&state).unwrap();
        let intent = ConversationCreationIntent {
            cwd: parent.cwd.clone(),
            profile: None,
            model: parent.model.clone(),
            effort: parent.effort,
            text: snapshot.seed_message(),
            expansion_preflighted: true,
            llm_text: None,
            skill_invocation: None,
            message_id: message_id.clone(),
            images: Vec::new(),
            files: Vec::new(),
            mode: Some("approved_task".to_string()),
            base_branch: Some(snapshot.base_branch.clone()),
            checkout_ref: Some(snapshot.branch_name.clone()),
            reserved_checkout_oid: approved_root_reservation
                .as_ref()
                .map(|reservation| reservation.exact_checkout_oid.clone()),
            reserved_repo_root: approved_root_reservation
                .as_ref()
                .map(|reservation| reservation.repository_root.clone()),
            reserved_root_freshness: approved_root_reservation
                .as_ref()
                .map(|_| "fresh".to_string()),
            reserved_root_failure: None,
            seed_parent_id: None,
            seed_label: Some(snapshot.title.clone()),
            approved_task: Some(snapshot.clone()),
        };
        let intent_json = serde_json::to_string(&intent)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let base_slug = {
            let slug = schema::slug_from_title(&snapshot.task_title);
            if slug.is_empty() {
                "conversation".to_string()
            } else {
                slug
            }
        };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: Option<(String, String)> = sqlx::query_as(
            "SELECT target.id, job.intent_json
             FROM product_conversation_sources source
             JOIN conversations target ON target.product_conversation_id = source.target_product_conversation_id
             JOIN conversation_creation_jobs job ON job.conversation_id = target.id
             WHERE source.source_product_conversation_id = ?1
               AND source.relation_kind = 'approved_task' AND source.relation_key = ?2
               AND target.parent_conversation_id IS NULL
             ORDER BY target.created_at ASC, target.id ASC
             LIMIT 1",
        )
        .bind(parent.product_conversation_id.as_str())
        .bind(&snapshot.task_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((existing, existing_intent_json)) = existing {
            let existing_intent: ConversationCreationIntent =
                serde_json::from_str(&existing_intent_json)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
            if existing_intent.approved_task.as_ref() != Some(&snapshot) {
                return Err(DbError::ContinuationPrecondition(format!(
                    "approved task handoff conflicts with committed reviewed snapshot {}",
                    snapshot.task_id
                )));
            }
            tx.commit().await?;
            return self.get_conversation(&existing).await;
        }
        if parent.continued_in_conv_id.is_some() {
            return Err(DbError::ContinuationPrecondition(
                "approved-task source already has a continuation".to_string(),
            ));
        }

        let product_id = phoenix_core::domain::product_conversation::ProductConversationId::new();
        sqlx::query("INSERT INTO product_conversations (id, kind, ordinary_lifecycle) VALUES (?1, 'ordinary', 'open')")
            .bind(product_id.as_str())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO product_conversation_sources (
                 target_product_conversation_id, source_product_conversation_id,
                 source_conversation_id, relation_kind, relation_key, created_at_us
             ) VALUES (?1, ?2, ?3, 'approved_task', ?4, ?5)",
        )
        .bind(product_id.as_str())
        .bind(parent.product_conversation_id.as_str())
        .bind(parent_id)
        .bind(&snapshot.task_id)
        .bind(now.timestamp_micros())
        .execute(&mut *tx)
        .await?;

        let mut slug = base_slug.clone();
        for attempt in 0..=20 {
            let title = schema::title_from_slug(&slug);
            match sqlx::query(
                "INSERT INTO conversations (
                     id, product_conversation_id, slug, title, parent_conversation_id,
                     user_initiated, state, state_kind, state_updated_at, created_at, updated_at,
                     archived, model, effort, project_id, desired_base_branch, seed_parent_id,
                     seed_label, llm_language, cm_kind, runtime_role, work_scope_id, service_tier
                 ) VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5, ?6, ?7, ?7, ?7, 0, ?8, ?9, ?10, ?11,
                           NULL, ?12, ?13, 'direct', 'user', NULL, ?14)
",
            )
            .bind(&new_id)
            .bind(product_id.as_str())
            .bind(&slug)
            .bind(title)
            .bind(&state_json)
            .bind(conv_state_kind(&state))
            .bind(&now_str)
            .bind(parent.model.as_deref())
            .bind(parent.effort.map(ModelEffort::as_wire_name))
            .bind(parent.project_id.as_deref())
            .bind(&snapshot.base_branch)
            .bind(&snapshot.title)
            .bind(parent.llm_language.as_str())
            .bind(parent.service_tier.as_wire_name())
            .execute(&mut *tx)
            .await
            {
                Ok(_) => break,
                Err(sqlx::Error::Database(error))
                    if is_sqlite_unique_constraint(error.as_ref()) && attempt < 20 =>
                {
                    slug = format!("{base_slug}-{}", attempt + 2);
                }
                Err(error) => return Err(DbError::Sqlx(error)),
            }
        }
        let source_state = ConvState::Idle;
        sqlx::query(
            "UPDATE conversations
             SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
             WHERE id = ?4",
        )
        .bind(serde_json::to_string(&source_state).unwrap())
        .bind(conv_state_kind(&source_state))
        .bind(&now_str)
        .bind(parent_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                 id, conversation_id, message_id, status, stage, attempt, generation, intent_json,
                 error, accepted_at, provisioning_started_at, completed_at, failed_at, cancelled_at,
                 deletion_requested_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 'accepted', 'validate_intent', 0, 0, ?4, NULL, ?5, NULL,
                       NULL, NULL, NULL, NULL, ?5, ?5)",
        )
        .bind(&job_id)
        .bind(&new_id)
        .bind(&message_id)
        .bind(intent_json)
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.get_conversation(&new_id).await
    }

    /// Create a continuation conversation for a context-exhausted parent, atomically.
    ///
    /// Implements REQ-BED-030 and bedrock.allium continuation rules.
    ///
    /// Within a single `SQLite` transaction:
    ///   1. INSERT a new `conversations` row with the parent's `conv_mode` cloned
    ///      verbatim (Work: `branch_name`/`worktree_path`/`base_branch`/`task_id`/`task_title`;
    ///      Branch/Explore with a worktree: `branch_name`/`worktree_path`/`base_branch`;
    ///      Direct: no worktree fields). `cwd`, `project_id`, and `model` are inherited.
    ///      State is fresh `Idle`; `continued_in_conv_id` is NULL.
    ///   2. UPDATE the parent's `continued_in_conv_id` to the new row's id.
    ///
    /// Preconditions checked before the INSERT runs:
    ///   - Parent exists (else `ConversationNotFound`).
    ///   - Parent state is `ContextExhausted`
    ///     (else `Ok(ContinueOutcome::ParentNotContextExhausted)`).
    ///   - Parent's `continued_in_conv_id` is NULL
    ///     (else `Ok(ContinueOutcome::AlreadyContinued)` — idempotent return of the
    ///     existing continuation).
    ///
    /// The transaction is rolled back via `Drop` if any step fails before `commit`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    #[allow(clippy::too_many_lines)] // single atomic flow; splitting hurts readability
    pub async fn continue_conversation(&self, parent_id: &str) -> DbResult<ContinueOutcome> {
        self.continue_conversation_inner(parent_id, None).await
    }

    /// Creates a continuation and its opening-handoff intent atomically.
    ///
    /// # Errors
    /// Returns the same conversation, validation, and transaction errors as
    /// [`Self::continue_conversation`].
    pub async fn continue_conversation_with_intent(
        &self,
        parent_id: &str,
        intent: NewContinuationDispatchIntent,
    ) -> DbResult<(ContinueOutcome, Option<ContinuationDispatchIntent>)> {
        let outcome = self
            .continue_conversation_inner(parent_id, Some(&intent))
            .await?;
        let stored = self.continuation_dispatch_intent(parent_id).await?;
        Ok((outcome, stored))
    }

    #[allow(clippy::too_many_lines)] // one transaction owns creation, transfer, and intent
    async fn continue_conversation_inner(
        &self,
        parent_id: &str,
        intent: Option<&NewContinuationDispatchIntent>,
    ) -> DbResult<ContinueOutcome> {
        // Fetch parent outside the transaction — the subsequent INSERT+UPDATE
        // guards against concurrent continuation via the parent's
        // `continued_in_conv_id` still being NULL at UPDATE time.
        let parent = self.get_conversation(parent_id).await?;

        // Idempotent shortcut: parent already has a continuation.
        if let Some(ref existing_id) = parent.continued_in_conv_id {
            tracing::info!(
                parent_id = %parent_id,
                existing_continuation = %existing_id,
                "continue_conversation: idempotent return of existing continuation",
            );
            let existing = self.get_conversation(existing_id).await?;
            return Ok(ContinueOutcome::AlreadyContinued(existing));
        }

        // Gate on context-exhausted state.
        if !matches!(parent.state, ConvState::ContextExhausted { .. }) {
            return Ok(ContinueOutcome::ParentNotContextExhausted {
                state_variant: parent.state.variant_name(),
            });
        }

        if parent.runtime_role == RuntimeRole::SubAgent {
            return Err(DbError::ContinuationPrecondition(
                "subordinate executions cannot create continuation transcripts".to_string(),
            ));
        }

        let new_id = uuid::Uuid::new_v4().to_string();

        // Sequential slug: walk to chain root, count existing members, then
        // assign `{root_slug}-{N}` where N = member_count + 1 (e.g. root-only
        // chain → first continuation is #2). Concurrent continuations are
        // handled by the UNIQUE-violation retry loop below (mirroring
        // `create_conversation`); `rows_affected() == 0` on the UPDATE remains
        // the definitive TOCTOU guard.
        let root_id = self
            .chain_root_of(parent_id)
            .await?
            .unwrap_or_else(|| parent_id.to_string());
        let root = self.get_conversation(&root_id).await?;
        let chain_len = self.chain_members_forward(&root_id).await?.len();
        let root_slug = root.slug.as_deref().unwrap_or("conversation");
        let base_n = chain_len + 1;
        let mut candidate_slug = format!("{root_slug}-{base_n}");
        let mut slug_offset: usize = 0;

        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();
        let cm = conv_mode_columns(&parent.conv_mode);

        // Atomic INSERT + UPDATE. On any error before `commit()`, the
        // transaction guard drops and SQLite rolls back.
        let mut tx = self.pool.begin().await?;

        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO product_continuation_reservations (
                 predecessor_conversation_id, successor_conversation_id,
                 product_conversation_id
             ) VALUES (?1, ?2, ?3)",
        )
        .bind(parent_id)
        .bind(&new_id)
        .bind(parent.product_conversation_id.as_str())
        .execute(&mut *tx)
        .await?;
        let reserved = if parent.runtime_role == RuntimeRole::Coordinator {
            sqlx::query(
                "UPDATE conversations
                 SET continued_in_conv_id = ?1, updated_at = ?2, coordinator_head = 0
                 WHERE id = ?3 AND continued_in_conv_id IS NULL AND coordinator_head = 1",
            )
            .bind(&new_id)
            .bind(&now_str)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?
        } else {
            sqlx::query(
                "UPDATE conversations SET continued_in_conv_id = ?1, updated_at = ?2
                 WHERE id = ?3 AND continued_in_conv_id IS NULL",
            )
            .bind(&new_id)
            .bind(&now_str)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?
        };
        if reserved.rows_affected() == 0 {
            drop(tx);
            let refetched = self.get_conversation(parent_id).await?;
            if let Some(existing_id) = refetched.continued_in_conv_id {
                return Ok(ContinueOutcome::AlreadyContinued(
                    self.get_conversation(&existing_id).await?,
                ));
            }
            return Err(DbError::ConversationNotFound(parent_id.to_string()));
        }

        let continuation_work_scope_id = if matches!(parent.conv_mode, ConvMode::Direct) {
            let (scope_id, authority_kind, environment) =
                Self::new_scope_for_conversation(&parent.cwd, &cm);
            Self::insert_work_scope_tx(&mut tx, &scope_id, authority_kind, environment, &now_str)
                .await?;
            Some(scope_id)
        } else {
            parent.attached_work_scope_id.clone()
        };

        // Retry on slug collision (UNIQUE constraint, SQLite error 2067).
        // Collisions are rare: concurrent continuations racing for the same
        // sequential number, or an unrelated conversation sharing the name.
        let actual_slug = loop {
            let title_for_insert = schema::title_from_slug(&candidate_slug);
            let result = sqlx::query(
                "INSERT INTO conversations (id, product_conversation_id, slug, title, coordinator_head, parent_conversation_id, user_initiated, state, state_kind, state_updated_at, created_at, updated_at, archived, transcript_generation, model, effort, project_id, desired_base_branch, seed_parent_id, seed_label, continued_in_conv_id, llm_language, cm_kind, cm_task_id, cm_task_title, cm_next_taskmd_id_hint, runtime_role, work_scope_id, sub_agent_cwd_override, service_tier)
                 VALUES (?1, ?23, ?2, ?3, CASE WHEN ?19 = 'coordinator' THEN 1 ELSE 0 END, NULL, ?18, ?4, ?5, ?6, ?6, ?6, 0, 1, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13, ?14, ?15, ?16, ?17, ?19, ?20, ?21, ?22)",
            )
            .bind(&new_id)
            .bind(&candidate_slug)
            .bind(&title_for_insert)
            .bind(&idle_state)
            .bind(conv_state_kind(&ConvState::Idle))
            .bind(&now_str)
            .bind(parent.model.as_deref())
            .bind(parent.effort.map(ModelEffort::as_wire_name))
            .bind(parent.project_id.as_deref())
            .bind(parent.desired_base_branch.as_deref())
            // Continuations do not inherit the parent's seed fields — those are
            // decorative UI metadata for a different concept (REQ-SEED-003/004).
            .bind::<Option<&str>>(None)
            .bind::<Option<&str>>(None)
            .bind(parent.llm_language.as_str())
            .bind(cm.kind)
            .bind(cm.task_id)
            .bind(cm.task_title)
            .bind(cm.next_taskmd_id_hint)
            .bind(parent.user_initiated)
            .bind(if parent.runtime_role == RuntimeRole::Coordinator {
                RuntimeRole::Coordinator.as_str()
            } else {
                RuntimeRole::User.as_str()
            })
            .bind(continuation_work_scope_id.as_ref().map(WorkScopeId::as_str))
            .bind::<Option<&str>>(None)
            .bind(parent.service_tier.as_wire_name())
            .bind(parent.product_conversation_id.as_str())
            .execute(&mut *tx)
            .await;

            match result {
                Ok(_) => break candidate_slug,
                Err(sqlx::Error::Database(ref e)) if is_sqlite_unique_constraint(e.as_ref()) => {
                    slug_offset += 1;
                    candidate_slug = if slug_offset <= 20 {
                        format!("{root_slug}-{}", base_n + slug_offset)
                    } else {
                        // Safety valve: fall back to UUID suffix.
                        let uid = uuid::Uuid::new_v4().to_string();
                        format!("{root_slug}-{}-{}", base_n, uid.get(..8).unwrap_or(&uid))
                    };
                }
                Err(e) => return Err(DbError::Sqlx(e)),
            }
        };

        sqlx::query(
            "DELETE FROM product_continuation_reservations
             WHERE predecessor_conversation_id = ?1",
        )
        .bind(parent_id)
        .execute(&mut *tx)
        .await?;

        if let Some(intent) = intent {
            sqlx::query(
                "INSERT INTO continuation_dispatch_intents (parent_conversation_id, successor_conversation_id, message_id, handoff, user_agent, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(parent_id)
            .bind(&new_id)
            .bind(&intent.message_id)
            .bind(&intent.handoff)
            .bind(intent.user_agent.as_deref())
            .bind(&now_str)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        let title_str = schema::title_from_slug(&actual_slug);
        let new_conversation = Conversation {
            id: new_id,
            product_conversation_id: parent.product_conversation_id,
            slug: Some(actual_slug),
            title: Some(title_str),
            cwd: parent.cwd,
            parent_conversation_id: None,
            user_initiated: parent.user_initiated,
            state: ConvState::Idle,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            transcript_generation: 1,
            model: parent.model,
            project_id: parent.project_id,
            conv_mode: parent.conv_mode,
            runtime_role: parent.runtime_role,
            attached_work_scope_id: continuation_work_scope_id,
            effort: parent.effort,
            service_tier: parent.service_tier,
            desired_base_branch: parent.desired_base_branch,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            // Continuations are not chain roots — chain_name lives on the
            // root only (REQ-CHN-007).
            chain_name: None,
            // Inherit language from the parent so the whole chain speaks the
            // same way.
            llm_language: parent.llm_language,
            spawned_from_conversation_id: None,
        };
        Ok(ContinueOutcome::Created(new_conversation))
    }

    /// Walk the continuation chain forward from `root_id` and return member
    /// conversation IDs in chain order (root first, leaf last). REQ-CHN-002.
    ///
    /// Returns:
    ///   - `[root_id]` when `root_id` exists with no continuation;
    ///   - `[root_id, …, leaf_id]` for a multi-member chain;
    ///   - empty vec when `root_id` doesn't exist.
    ///
    /// Implementation uses a recursive CTE on `continued_in_conv_id`. The
    /// `continued_in_conv_id` column is a single scalar pointer per row, so
    /// the chain is structurally linear; this method does not need to defend
    /// against fan-out.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn chain_members_forward(&self, root_id: &str) -> DbResult<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "WITH RECURSIVE chain(id, next_id, depth) AS (
                SELECT id, continued_in_conv_id, 0
                FROM conversations
                WHERE id = ?1
                UNION ALL
                SELECT c.id, c.continued_in_conv_id, chain.depth + 1
                FROM conversations c
                JOIN chain ON c.id = chain.next_id
            )
            SELECT id FROM chain ORDER BY depth",
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Forward chain members as fully-hydrated [`Conversation`] rows, ordered
    /// root-first by continuation depth.
    ///
    /// Equivalent to [`Self::chain_members_forward`] followed by a
    /// [`Self::get_conversation`] per id, but in a single query: the recursive
    /// chain walk is joined back to `conversations` so the whole chain is
    /// hydrated in one round-trip instead of N. Callers that need the member
    /// rows (not just their ids) should prefer this.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn chain_members_forward_full(&self, root_id: &str) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "WITH RECURSIVE chain(id, next_id, depth) AS (
                SELECT id, continued_in_conv_id, 0
                FROM conversations
                WHERE id = ?1
                UNION ALL
                SELECT c.id, c.continued_in_conv_id, chain.depth + 1
                FROM conversations c
                JOIN chain ON c.id = chain.next_id
            )
            SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                   c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                   c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                   c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                   (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
            FROM conversations c
            JOIN chain ON c.id = chain.id
            LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
            ORDER BY chain.depth",
        )
        .bind(root_id)
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Walk the continuation chain backward from `conv_id` to its root and
    /// return the root's id. REQ-CHN-002.
    ///
    /// Returns:
    ///   - `Some(root_id)` for any chain member (including a chain of length
    ///     one — `Some(conv_id)` when `conv_id` itself is the root);
    ///   - `None` when `conv_id` doesn't exist.
    ///
    /// Walks the inverse edge `WHERE p.continued_in_conv_id = current.id`
    /// until no predecessor exists.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn chain_root_of(&self, conv_id: &str) -> DbResult<Option<String>> {
        let row = sqlx::query_scalar::<_, String>(
            "WITH RECURSIVE chain(id, depth) AS (
                SELECT id, 0
                FROM conversations
                WHERE id = ?1
                UNION ALL
                SELECT p.id, chain.depth + 1
                FROM conversations p
                JOIN chain ON p.continued_in_conv_id = chain.id
            )
            SELECT id FROM chain ORDER BY depth DESC LIMIT 1",
        )
        .bind(conv_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Return `Some(root_id)` if `conv_id` is a member of a chain (≥2
    /// members), else `None`. Used by per-conversation lifecycle handlers
    /// to refuse archive/delete on chain members and route the caller to
    /// the chain endpoint via `conflict_slug`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn chain_root_if_member(&self, conv_id: &str) -> DbResult<Option<String>> {
        let Some(root) = self.chain_root_of(conv_id).await? else {
            return Ok(None);
        };
        let len = self.chain_members_forward(&root).await?.len();
        if len >= 2 {
            Ok(Some(root))
        } else {
            Ok(None)
        }
    }

    /// Set or clear the `chain_name` on a conversation (REQ-CHN-007).
    ///
    /// Phase 2 owns the column write; the API layer (Phase 4) is responsible
    /// for validating that `root_conv_id` is actually a chain root before
    /// calling this. Writing to a non-root row is structurally permitted but
    /// has no UI effect (`parse_conversation_row` only reads `chain_name`
    /// from the root).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Wired via API handlers in Phase 4 (task 02690)
    pub async fn set_chain_name(&self, root_conv_id: &str, name: Option<&str>) -> DbResult<()> {
        let result = sqlx::query("UPDATE conversations SET chain_name = ?1 WHERE id = ?2")
            .bind(name)
            .bind(root_conv_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(root_conv_id.to_string()));
        }
        Ok(())
    }

    /// Return the display text of the earliest *opening* message in a
    /// conversation, or `None` when there is none (REQ-CHN-010).
    ///
    /// An opening is a user-initiated message: a plain `user` message, or a
    /// `skill` invocation (a user action whose original trigger text is the
    /// opening intent — Phoenix persists a skill-invoking prompt as
    /// `message_type = 'skill'`, not `'user'`). System-generated user messages
    /// (task-approval seeds, `is_meta`) count too; agent/tool/continuation
    /// messages do not.
    ///
    /// "Earliest" is by `sequence_id` (the messages table's append order).
    /// Used by chain-name regeneration to summarize each member's opening
    /// intent.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn first_opening_message_text(&self, conv_id: &str) -> DbResult<Option<String>> {
        let row = sqlx::query(
            "SELECT message_type, content FROM messages
             WHERE conversation_id = ?1 AND message_type IN ('user', 'skill')
             ORDER BY sequence_id ASC
             LIMIT 1",
        )
        .bind(conv_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let message_type: String = row.try_get("message_type")?;
        let content_str: String = row.try_get("content")?;
        let value: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();

        // A row whose content fails to parse as its tagged type is corrupt;
        // treat it as "no usable opening" rather than surfacing a parse error
        // to the naming flow.
        let text = match message_type.as_str() {
            "user" => match MessageContent::from_stored_json(MessageType::User, value) {
                Ok(MessageContent::User(user)) => user.text,
                _ => return Ok(None),
            },
            // The trigger is the user's original text that invoked the skill —
            // the opening intent. The expanded body is the wrong thing to name
            // from. Fall back to the skill name if the trigger is empty.
            "skill" => match MessageContent::from_stored_json(MessageType::Skill, value) {
                Ok(MessageContent::Skill(skill)) => {
                    if skill.trigger.trim().is_empty() {
                        skill.name
                    } else {
                        skill.trigger
                    }
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };

        let text = text.trim();
        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text.to_string()))
        }
    }

    /// Insert a freshly-submitted Q&A row in the `in_flight` state (REQ-CHN-005).
    ///
    /// `answer` and `completed_at` are NULL at insertion. The row is
    /// transitioned via [`Database::complete_chain_qa`] or
    /// [`Database::fail_chain_qa`] when the stream resolves.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Wired through `chain_qa::ChainQa::submit_question` (Phase 2/3)
    pub async fn insert_chain_qa(&self, row: NewChainQa) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO chain_qa (
                id, root_conv_id, question, answer, model, status,
                chain_members_at_answer, chain_messages_at_answer,
                created_at, completed_at
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, NULL)",
        )
        .bind(&row.id)
        .bind(&row.root_conv_id)
        .bind(&row.question)
        .bind(&row.model)
        .bind(ChainQaStatus::InFlight.as_str())
        .bind(row.chain_members_at_answer)
        .bind(row.chain_messages_at_answer)
        .bind(row.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a Q&A row complete with the final answer and the chain snapshot as
    /// of completion (REQ-CHN-005).
    ///
    /// The snapshot counters are rewritten (not just set once at insert)
    /// because the Q&A agent resolves chain members live, so a continuation
    /// added mid-run can inform the answer; the freshness comparison must be
    /// against the shape the answer actually saw.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Wired through `chain_qa::ChainQa::submit_question` (Phase 2/3)
    pub async fn complete_chain_qa(
        &self,
        id: &str,
        answer: &str,
        chain_members_at_answer: i64,
        chain_messages_at_answer: i64,
        completed_at: DateTime<Utc>,
    ) -> DbResult<()> {
        let result = sqlx::query(
            "UPDATE chain_qa
             SET answer = ?1, status = ?2, completed_at = ?3,
                 chain_members_at_answer = ?4, chain_messages_at_answer = ?5
             WHERE id = ?6",
        )
        .bind(answer)
        .bind(ChainQaStatus::Completed.as_str())
        .bind(completed_at.to_rfc3339())
        .bind(chain_members_at_answer)
        .bind(chain_messages_at_answer)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Mark a Q&A row failed; `partial_answer` is preserved when present
    /// (REQ-CHN-005 — failed rows render with whatever tokens streamed).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Wired through `chain_qa::ChainQa::submit_question` (Phase 2/3)
    pub async fn fail_chain_qa(&self, id: &str, partial_answer: Option<&str>) -> DbResult<()> {
        let result = sqlx::query("UPDATE chain_qa SET answer = ?1, status = ?2 WHERE id = ?3")
            .bind(partial_answer)
            .bind(ChainQaStatus::Failed.as_str())
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// List Q&A rows for a chain in chronological order (REQ-CHN-005).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Wired through `chain_qa::ChainQa::list_history` and Phase 4 API handlers
    pub async fn list_chain_qa(&self, root_conv_id: &str) -> DbResult<Vec<ChainQaRow>> {
        let rows = sqlx::query(
            "SELECT id, root_conv_id, question, answer, model, status,
                    chain_members_at_answer, chain_messages_at_answer,
                    created_at, completed_at
             FROM chain_qa
             WHERE root_conv_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .bind(root_conv_id)
        .try_map(parse_chain_qa_row)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Transition every `in_flight` row to `abandoned` (startup sweep).
    ///
    /// Returns the number of rows transitioned. Called once at server start
    /// (REQ-CHN-005) — any row still `in_flight` after a process exit has
    /// no live stream behind it and would otherwise render as an
    /// indefinite "still working…" placeholder.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn sweep_in_flight_chain_qa(&self) -> DbResult<usize> {
        let result = sqlx::query("UPDATE chain_qa SET status = ?1 WHERE status = ?2")
            .bind(ChainQaStatus::Abandoned.as_str())
            .bind(ChainQaStatus::InFlight.as_str())
            .execute(&self.pool)
            .await?;
        Ok(usize::try_from(result.rows_affected()).unwrap_or(0))
    }

    // ==================== Fork Proposal Operations ====================

    /// Insert a new fork proposal (REQ-PROJ-033).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn insert_fork_proposal(&self, p: &ForkProposal) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        insert_fork_proposal_tx(&mut tx, p).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically persist a fork proposal together with the originating turn's
    /// tool round (REQ-PROJ-033): in a single transaction, write the assistant
    /// message row, each synthetic tool-result row, and the `fork_proposals`
    /// row. The synthetic success ack must never be durable without the
    /// control-plane row the review/approve surface reads, so a crash between
    /// the two is structurally impossible — either both commit or neither does.
    ///
    /// Message inserts use `INSERT OR IGNORE` on `message_id` so a crash-retry
    /// that finds the assistant/tool rows already present is a no-op rather than
    /// a UNIQUE failure; the proposal insert is keyed on its own `id`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn persist_fork_proposal_with_tool_round(
        &self,
        origin_conv_id: &str,
        assistant: &Message,
        tool_results: &[Message],
        proposal: &ForkProposal,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        insert_message_tx(&mut tx, assistant).await?;
        for msg in tool_results {
            insert_message_tx(&mut tx, msg).await?;
        }

        // Mirror `add_message_with_seq`'s side-effect: bump the conversation's
        // `updated_at` so list-ordering stays current.
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(Utc::now().to_rfc3339())
            .bind(origin_conv_id)
            .execute(&mut *tx)
            .await?;

        insert_fork_proposal_tx(&mut tx, proposal).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Atomically persist a completed tool round: the assistant message row and
    /// each of its tool-result rows commit in a single transaction, or none do.
    ///
    /// An assistant message carries one `tool_use` block per dispatched tool;
    /// each needs a paired `tool_result`. Writing them as independent statements
    /// (the historical shape of `persist_checkpoint`) admits a window where the
    /// assistant row is durable but one or more tool-result rows are not — e.g.
    /// `SQLITE_BUSY` past the busy timeout, or a crash between inserts. That
    /// leaves an unpaired `tool_use` in history: every subsequent LLM request
    /// 400s on the missing `tool_result`, and the restart repair sweep then
    /// overwrites the genuinely-completed tool's real output with an
    /// `[interrupted by server restart]` placeholder — permanent loss of work
    /// the user already saw. Committing the whole round as one transaction makes
    /// that partial state structurally impossible.
    ///
    /// Message inserts use `INSERT OR IGNORE` on `message_id` (via
    /// `insert_message_tx`) so a crash-retry that finds rows already present is a
    /// no-op rather than a UNIQUE failure.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn persist_tool_round(
        &self,
        conversation_id: &str,
        assistant: &Message,
        tool_results: &[Message],
    ) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        insert_message_tx(&mut tx, assistant).await?;
        for msg in tool_results {
            insert_message_tx(&mut tx, msg).await?;
        }

        // Mirror `add_message_with_seq`'s side-effect: bump the conversation's
        // `updated_at` so list-ordering stays current.
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(Utc::now().to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Persist a terminal tool checkpoint and its direct-turn obligation atomically.
    ///
    /// # Errors
    ///
    /// Returns an error if any message or obligation write fails or the transaction cannot commit.
    async fn persist_tool_round_with_terminal_obligation_at_cut(
        &self,
        conversation_id: &str,
        assistant: &Message,
        tool_results: &[Message],
        obligation: &workflow::DirectTurnTerminalObligationInput,
        cut: TerminalEvidenceTransactionCut,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        insert_message_tx(&mut tx, assistant).await?;
        for message in tool_results {
            insert_message_tx(&mut tx, message).await?;
        }
        workflow::WorkflowRepository::persist_terminal_obligation_tx(&mut tx, obligation).await?;
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(Utc::now().to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        if cut == TerminalEvidenceTransactionCut::BeforeCommit {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "injected before-commit cut".to_string(),
            ));
        }
        tx.commit().await?;
        if cut == TerminalEvidenceTransactionCut::AfterCommit {
            return Err(DbError::Serialization(
                "injected after-commit cut".to_string(),
            ));
        }
        Ok(())
    }

    /// Persist a terminal tool round and its obligation in one transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if any exact evidence write or the commit fails.
    pub async fn persist_tool_round_with_terminal_obligation(
        &self,
        conversation_id: &str,
        assistant: &Message,
        tool_results: &[Message],
        obligation: &workflow::DirectTurnTerminalObligationInput,
    ) -> DbResult<()> {
        self.persist_tool_round_with_terminal_obligation_at_cut(
            conversation_id,
            assistant,
            tool_results,
            obligation,
            TerminalEvidenceTransactionCut::None,
        )
        .await
    }

    /// Persist one sub-agent terminal transcript carrier and its obligation atomically.
    ///
    /// # Errors
    ///
    /// Returns an error if the exact carrier cannot be written or the commit fails.
    pub async fn persist_sub_agent_terminal_evidence(
        &self,
        evidence: &workflow::TerminalEvidenceExpectation,
        obligation: &workflow::DirectTurnTerminalObligationInput,
    ) -> DbResult<Option<i64>> {
        self.persist_sub_agent_terminal_evidence_at_cut(
            evidence,
            obligation,
            TerminalEvidenceTransactionCut::None,
        )
        .await
    }

    async fn persist_sub_agent_terminal_evidence_at_cut(
        &self,
        evidence: &workflow::TerminalEvidenceExpectation,
        obligation: &workflow::DirectTurnTerminalObligationInput,
        cut: TerminalEvidenceTransactionCut,
    ) -> DbResult<Option<i64>> {
        let mut tx = self.pool.begin().await?;
        let transcript_generation = match evidence {
            workflow::TerminalEvidenceExpectation::ObligationOnly { .. } => None,
            workflow::TerminalEvidenceExpectation::Messages(messages) => {
                for message in messages {
                    insert_message_tx(&mut tx, message).await?;
                }
                None
            }
            workflow::TerminalEvidenceExpectation::MessageMutation {
                conversation_id,
                message_id,
                content,
                display_data,
            } => {
                let content = serde_json::to_string(content)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                let display_data = serde_json::to_string(display_data)
                    .map_err(|error| DbError::Serialization(error.to_string()))?;
                let updated: Option<String> = sqlx::query_scalar(
                    "UPDATE messages SET content = ?1, display_data = ?2
                     WHERE message_id = ?3 AND conversation_id = ?4
                     RETURNING message_id",
                )
                .bind(content)
                .bind(display_data)
                .bind(message_id)
                .bind(conversation_id)
                .fetch_optional(&mut *tx)
                .await?;
                if updated.is_none() {
                    tx.rollback().await?;
                    return Err(DbError::MessageNotFound(message_id.clone()));
                }
                let updated_message = sqlx::query(
                    "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
                     FROM messages WHERE message_id = ?1",
                )
                .bind(message_id)
                .try_map(parse_message_row)
                .fetch_one(&mut *tx)
                .await?;
                retrieval::fts_upsert_conn(
                    &mut tx,
                    &updated_message,
                    retrieval::FtsObservation::ParentTransaction(
                        sqlite_telemetry::ParentSqliteObserver::UninstrumentedNested,
                    ),
                )
                .await?;
                Some(
                    sqlx::query_scalar(
                        "UPDATE conversations
                         SET transcript_generation = transcript_generation + 1
                         WHERE id = ?1 RETURNING transcript_generation",
                    )
                    .bind(conversation_id)
                    .fetch_one(&mut *tx)
                    .await?,
                )
            }
        };
        workflow::WorkflowRepository::persist_terminal_obligation_tx(&mut tx, obligation).await?;
        if cut == TerminalEvidenceTransactionCut::BeforeCommit {
            tx.rollback().await?;
            return Err(DbError::Serialization(
                "injected before-commit cut".to_string(),
            ));
        }
        tx.commit().await?;
        if cut == TerminalEvidenceTransactionCut::AfterCommit {
            return Err(DbError::Serialization(
                "injected after-commit cut".to_string(),
            ));
        }
        Ok(transcript_generation)
    }

    /// Fetch a fork proposal by id.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_fork_proposal(&self, id: &str) -> DbResult<Option<ForkProposal>> {
        let row = sqlx::query(FORK_PROPOSAL_SELECT_COLUMNS)
            .bind(id)
            .try_map(parse_fork_proposal_row)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    /// List all fork proposals for an origin conversation, oldest first.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_fork_proposals_for_origin(
        &self,
        origin_conv_id: &str,
    ) -> DbResult<Vec<ForkProposal>> {
        let rows = sqlx::query(
            "SELECT id, origin_conv_id, task_file, title, priority, body, status,
                    fork_conv_id, refinement_conv_id, created_at, resolved_at
             FROM fork_proposals
             WHERE origin_conv_id = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .bind(origin_conv_id)
        .try_map(parse_fork_proposal_row)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// List every still-`pending` fork proposal across all origins, oldest
    /// first. Used by the startup reconciliation pass that retires proposals
    /// whose origin conversation has reached a terminal state (REQ-PROJ-035):
    /// a crash after the origin went terminal but before its proposals were
    /// retired leaves them `pending`, so they must be swept on restart.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_pending_fork_proposals(&self) -> DbResult<Vec<ForkProposal>> {
        let rows = sqlx::query(
            "SELECT id, origin_conv_id, task_file, title, priority, body, status,
                    fork_conv_id, refinement_conv_id, created_at, resolved_at
             FROM fork_proposals
             WHERE status = ?1
             ORDER BY created_at ASC, id ASC",
        )
        .bind(ForkProposalStatus::Pending.as_str())
        .try_map(parse_fork_proposal_row)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Dismiss a pending fork proposal (REQ-PROJ-034). Idempotent: returns
    /// `true` iff a pending row was transitioned to `dismissed`; a second
    /// call (or dismissing an already-resolved proposal) updates nothing and
    /// returns `false`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn dismiss_fork_proposal(&self, id: &str) -> DbResult<bool> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE fork_proposals
             SET status = ?1, resolved_at = ?2
             WHERE id = ?3 AND status = ?4",
        )
        .bind(ForkProposalStatus::Dismissed.as_str())
        .bind(now.to_rfc3339())
        .bind(id)
        .bind(ForkProposalStatus::Pending.as_str())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Retire every still-`pending` proposal for an origin conversation by
    /// transitioning it to `dismissed` (REQ-PROJ-035 retire-on-terminal).
    /// Already-resolved proposals are untouched.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn retire_pending_fork_proposals_for_origin(
        &self,
        origin_conv_id: &str,
    ) -> DbResult<()> {
        let now = Utc::now();
        sqlx::query(
            "UPDATE fork_proposals
             SET status = ?1, resolved_at = ?2
             WHERE origin_conv_id = ?3 AND status = ?4",
        )
        .bind(ForkProposalStatus::Dismissed.as_str())
        .bind(now.to_rfc3339())
        .bind(origin_conv_id)
        .bind(ForkProposalStatus::Pending.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically resolve a pending proposal as `spawned` (REQ-PROJ-034): in a
    /// single transaction, persist the fork `Conversation` and its `seed`
    /// messages, then record the `spawned { fork_conv_id }` resolution.
    ///
    /// Idempotent on crash-retry: the fork conversation id is deterministic, so
    /// a retry may find the child row and/or the resolution already present. If
    /// the proposal is already `spawned` to the **same** `fork.id`, this is a
    /// no-op returning `Ok(())`. If it is resolved to a different state or id,
    /// returns [`DbError::ForkProposalConflict`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ForkProposalConflict`] on a divergent prior
    /// resolution, or a [`DbError`] for an underlying database failure.
    pub async fn resolve_fork_proposal_spawned(
        &self,
        proposal_id: &str,
        fork: &Conversation,
        seed_messages: &[Message],
    ) -> DbResult<()> {
        self.resolve_fork_proposal(
            proposal_id,
            fork,
            seed_messages,
            ForkProposalStatus::Spawned,
        )
        .await
    }

    /// Atomically resolve a pending proposal as `promoted` (REQ-PROJ-037): in a
    /// single transaction, persist the Explore refinement `Conversation` and
    /// its `seed` messages, then record the `promoted { refinement_conv_id }`
    /// resolution. Same idempotency / conflict semantics as
    /// [`Database::resolve_fork_proposal_spawned`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ForkProposalConflict`] on a divergent prior
    /// resolution, or a [`DbError`] for an underlying database failure.
    pub async fn resolve_fork_proposal_promoted(
        &self,
        proposal_id: &str,
        refinement: &Conversation,
        seed_messages: &[Message],
    ) -> DbResult<()> {
        self.resolve_fork_proposal(
            proposal_id,
            refinement,
            seed_messages,
            ForkProposalStatus::Promoted,
        )
        .await
    }

    /// Shared body for the two atomic resolve paths. `terminal_status` is
    /// `Spawned` or `Promoted`; the corresponding `fork_conv_id` /
    /// `refinement_conv_id` column is set from `child.id` while the other stays
    /// NULL.
    async fn resolve_fork_proposal(
        &self,
        proposal_id: &str,
        child: &Conversation,
        seed_messages: &[Message],
        terminal_status: ForkProposalStatus,
    ) -> DbResult<()> {
        debug_assert!(matches!(
            terminal_status,
            ForkProposalStatus::Spawned | ForkProposalStatus::Promoted
        ));

        // Idempotent short-circuit: a prior identical resolution converges to a
        // no-op; a divergent one is a conflict.
        let Some(existing) = self.get_fork_proposal(proposal_id).await? else {
            return Err(DbError::ConversationNotFound(format!(
                "fork proposal {proposal_id}"
            )));
        };
        if existing.status != ForkProposalStatus::Pending {
            // Already resolved: an identical prior resolution (same terminal
            // state + same child id) converges to a no-op; anything else is a
            // conflict.
            if existing.status == terminal_status
                && resolution_child_id(&existing) == Some(child.id.as_str())
            {
                return Ok(());
            }
            return Err(DbError::ForkProposalConflict(format!(
                "proposal {proposal_id} already resolved as {}",
                existing.status.as_str()
            )));
        }

        let now = Utc::now();
        let (fork_conv_id, refinement_conv_id) = match terminal_status {
            ForkProposalStatus::Spawned => (Some(child.id.as_str()), None),
            ForkProposalStatus::Promoted => (None, Some(child.id.as_str())),
            ForkProposalStatus::Pending | ForkProposalStatus::Dismissed => {
                // Excluded by the debug_assert at the top of this fn; treat as
                // a programmer error rather than silently mis-binding columns.
                return Err(DbError::ForkProposalConflict(format!(
                    "invalid terminal status {} for resolve",
                    terminal_status.as_str()
                )));
            }
        };

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES (?1, 'ordinary', 'open')
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(child.product_conversation_id.as_str())
        .execute(&mut *tx)
        .await?;
        insert_conversation_tx(&mut tx, child).await?;

        for msg in seed_messages {
            insert_message_tx(&mut tx, msg).await?;
        }

        // Guard the resolution on the pending state so a concurrent resolver
        // cannot double-resolve; the idempotent short-circuit above already
        // handled the same-id retry.
        let updated = sqlx::query(
            "UPDATE fork_proposals
             SET status = ?1, fork_conv_id = ?2, refinement_conv_id = ?3, resolved_at = ?4
             WHERE id = ?5 AND status = ?6",
        )
        .bind(terminal_status.as_str())
        .bind(fork_conv_id)
        .bind(refinement_conv_id)
        .bind(now.to_rfc3339())
        .bind(proposal_id)
        .bind(ForkProposalStatus::Pending.as_str())
        .execute(&mut *tx)
        .await?;

        if updated.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(DbError::ForkProposalConflict(format!(
                "proposal {proposal_id} was resolved concurrently"
            )));
        }

        tx.commit().await?;
        Ok(())
    }

    /// Normalize a tier only if the model and tier still match a previously read snapshot.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the conversation is absent or the database update fails.
    pub async fn compare_and_set_conversation_service_tier(
        &self,
        id: &str,
        expected_model: Option<&str>,
        expected_service_tier: ServiceTier,
        service_tier: ServiceTier,
    ) -> DbResult<bool> {
        let result = sqlx::query(
            "UPDATE conversations\n             SET service_tier = ?1, updated_at = ?2\n             WHERE id = ?3\n               AND service_tier = ?4\n               AND ((model IS NULL AND ?5 IS NULL) OR model = ?5)",
        )
        .bind(service_tier.as_wire_name())
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .bind(expected_service_tier.as_wire_name())
        .bind(expected_model)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() > 0 {
            return Ok(true);
        }
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(1) FROM conversations WHERE id = ?1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        if exists == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(false)
    }

    /// Atomically update the model, effort, and service tier.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the conversation is absent or the database update fails.
    pub async fn update_conversation_model_and_effort(
        &self,
        id: &str,
        model: &str,
        effort: Option<ModelEffort>,
        service_tier: ServiceTier,
    ) -> DbResult<()> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE conversations SET model = ?1, effort = ?2, service_tier = ?3, updated_at = ?4 WHERE id = ?5",
        )
        .bind(model)
        .bind(effort.map(ModelEffort::as_wire_name))
        .bind(service_tier.as_wire_name())
        .bind(now.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Update creation-time conversation metadata after async provisioning.
    ///
    /// Fields left as `None` are untouched; `Some(None)` clears a nullable
    /// column; `Some(Some(v))` writes the value.
    /// Update conversation metadata after async creation provisioning completes.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the update fails or the conversation does not exist.
    pub async fn update_conversation_creation_metadata(
        &self,
        id: &str,
        update: &ConversationCreationMetadataUpdate,
    ) -> DbResult<()> {
        let base_slug = update.slug.clone();
        let mut candidate_slug = base_slug.clone();
        let mut attempts = 0u8;
        loop {
            let now = Utc::now().to_rfc3339();
            let result = sqlx::query(
                "UPDATE conversations
                 SET slug = COALESCE(?1, slug),
                     title = CASE
                         WHEN ?2 = 1 THEN ?3
                         ELSE title
                     END,
                     project_id = CASE
                         WHEN ?5 = 1 THEN ?6
                         ELSE project_id
                     END,
                     desired_base_branch = CASE
                         WHEN ?7 = 1 THEN ?8
                         ELSE desired_base_branch
                     END,
                     updated_at = ?9
                 WHERE id = ?10",
            )
            .bind(candidate_slug.as_deref())
            .bind(update.title.is_some())
            .bind(update.title.as_ref().and_then(|v| v.as_deref()))
            .bind(update.cwd.as_deref())
            .bind(update.project_id.is_some())
            .bind(update.project_id.as_ref().and_then(|v| v.as_deref()))
            .bind(update.desired_base_branch.is_some())
            .bind(
                update
                    .desired_base_branch
                    .as_ref()
                    .and_then(|v| v.as_deref()),
            )
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await;
            match result {
                Ok(result) => {
                    if result.rows_affected() == 0 {
                        return Err(DbError::ConversationNotFound(id.to_string()));
                    }
                    if let Some(cwd) = update.cwd.as_deref() {
                        let updated = sqlx::query(
                            "UPDATE work_scopes SET cwd = ?1, updated_at = ?2
                             WHERE id = (SELECT work_scope_id FROM conversations WHERE id = ?3)",
                        )
                        .bind(cwd)
                        .bind(&now)
                        .bind(id)
                        .execute(&self.pool)
                        .await?;
                        if updated.rows_affected() == 0 {
                            return Err(DbError::ConversationNotFound(id.to_string()));
                        }
                    }
                    return Ok(());
                }
                Err(sqlx::Error::Database(ref e))
                    if is_sqlite_unique_constraint(e.as_ref()) && base_slug.is_some() =>
                {
                    attempts += 1;
                    let slug = base_slug.as_deref().unwrap_or_default();
                    if attempts >= 10 {
                        let uuid_str = uuid::Uuid::new_v4().to_string();
                        candidate_slug =
                            Some(format!("{slug}-{}", uuid_str.get(..8).unwrap_or(&uuid_str)));
                    } else {
                        candidate_slug = Some(format!("{slug}-{:04x}", rand::random::<u16>()));
                    }
                }
                Err(e) => return Err(DbError::Sqlx(e)),
            }
        }
    }

    /// Update async creation metadata and ownership mode in one transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if serialization or either write fails.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn update_conversation_creation_metadata_and_mode(
        &self,
        job_id: &str,
        claim: &CreationClaim,
        id: &str,
        update: &ConversationCreationMetadataUpdate,
        mode: &ConvMode,
        model: &str,
        expected_stage: CreationStage,
        next_stage: CreationStage,
    ) -> DbResult<CreationCasOutcome> {
        let cm = conv_mode_columns(mode);
        let base_slug = update.slug.clone();
        let mut candidate_slug = base_slug.clone();
        let mut attempts = 0u8;
        loop {
            let now = Utc::now().to_rfc3339();
            let mut tx = self.pool.begin().await?;
            let result = sqlx::query(
                "UPDATE conversations
                 SET slug = COALESCE(?1, slug),
                     title = CASE
                         WHEN ?2 = 1 THEN ?3
                         ELSE title
                     END,
                     project_id = CASE
                         WHEN ?4 = 1 THEN ?5
                         ELSE project_id
                     END,
                     desired_base_branch = CASE
                         WHEN ?6 = 1 THEN ?7
                         ELSE desired_base_branch
                     END,
                     cm_kind = ?8,
                     cm_task_id = ?9,
                     cm_task_title = ?10,
                     cm_next_taskmd_id_hint = ?11,
                     model = ?12,
                     updated_at = ?13
                 WHERE id = ?14
                   AND EXISTS (
                       SELECT 1 FROM conversation_creation_jobs j
                       WHERE j.id = ?15 AND j.conversation_id = conversations.id
                         AND j.status = 'claimed' AND j.generation = ?16
                         AND j.claim_worker_id = ?17 AND j.claim_token = ?18
                         AND j.lease_until > ?13 AND j.stage = ?19
                   )",
            )
            .bind(candidate_slug.as_deref())
            .bind(update.title.is_some())
            .bind(update.title.as_ref().and_then(|v| v.as_deref()))
            .bind(update.project_id.is_some())
            .bind(update.project_id.as_ref().and_then(|v| v.as_deref()))
            .bind(update.desired_base_branch.is_some())
            .bind(
                update
                    .desired_base_branch
                    .as_ref()
                    .and_then(|v| v.as_deref()),
            )
            .bind(cm.kind)
            .bind(cm.task_id)
            .bind(cm.task_title)
            .bind(cm.next_taskmd_id_hint)
            .bind(model)
            .bind(&now)
            .bind(id)
            .bind(job_id)
            .bind(claim_generation_i64(claim)?)
            .bind(&claim.worker_id.0)
            .bind(&claim.token.0)
            .bind(creation_stage_db_str(expected_stage))
            .execute(&mut *tx)
            .await;
            match result {
                Ok(result) => {
                    if result.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(CreationCasOutcome::ClaimLost);
                    }
                    let conversation_row = sqlx::query(
                        "SELECT c.work_scope_id
                         FROM conversations c
                         WHERE c.id = ?1",
                    )
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
                    let environment_scope = if let Some(raw_scope_id) =
                        conversation_row.get::<Option<String>, _>("work_scope_id")
                    {
                        let environment_scope = WorkScopeId::parse(raw_scope_id)
                            .map_err(|error| DbError::Serialization(error.to_string()))?;
                        let environment_row = sqlx::query(
                            "SELECT cwd FROM work_scope_environments WHERE work_scope_id = ?1",
                        )
                        .bind(environment_scope.as_str())
                        .fetch_one(&mut *tx)
                        .await?;
                        let persisted_environment_cwd: String = environment_row.get("cwd");
                        let environment_cwd =
                            update.cwd.as_deref().unwrap_or(&persisted_environment_cwd);
                        Self::update_work_scope_environment_tx(
                            &mut tx,
                            &environment_scope,
                            Self::environment_for_mode(environment_cwd, &cm),
                            &now,
                        )
                        .await?;
                        environment_scope
                    } else {
                        let environment_cwd = update.cwd.as_deref().ok_or_else(|| {
                            DbError::Serialization(
                                "cannot allocate work scope without a persisted cwd".to_string(),
                            )
                        })?;
                        let (scope_id, authority_kind, environment) =
                            Self::new_scope_for_conversation(environment_cwd, &cm);
                        Self::insert_work_scope_tx(
                            &mut tx,
                            &scope_id,
                            authority_kind,
                            environment,
                            &now,
                        )
                        .await?;
                        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = ?2")
                            .bind(scope_id.as_str())
                            .bind(id)
                            .execute(&mut *tx)
                            .await?;
                        scope_id
                    };

                    let authority = match mode {
                        ConvMode::Explore { .. } => AuthorityKind::RestrictedExplore,
                        ConvMode::Direct | ConvMode::Work { .. } | ConvMode::Branch { .. } => {
                            AuthorityKind::Work
                        }
                    };
                    sqlx::query(
                        "UPDATE work_scopes
                         SET authority_kind = ?1, updated_at = ?2
                         WHERE id = ?3",
                    )
                    .bind(authority.as_str())
                    .bind(&now)
                    .bind(environment_scope.as_str())
                    .execute(&mut *tx)
                    .await?;

                    let stage_updated = sqlx::query(
                        "UPDATE conversation_creation_jobs SET stage = ?1, updated_at = ?2
                         WHERE id = ?3 AND status = 'claimed' AND generation = ?4
                           AND claim_worker_id = ?5 AND claim_token = ?6
                           AND lease_until > ?2 AND stage = ?7",
                    )
                    .bind(creation_stage_db_str(next_stage))
                    .bind(&now)
                    .bind(job_id)
                    .bind(claim_generation_i64(claim)?)
                    .bind(&claim.worker_id.0)
                    .bind(&claim.token.0)
                    .bind(creation_stage_db_str(expected_stage))
                    .execute(&mut *tx)
                    .await?;
                    if stage_updated.rows_affected() != 1 {
                        tx.rollback().await?;
                        return Ok(CreationCasOutcome::ClaimLost);
                    }
                    tx.commit().await?;
                    return Ok(CreationCasOutcome::Applied);
                }
                Err(sqlx::Error::Database(ref e))
                    if is_sqlite_unique_constraint(e.as_ref()) && base_slug.is_some() =>
                {
                    attempts += 1;
                    let slug = base_slug.as_deref().unwrap_or_default();
                    if attempts >= 10 {
                        let uuid_str = uuid::Uuid::new_v4().to_string();
                        candidate_slug =
                            Some(format!("{slug}-{}", uuid_str.get(..8).unwrap_or(&uuid_str)));
                    } else {
                        candidate_slug = Some(format!("{slug}-{:04x}", rand::random::<u16>()));
                    }
                }
                Err(e) => return Err(DbError::Sqlx(e)),
            }
        }
    }

    /// Get all non-archived Work/Branch conversations (for startup worktree reconciliation).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_work_conversations(&self) -> DbResult<Vec<Conversation>> {
        sqlx::query(
            "SELECT c.id, c.product_conversation_id, c.slug, c.title, COALESCE(c.sub_agent_cwd_override, e.cwd, '') AS cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.transcript_generation, c.model, c.effort, c.service_tier,
                    c.project_id, c.desired_base_branch,
                    c.runtime_role, c.work_scope_id,
                    c.cm_kind, e.branch_name AS env_branch_name, e.worktree_path AS env_worktree_path, e.base_branch AS env_base_branch, c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.archived = 0
               AND c.cm_kind IN ('work', 'branch')",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::Sqlx)
    }

    /// Archive a conversation
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn archive_conversation(&self, id: &str) -> DbResult<()> {
        let now = Utc::now();

        let result =
            sqlx::query("UPDATE conversations SET archived = 1, updated_at = ?1 WHERE id = ?2")
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    async fn delete_product_conversation_if_empty(
        connection: &mut sqlx::SqliteConnection,
        product_conversation_id: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "DELETE FROM product_conversations
             WHERE id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM conversations
                   WHERE product_conversation_id = ?1
               )",
        )
        .bind(product_conversation_id)
        .execute(connection)
        .await?;
        Ok(())
    }

    async fn delete_work_scope_if_empty(
        connection: &mut sqlx::SqliteConnection,
        work_scope_id: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "DELETE FROM work_scopes
             WHERE id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM conversations WHERE work_scope_id = ?1
               )",
        )
        .bind(work_scope_id)
        .execute(connection)
        .await?;
        Ok(())
    }

    async fn delete_conversation_row_with_dependents(
        connection: &mut sqlx::SqliteConnection,
        conversation_id: &str,
        observer: sqlite_telemetry::ParentSqliteObserver<'_>,
        creation_cleanup_claim: Option<(&CreationCleanupJob, i64, &str)>,
    ) -> DbResult<Option<String>> {
        let membership: Option<String> =
            sqlx::query_scalar("SELECT product_conversation_id FROM conversations WHERE id = ?1")
                .bind(conversation_id)
                .fetch_optional(&mut *connection)
                .await?;
        let Some(membership) = membership else {
            return Ok(None);
        };

        sqlx::query(
            "DELETE FROM workflows
             WHERE workflow_id IN (
                 SELECT workflow_id FROM wake_bindings WHERE conversation_id = ?1
             )",
        )
        .bind(conversation_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO direct_turn_retirements (turn_id, conversation_id)
             SELECT turn_id, conversation_id FROM durable_turns
             WHERE conversation_id = ?1 AND disposition = 'Runtime'",
        )
        .bind(conversation_id)
        .execute(&mut *connection)
        .await?;
        let deleted = if let Some((cleanup, generation, now_str)) = creation_cleanup_claim {
            sqlx::query(
                "DELETE FROM conversations
                 WHERE id = ?1 AND EXISTS (
                     SELECT 1 FROM conversation_creation_jobs job
                     WHERE job.conversation_id = conversations.id AND job.id = ?2
                       AND job.status = 'deletion_pending' AND job.generation = ?3
                       AND job.cleanup_worker_id = ?4 AND job.cleanup_token = ?5
                       AND job.cleanup_lease_until > ?6
                 )",
            )
            .bind(conversation_id)
            .bind(&cleanup.job_id)
            .bind(generation)
            .bind(&cleanup.worker_id)
            .bind(&cleanup.token)
            .bind(now_str)
            .execute(&mut *connection)
            .await?
        } else {
            sqlx::query("DELETE FROM conversations WHERE id = ?1")
                .bind(conversation_id)
                .execute(&mut *connection)
                .await?
        };
        if deleted.rows_affected() == 0 {
            return Ok(None);
        }
        retrieval::fts_delete_conversation_conn(&mut *connection, conversation_id, observer)
            .await?;
        Ok(Some(membership))
    }

    async fn delete_subordinates_if_last_parent(
        connection: &mut sqlx::SqliteConnection,
        product_conversation_id: &str,
        deleted_conversation_id: &str,
        observer: sqlite_telemetry::ParentSqliteObserver<'_>,
    ) -> DbResult<()> {
        let parent_members: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations
             WHERE product_conversation_id = ?1
               AND id <> ?2
               AND runtime_role IN ('user', 'coordinator')
               AND parent_conversation_id IS NULL",
        )
        .bind(product_conversation_id)
        .bind(deleted_conversation_id)
        .fetch_one(&mut *connection)
        .await?;
        if parent_members == 0 {
            let subordinate_ids: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM conversations
                 WHERE product_conversation_id = ?1
                   AND runtime_role = 'sub_agent'",
            )
            .bind(product_conversation_id)
            .fetch_all(&mut *connection)
            .await?;
            for subordinate_id in subordinate_ids {
                let deleted = Self::delete_conversation_row_with_dependents(
                    connection,
                    &subordinate_id,
                    observer,
                    None,
                )
                .await?;
                debug_assert!(deleted.is_some(), "selected subordinate must still exist");
            }
        }
        Ok(())
    }

    async fn hard_delete_conversation_tx(
        connection: &mut sqlx::SqliteConnection,
        conversation_id: &str,
        observer: sqlite_telemetry::ParentSqliteObserver<'_>,
        creation_cleanup_claim: Option<(&CreationCleanupJob, i64, &str)>,
    ) -> DbResult<bool> {
        let Some(product_conversation_id) = Self::delete_conversation_row_with_dependents(
            connection,
            conversation_id,
            observer,
            creation_cleanup_claim,
        )
        .await?
        else {
            return Ok(false);
        };
        Self::delete_subordinates_if_last_parent(
            connection,
            &product_conversation_id,
            conversation_id,
            observer,
        )
        .await?;
        sqlx::query(
            "DELETE FROM product_root_reservations
             WHERE consumed_by_conversation_id = ?1",
        )
        .bind(conversation_id)
        .execute(&mut *connection)
        .await?;
        Self::delete_product_conversation_if_empty(connection, &product_conversation_id).await?;
        Ok(true)
    }

    /// Delete a conversation and all its messages
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(clippy::too_many_lines)]
    pub async fn delete_conversation(&self, id: &str) -> DbResult<()> {
        let telemetry = self.sqlite_telemetry(
            SqliteOperation::ConversationDelete,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
        );
        let (mut connection, acquisition) = telemetry
            .observe_pool_acquisition_sqlx(self.pool.acquire())
            .await?;
        let ((), timing) = telemetry
            .observe_transaction_admission_db(acquisition, async {
                sqlx::query("BEGIN IMMEDIATE")
                    .execute(&mut *connection)
                    .await
                    .map(|_| ())
                    .map_err(DbError::from)
            })
            .await?;
        let body = Self::hard_delete_conversation_tx(
            &mut connection,
            id,
            telemetry.parent_observer(),
            None,
        )
        .await;

        match body {
            Ok(true) => {
                telemetry
                    .observe_commit_db(timing, async {
                        sqlx::query("COMMIT")
                            .execute(&mut *connection)
                            .await
                            .map(|_| ())
                            .map_err(DbError::from)
                    })
                    .await
            }
            Ok(false) => {
                telemetry
                    .observe_failure_rollback_db(timing, async {
                        sqlx::query("ROLLBACK")
                            .execute(&mut *connection)
                            .await
                            .map(|_| ())
                            .map_err(DbError::from)
                    })
                    .await?;
                Err(DbError::ConversationNotFound(id.to_string()))
            }
            Err(error) => {
                telemetry
                    .observe_failure_rollback_db(timing, async {
                        sqlx::query("ROLLBACK")
                            .execute(&mut *connection)
                            .await
                            .map(|_| ())
                            .map_err(DbError::from)
                    })
                    .await?;
                Err(error)
            }
        }
    }

    /// Rename conversation (update slug)
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn rename_conversation(&self, id: &str, new_slug: &str) -> DbResult<()> {
        let now = Utc::now();

        // Check if slug already exists
        let row =
            sqlx::query("SELECT EXISTS(SELECT 1 FROM conversations WHERE slug = ?1 AND id != ?2)")
                .bind(new_slug)
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        let exists: bool = row.get(0);

        if exists {
            return Err(DbError::SlugExists(new_slug.to_string()));
        }

        let result =
            sqlx::query("UPDATE conversations SET slug = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(new_slug)
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// List conversations with terminal obligations owed at startup.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub async fn terminal_obligated_conversation_ids(
        &self,
    ) -> DbResult<std::collections::HashSet<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT t.conversation_id
             FROM durable_turns AS t
             JOIN direct_turn_terminal_obligations AS o ON o.turn_id = t.turn_id
             UNION
             SELECT DISTINCT child.parent_conversation_id
             FROM durable_turns AS t
             JOIN direct_turn_terminal_obligations AS o ON o.turn_id = t.turn_id
             JOIN conversations AS child ON child.id = t.conversation_id
             WHERE child.parent_conversation_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect())
    }

    /// Remove process-scoped hard-delete retirement evidence at process start.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot clear the retirement table.
    pub async fn clear_direct_turn_retirements(&self) -> DbResult<()> {
        sqlx::query("DELETE FROM direct_turn_retirements")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Reset all conversations to idle on server restart.
    /// Also repairs any orphaned `tool_use` by injecting synthetic `tool_result`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    pub async fn reset_all_to_idle(&self) -> DbResult<()> {
        let now = Utc::now();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();

        // Materialize the assistant turn + completed tool results that an
        // in-flight `ToolExecuting` round holds ONLY in its state JSON, before
        // the reset below overwrites that state with idle. The assistant message
        // and the already-completed tools' real outputs were broadcast over SSE
        // and seen by the user, but are not yet in `messages` (atomic
        // persistence happens at end-of-round). Without this, a deploy/crash
        // mid-round silently drops them and rewinds the conversation to the
        // prior user turn (REQ-BED-007, F1).
        let materialized = self.materialize_in_flight_tool_rounds(&now, None).await?;
        if !materialized.is_empty() {
            self.reconcile_startup_obligated_parents(&materialized)
                .await?;
        }

        // Then repair any orphaned tool_use blocks. After materialization the
        // round above is fully paired, so this is a no-op for it; it remains the
        // backstop for any other orphan shape (e.g. a partial pre-fix write).
        self.repair_orphaned_tool_use(&now).await?;

        // AwaitingRecovery is polymorphic: continuation-summary credential
        // recovery must survive restart, while an interrupted ordinary turn
        // resets per REQ-BED-007. Decode the aggregate rather than querying its
        // JSON fields from SQL.
        let recovery_rows = sqlx::query(
            "SELECT id, state FROM conversations WHERE state_kind = 'awaiting_recovery'",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in recovery_rows {
            let id: String = row.try_get("id")?;
            let state_json: String = row.try_get("state")?;
            let state: ConvState = serde_json::from_str(&state_json)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            if !matches!(
                state,
                ConvState::AwaitingRecovery {
                    resume:
                        phoenix_core::domain::sm_state::RecoveryResumeTarget::ContinuationSummary { .. },
                    ..
                }
            ) {
                self.update_conversation_state_at(&id, &ConvState::Idle, now)
                    .await?;
            }
        }

        // Reset non-terminal conversations to idle.
        // Preserved states (NOT reset):
        //   - context_exhausted: completed conversations that cannot accept new messages
        //   - awaiting_task_approval: user approval pending; state data (title/priority/plan)
        //     is in the JSON column and must survive restart
        //   - awaiting_user_response: user questions pending; state data (questions/tool_use_id)
        //     is in the JSON column and must survive restart
        //   - completed/failed/terminal: lifecycle ended — permanently read-only
        sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
             WHERE state_kind NOT IN ('idle', 'provisioning', 'completed', 'failed', 'creation_failed', 'creation_cancelled', 'context_exhausted', 'handed_off', 'seeded_llm_requesting', 'awaiting_continuation', 'recoverable_continuation_failure', 'awaiting_recovery', 'awaiting_task_approval', 'awaiting_user_response', 'terminal')
               AND NOT EXISTS (
                   SELECT 1
                   FROM durable_turns AS obligated_turn
                   JOIN direct_turn_terminal_obligations AS obligation
                     ON obligation.turn_id = obligated_turn.turn_id
                   WHERE obligated_turn.disposition = 'Runtime'
                     AND (
                         obligated_turn.conversation_id = conversations.id
                         OR obligated_turn.conversation_id IN (
                             SELECT child.id FROM conversations AS child
                             WHERE child.parent_conversation_id = conversations.id
                         )
                     )
               )
               AND NOT (
                   state_kind = 'llm_requesting'
                   AND (
                       EXISTS (
                           SELECT 1 FROM conversation_creation_jobs j
                           WHERE j.conversation_id = conversations.id
                             AND j.status IN ('accepted', 'claimed', 'retry_scheduled')
                       )
                       OR EXISTS (
                           SELECT 1 FROM durable_turns t
                           WHERE t.conversation_id = conversations.id
                             AND t.owns_conversation = 1
                             AND t.canonical_message_id IS NOT NULL
                             AND t.terminal_kind IS NULL
                       )
                       OR EXISTS (
                           SELECT 1
                           FROM messages m
                           JOIN steering_acceptance_receipts r
                             ON r.conversation_id = m.conversation_id
                            AND r.message_id = m.message_id
                           WHERE m.conversation_id = conversations.id
                             AND m.message_type IN ('user', 'skill')
                             AND m.sequence_id = (
                                 SELECT MAX(latest.sequence_id)
                                 FROM messages latest
                                 WHERE latest.conversation_id = m.conversation_id
                             )
                             AND NOT EXISTS (
                                 SELECT 1
                                 FROM steering_messages queued
                                 WHERE queued.conversation_id = m.conversation_id
                                   AND queued.message_id = m.message_id
                             )
                       )
                   )
               )",
        )
        .bind(&idle_state)
        .bind(conv_state_kind(&ConvState::Idle))
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Materialize the assistant message and completed tool results that an
    /// in-flight `ToolExecuting` or `CancellingTool` round carries only in its
    /// `ConvState` JSON, so they survive the startup reset that overwrites the
    /// state with idle.
    ///
    /// Both states bundle the un-persisted round for atomic end-of-round
    /// persistence and hold the same recoverable data, differing only in how
    /// the unfinished tools are named:
    ///   - `assistant_message`: the LLM turn (with one `tool_use` block per
    ///     dispatched tool) — broadcast over SSE and seen by the user, but not
    ///     yet in `messages`;
    ///   - `completed_results`: real outputs of tools that already finished;
    ///   - the unfinished tools: `ToolExecuting` carries the running tool as
    ///     `current_tool` (a `ToolCall`) plus `remaining_tools`; `CancellingTool`
    ///     carries the tool being aborted as `tool_use_id` (just the id) plus
    ///     `skipped_tools`. Both are normalized to a flat list of interrupted
    ///     `tool_use` ids — that's all the builder needs to pair a synthetic
    ///     result.
    ///
    /// Each `tool_use` needs exactly one paired `tool_result` or the next LLM
    /// request 400s. We write the assistant message, then one result per
    /// `tool_use`: completed tools contribute their real result, and every
    /// unfinished tool gets a synthetic interrupted error. The whole round
    /// commits atomically via [`Database::persist_tool_round`].
    ///
    /// The pairing count is checked before any write: if the materialized
    /// results don't match the assistant's `tool_use` count, we skip this
    /// conversation rather than persist a mis-paired chain — the subsequent
    /// reset + `repair_orphaned_tool_use` then handles it the legacy way.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    /// Resolve the real terminal outcome of each pending sub-agent by reading
    /// its child conversation row (created with `id == agent_id`). A sub-agent
    /// that reached `Completed`/`Failed` before the restart is returned with its
    /// real `Success`/`Failure` outcome; one still running (or whose row is
    /// missing or non-terminal) is omitted, so the caller falls back to the
    /// "interrupted by server restart" synthetic outcome. Query and decode
    /// failures remain errors because they cannot prove the child nonterminal.
    async fn resolve_pending_sub_agent_outcomes(
        &self,
        pending: &[phoenix_core::domain::sm_state::PendingSubAgent],
    ) -> DbResult<std::collections::HashMap<String, phoenix_core::domain::sm_state::SubAgentOutcome>>
    {
        use phoenix_core::domain::sm_state::{ConvState, SubAgentOutcome};
        use std::collections::HashMap;

        let mut outcomes = HashMap::new();
        for agent in pending {
            let row: Option<String> =
                sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
                    .bind(&agent.agent_id)
                    .fetch_optional(&self.pool)
                    .await?;
            let Some(state_json) = row else { continue };
            let state = serde_json::from_str::<ConvState>(&state_json).map_err(|error| {
                DbError::Serialization(format!(
                    "decode pending sub-agent {} state: {error}",
                    agent.agent_id
                ))
            })?;
            // Only the two terminal sub-agent states carry a real outcome; any
            // other (still running) state leaves the agent absent so the caller
            // uses the interrupted fallback. `if let` chain rather than a match
            // with a wildcard arm (denied by `wildcard_enum_match_arm`).
            if let ConvState::Completed { result } = state {
                outcomes.insert(agent.agent_id.clone(), SubAgentOutcome::Success { result });
            } else if let ConvState::Failed { error, error_kind } = state {
                outcomes.insert(
                    agent.agent_id.clone(),
                    SubAgentOutcome::Failure { error, error_kind },
                );
            }
        }
        Ok(outcomes)
    }

    #[allow(clippy::too_many_lines)]
    async fn materialize_in_flight_tool_rounds(
        &self,
        now: &DateTime<Utc>,
        only_conversations: Option<&std::collections::HashSet<String>>,
    ) -> DbResult<std::collections::HashSet<String>> {
        use phoenix_core::domain::sm_state::ConvState;

        // Both `tool_executing` and `cancelling_tool` rows carry an
        // un-persisted assistant turn (the cancel snapshots the in-flight round
        // until abort/complete persists the checkpoint).
        let conv_rows: Vec<(String, String, String)> = sqlx::query(
            "SELECT c.id, c.state, c.state_updated_at FROM conversations AS c
             WHERE c.state_kind IN ('tool_executing', 'cancelling_tool')
               AND NOT EXISTS (
                   SELECT 1
                   FROM durable_turns AS t
                   JOIN direct_turn_terminal_obligations AS o ON o.turn_id = t.turn_id
                   WHERE t.disposition = 'Runtime'
                     AND (
                         t.conversation_id = c.id
                         OR t.conversation_id IN (
                             SELECT child.id FROM conversations AS child
                             WHERE child.parent_conversation_id = c.id
                         )
                     )
               )",
        )
        .try_map(|row: SqliteRow| {
            Ok((
                row.try_get("id")?,
                row.try_get("state")?,
                row.try_get("state_updated_at")?,
            ))
        })
        .fetch_all(&self.pool)
        .await?;

        let mut materialized = std::collections::HashSet::new();
        for (conv_id, state_json, state_updated_at) in conv_rows {
            if only_conversations.is_some_and(|ids| !ids.contains(&conv_id)) {
                continue;
            }
            let state: ConvState = match serde_json::from_str(&state_json) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        conv_id = %conv_id, error = %e,
                        "could not parse in-flight tool round state for materialization; \
                         leaving to reset + orphan repair",
                    );
                    continue;
                }
            };

            // Normalize both states to the assistant message, completed
            // results, interrupted tool ids, and pending sub-agents. The
            // unfinished tools are identified by id in both states. The SQL
            // `WHERE` already restricts rows to these two states, so
            // `normalize_in_flight_round` returning `None` is unreachable — but
            // kept total rather than panicking.
            let Some(NormalizedRound {
                assistant_message,
                completed_results,
                interrupted_tool_ids,
                pending_sub_agents,
            }) = normalize_in_flight_round(state)
            else {
                continue;
            };

            // A sub-agent can have reached its own terminal state (persisted in
            // its child conversation row) while the parent only buffered the
            // result in memory. Read those real outcomes so a finished sub-agent
            // is fanned in as success/failure rather than "interrupted".
            let sub_agent_outcomes = self
                .resolve_pending_sub_agent_outcomes(&pending_sub_agents)
                .await?;

            let materialized_at = parse_datetime(&state_updated_at);
            let persisted_start: Option<i64> =
                sqlx::query_scalar("SELECT sequence_id FROM messages WHERE message_id = ?1")
                    .bind(&assistant_message.message_id)
                    .fetch_optional(&self.pool)
                    .await?;
            let start_seq = match persisted_start {
                Some(sequence_id) => sequence_id,
                None => self.next_sequence_id(&conv_id).await?,
            };
            let (agent_msg, tool_msgs) = build_materialized_tool_round(
                &conv_id,
                start_seq,
                &materialized_at,
                &assistant_message,
                &completed_results,
                &interrupted_tool_ids,
                &pending_sub_agents,
                &sub_agent_outcomes,
            );

            // Pairing invariant: every `tool_use` in the assistant message gets
            // exactly one result. Refuse to persist a mis-paired chain.
            let tool_use_count = assistant_message.tool_uses().len();
            if tool_use_count != tool_msgs.len() {
                tracing::warn!(
                    conv_id = %conv_id,
                    tool_uses = tool_use_count,
                    results = tool_msgs.len(),
                    "in-flight tool round has mismatched tool_use/result counts; \
                     skipping materialization, leaving to orphan repair",
                );
                continue;
            }

            self.persist_tool_round(&conv_id, &agent_msg, &tool_msgs)
                .await?;
            materialized.insert(conv_id.clone());

            let interrupted_state = serde_json::to_string(&ConvState::Failed {
                error: "Sub-agent interrupted by server restart".to_string(),
                error_kind: phoenix_core::domain::db_schema::ErrorKind::SubAgentError,
            })
            .unwrap();
            for agent in &pending_sub_agents {
                if sub_agent_outcomes.contains_key(&agent.agent_id) {
                    continue;
                }
                sqlx::query(
                    "UPDATE conversations
                     SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
                     WHERE id = ?4
                       AND state_kind NOT IN
                           ('completed', 'failed', 'creation_failed', 'creation_cancelled',
                            'context_exhausted', 'handed_off', 'terminal')",
                )
                .bind(&interrupted_state)
                .bind(conv_state_kind(&ConvState::Failed {
                    error: "Sub-agent interrupted by server restart".to_string(),
                    error_kind: phoenix_core::domain::db_schema::ErrorKind::SubAgentError,
                }))
                .bind(now.to_rfc3339())
                .bind(&agent.agent_id)
                .execute(&self.pool)
                .await?;
            }

            tracing::info!(
                conv_id = %conv_id,
                completed = completed_results.len(),
                interrupted = tool_use_count - completed_results.len(),
                "materialized in-flight tool round on restart",
            );
        }

        Ok(materialized)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn persist_startup_sub_agent_fan_in(
        &self,
        conversation_id: &str,
        results: &[phoenix_core::domain::sm_state::SubAgentResult],
        spawn_tool_id: Option<&str>,
        expected_state: &ConvState,
        destination: &ConvState,
        action: StartupParentAction,
        now: DateTime<Utc>,
    ) -> DbResult<()> {
        let (content, display_data) = build_sub_agent_fan_in(results);
        let mut tx = self.pool.begin().await?;
        if let Some(tool_id) = spawn_tool_id {
            let message_id = tool_result_message_id(tool_id);
            let stored_content = serde_json::to_string(
                &MessageContent::tool(tool_id, content, false).to_stored_json(),
            )
            .map_err(|error| DbError::Serialization(error.to_string()))?;
            let updated = sqlx::query(
                "UPDATE messages SET content = ?1, display_data = ?2
                 WHERE message_id = ?3 AND conversation_id = ?4",
            )
            .bind(stored_content)
            .bind(
                serde_json::to_string(&display_data)
                    .map_err(|error| DbError::Serialization(error.to_string()))?,
            )
            .bind(&message_id)
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(DbError::Serialization(format!(
                    "startup sub-agent fan-in message missing for {conversation_id}"
                )));
            }
            let updated_message = sqlx::query(
                "SELECT message_id, conversation_id, sequence_id, message_type,
                        content, display_data, usage_data, created_at
                 FROM messages WHERE message_id = ?1",
            )
            .bind(message_id)
            .try_map(parse_message_row)
            .fetch_one(&mut *tx)
            .await?;
            retrieval::fts_upsert_conn(
                &mut tx,
                &updated_message,
                retrieval::FtsObservation::ParentTransaction(
                    sqlite_telemetry::ParentSqliteObserver::UninstrumentedNested,
                ),
            )
            .await?;
        } else {
            let sequence_id: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(sequence_id), 0) + 1
                 FROM messages WHERE conversation_id = ?1",
            )
            .bind(conversation_id)
            .fetch_one(&mut *tx)
            .await?;
            let mut identity = sha2::Sha256::new();
            for result in results {
                identity.update(result.agent_id.as_bytes());
                identity.update([0]);
            }
            let round_id =
                identity
                    .finalize()
                    .iter()
                    .fold(String::with_capacity(64), |mut encoded, byte| {
                        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                        encoded
                    });
            let message = Message {
                message_id: format!("startup-sub-agent-summary:{conversation_id}:{round_id}"),
                conversation_id: conversation_id.to_string(),
                sequence_id,
                message_type: MessageType::User,
                content: MessageContent::User(UserContent::meta(&content)),
                display_data: Some(display_data),
                usage_data: None,
                created_at: now,
            };
            insert_message_tx(&mut tx, &message).await?;
        }
        let projection_update = sqlx::query(
            "UPDATE conversations
             SET state = ?1, state_kind = ?2, state_updated_at = ?3,
                 updated_at = ?3, transcript_generation = transcript_generation + 1
             WHERE id = ?4 AND state = ?5",
        )
        .bind(
            serde_json::to_string(destination)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        )
        .bind(conv_state_kind(destination))
        .bind(now.to_rfc3339())
        .bind(conversation_id)
        .bind(
            serde_json::to_string(expected_state)
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        )
        .execute(&mut *tx)
        .await?;
        if projection_update.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(DbError::Serialization(format!(
                "parent {conversation_id} changed during recovered fan-in"
            )));
        }
        sqlx::query(
            "INSERT OR REPLACE INTO startup_parent_actions
                 (conversation_id, action, transcript_generation, turn_id, turn_generation, created_at)
             SELECT c.id, ?2, c.transcript_generation, t.turn_id, t.generation, ?3
             FROM conversations AS c
             LEFT JOIN durable_turns AS t ON t.conversation_id = c.id
                 AND t.owns_conversation = 1 AND t.terminal_kind IS NULL
             WHERE c.id = ?1",
        )
        .bind(conversation_id)
        .bind(match action {
            StartupParentAction::Reconcile => "Reconcile",
            StartupParentAction::Resume => "Resume",
            StartupParentAction::Cancel => "Cancel",
        })
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Establish the exact parent authority before a child can outlive it.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable action cannot be persisted.
    pub async fn establish_parent_reconcile_action(&self, conversation_id: &str) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO startup_parent_actions
                 (conversation_id, action, transcript_generation,
                  turn_id, turn_generation, created_at)
             SELECT c.id, 'Reconcile', c.transcript_generation,
                    t.turn_id, t.generation, ?2
             FROM conversations AS c
             LEFT JOIN durable_turns AS t ON t.conversation_id = c.id
                 AND t.owns_conversation = 1 AND t.terminal_kind IS NULL
             WHERE c.id = ?1
             ON CONFLICT(conversation_id) DO NOTHING",
        )
        .bind(conversation_id)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load durable parent actions that remain executable at startup.
    ///
    /// # Errors
    ///
    /// Returns an error when stale-action cleanup or action decoding fails.
    pub async fn list_startup_parent_actions(&self) -> DbResult<Vec<StartupParentActionRecord>> {
        sqlx::query(
            "DELETE FROM startup_parent_actions
             WHERE action = 'Resume' AND EXISTS (
                 SELECT 1 FROM conversations AS c
                 WHERE c.id = startup_parent_actions.conversation_id
                   AND (c.transcript_generation != startup_parent_actions.transcript_generation
                       OR EXISTS (
                           SELECT 1 FROM durable_turns AS t
                           WHERE t.turn_id = startup_parent_actions.turn_id
                             AND t.terminal_kind IS NOT NULL
                       ))
             )",
        )
        .execute(&self.pool)
        .await?;
        let rows = sqlx::query(
            "SELECT a.action_id, a.conversation_id, a.action, a.transcript_generation, a.created_at, a.turn_id, a.turn_generation
             FROM startup_parent_actions AS a
             JOIN conversations AS c ON c.id = a.conversation_id
             WHERE a.action IN ('Reconcile', 'Cancel')
                OR (a.action = 'Resume'
                    AND c.transcript_generation = a.transcript_generation)
             ORDER BY a.conversation_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let action: String = row.try_get("action")?;
                let action = match action.as_str() {
                    "Reconcile" => StartupParentAction::Reconcile,
                    "Resume" => StartupParentAction::Resume,
                    "Cancel" => StartupParentAction::Cancel,
                    value => {
                        return Err(DbError::Serialization(format!(
                            "unknown startup parent action {value}"
                        )))
                    }
                };
                Ok(StartupParentActionRecord {
                    action_id: row.try_get("action_id")?,
                    conversation_id: row.try_get("conversation_id")?,
                    action,
                    transcript_generation: row.try_get("transcript_generation")?,
                    created_at: row.try_get("created_at")?,
                    turn_id: row.try_get::<Option<i64>, _>("turn_id")?.map(|id| {
                        phoenix_workflow::TurnAuthorityId(u64::try_from(id).unwrap_or(0))
                    }),
                    turn_generation: row
                        .try_get::<Option<i64>, _>("turn_generation")?
                        .map(|generation| u64::try_from(generation).unwrap_or(0)),
                })
            })
            .collect()
    }

    /// Retire a durably completed parent action.
    ///
    /// # Errors
    ///
    /// Returns an error when the action cannot be deleted.
    pub async fn delete_startup_parent_action(
        &self,
        conversation_id: &str,
        action_id: i64,
    ) -> DbResult<()> {
        sqlx::query(
            "DELETE FROM startup_parent_actions
             WHERE conversation_id = ?1 AND action_id = ?2",
        )
        .bind(conversation_id)
        .bind(action_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reconcile parents after process startup, when no pre-restart runtime can
    /// still own an in-flight child or tool result.
    ///
    /// # Errors
    ///
    /// Returns an error when parent or child durable state cannot be read or
    /// atomically persisted.
    pub async fn reconcile_startup_obligated_parents(
        &self,
        conversation_ids: &std::collections::HashSet<String>,
    ) -> DbResult<Vec<StartupParentReconciliation>> {
        self.reconcile_startup_parents(conversation_ids).await
    }

    /// Materialize parent progress that was deliberately preserved while a
    /// child terminal obligation still owned the exact outcome.
    ///
    /// # Errors
    ///
    /// Returns an error when any parent transcript or state cannot be read or
    /// atomically persisted.
    #[allow(clippy::too_many_lines)]
    async fn reconcile_startup_parents(
        &self,
        conversation_ids: &std::collections::HashSet<String>,
    ) -> DbResult<Vec<StartupParentReconciliation>> {
        let now = Utc::now();
        let _ = self
            .materialize_in_flight_tool_rounds(&now, Some(conversation_ids))
            .await?;
        let idle_json = serde_json::to_string(&ConvState::Idle)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let mut reconciled = Vec::new();
        for conversation_id in conversation_ids {
            let conversation = match self.get_conversation(conversation_id).await {
                Ok(conversation) => conversation,
                Err(DbError::ConversationNotFound(_)) => continue,
                Err(error) => return Err(error),
            };
            let pending_fan_in = match conversation.state.clone() {
                ConvState::AwaitingSubAgents {
                    pending,
                    completed_results,
                    spawn_tool_id,
                } => Some((
                    pending,
                    completed_results,
                    spawn_tool_id,
                    None,
                    ConvState::LlmRequesting { attempt: 1 },
                )),
                ConvState::CancellingSubAgents {
                    pending,
                    completed_results,
                    cause,
                    spawn_tool_id,
                } => Some((
                    pending,
                    completed_results,
                    spawn_tool_id,
                    Some(cause),
                    match cause {
                        phoenix_core::domain::sm_event::CancelCause::Timeout => {
                            ConvState::LlmRequesting { attempt: 1 }
                        }
                        phoenix_core::domain::sm_event::CancelCause::UserRequested => {
                            ConvState::Idle
                        }
                    },
                )),
                ConvState::Idle
                | ConvState::LlmRequesting { .. }
                | ConvState::SeededLlmRequesting { .. }
                | ConvState::Provisioning { .. }
                | ConvState::CreationCancelled { .. }
                | ConvState::ToolExecuting { .. }
                | ConvState::CancellingTool { .. }
                | ConvState::Completed { .. }
                | ConvState::Failed { .. }
                | ConvState::CreationFailed { .. }
                | ConvState::Error { .. }
                | ConvState::AwaitingRecovery { .. }
                | ConvState::AwaitingContinuation { .. }
                | ConvState::RecoverableContinuationFailure { .. }
                | ConvState::AwaitingTaskApproval { .. }
                | ConvState::AwaitingUserResponse { .. }
                | ConvState::ContextExhausted { .. }
                | ConvState::HandedOff { .. }
                | ConvState::Terminal => None,
            };
            if let Some((
                pending,
                mut completed_results,
                spawn_tool_id,
                cancel_cause,
                destination,
            )) = pending_fan_in
            {
                let outcomes = self.resolve_pending_sub_agent_outcomes(&pending).await?;
                for agent in pending {
                    let outcome = if let Some(outcome) = outcomes.get(&agent.agent_id).cloned() {
                        outcome
                    } else {
                        let interrupted = ConvState::Failed {
                            error: "Sub-agent interrupted by server restart".to_string(),
                            error_kind: phoenix_core::domain::db_schema::ErrorKind::SubAgentError,
                        };
                        self.update_conversation_state(&agent.agent_id, &interrupted)
                            .await?;
                        phoenix_core::domain::sm_state::SubAgentOutcome::Failure {
                            error: "Sub-agent interrupted by server restart".to_string(),
                            error_kind: phoenix_core::domain::db_schema::ErrorKind::SubAgentError,
                        }
                    };
                    let outcome = match (cancel_cause, outcome) {
                        (Some(phoenix_core::domain::sm_event::CancelCause::Timeout), _) => {
                            phoenix_core::domain::sm_state::SubAgentOutcome::TimedOut
                        }
                        (_, outcome) => outcome,
                    };
                    completed_results.push(phoenix_core::domain::sm_state::SubAgentResult {
                        agent_id: agent.agent_id,
                        task: agent.task,
                        outcome,
                    });
                }
                self.persist_startup_sub_agent_fan_in(
                    conversation_id,
                    &completed_results,
                    spawn_tool_id.as_deref(),
                    &conversation.state,
                    &destination,
                    if matches!(
                        cancel_cause,
                        Some(phoenix_core::domain::sm_event::CancelCause::UserRequested)
                    ) {
                        StartupParentAction::Cancel
                    } else {
                        StartupParentAction::Resume
                    },
                    now,
                )
                .await?;
                reconciled.push(StartupParentReconciliation {
                    conversation_id: conversation_id.clone(),
                });
                continue;
            }
            if matches!(
                conversation.state,
                ConvState::Idle | ConvState::LlmRequesting { .. }
            ) {
                let is_parent: i64 = sqlx::query_scalar(
                    "SELECT EXISTS(
                         SELECT 1 FROM conversations AS child
                         WHERE child.parent_conversation_id = ?1
                     )",
                )
                .bind(conversation_id)
                .fetch_one(&self.pool)
                .await?;
                if is_parent == 1 {
                    if matches!(conversation.state, ConvState::LlmRequesting { .. }) {
                        sqlx::query(
                            "INSERT OR REPLACE INTO startup_parent_actions
                                 (conversation_id, action, transcript_generation,
                                  turn_id, turn_generation, created_at)
                             SELECT c.id, 'Resume', c.transcript_generation,
                                    a.turn_id, a.turn_generation, ?3
                             FROM conversations AS c
                             JOIN startup_parent_actions AS a ON a.conversation_id = c.id
                             WHERE c.id = ?1 AND a.action = 'Reconcile'
                               AND (
                                   (a.turn_id IS NULL AND NOT EXISTS (
                                       SELECT 1 FROM durable_turns
                                       WHERE conversation_id = ?1 AND owns_conversation = 1
                                         AND terminal_kind IS NULL
                                   ))
                                   OR EXISTS (
                                       SELECT 1 FROM durable_turns
                                       WHERE turn_id = a.turn_id
                                         AND generation = a.turn_generation
                                         AND owns_conversation = 1 AND terminal_kind IS NULL
                                   )
                               )",
                        )
                        .bind(conversation_id)
                        .bind(conversation.transcript_generation)
                        .bind(now.to_rfc3339())
                        .execute(&self.pool)
                        .await?;
                    }
                    reconciled.push(StartupParentReconciliation {
                        conversation_id: conversation_id.clone(),
                    });
                    continue;
                }
            }
            let mut tx = self.pool.begin().await?;
            let updated = sqlx::query(
                "UPDATE conversations
                 SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?3
                 WHERE id = ?4 AND state_kind IN ('tool_executing', 'cancelling_tool')
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations AS child
                       JOIN durable_turns AS t ON t.conversation_id = child.id
                       JOIN direct_turn_terminal_obligations AS o ON o.turn_id = t.turn_id
                       WHERE child.parent_conversation_id = ?4
                   )",
            )
            .bind(&idle_json)
            .bind(conv_state_kind(&ConvState::Idle))
            .bind(now.to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() == 1 {
                sqlx::query(
                    "INSERT OR REPLACE INTO startup_parent_actions
                         (conversation_id, action, transcript_generation, turn_id, turn_generation, created_at)
                     SELECT c.id, 'Resume', c.transcript_generation, t.turn_id, t.generation, ?2
                     FROM conversations AS c
                     LEFT JOIN durable_turns AS t ON t.conversation_id = c.id
                         AND t.owns_conversation = 1 AND t.terminal_kind IS NULL
                     WHERE c.id = ?1",
                )
                .bind(conversation_id)
                .bind(now.to_rfc3339())
                .execute(&mut *tx)
                .await?;
                reconciled.push(StartupParentReconciliation {
                    conversation_id: conversation_id.clone(),
                });
            }
            tx.commit().await?;
        }
        Ok(reconciled)
    }

    /// Allocate the next `sequence_id` for a conversation from the message
    /// watermark (`MAX(sequence_id) + 1`). Used by the restart materialization
    /// path, which has no live `SseBroadcaster` counter to draw from.
    async fn next_sequence_id(&self, conversation_id: &str) -> DbResult<i64> {
        let row = sqlx::query(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get(0))
    }

    /// Scan all conversations for orphaned `tool_use` and inject synthetic `tool_result`.
    /// An orphaned `tool_use` is an agent message containing `tool_use` blocks where
    /// not all `tool_use` IDs have a corresponding `tool_result` in the following messages.
    ///
    /// Skips conversations in preserved (frozen) states — `context_exhausted`,
    /// `terminal`, continuation/recovery states, and approval states. Those
    /// match the allowlist in `reset_all_to_idle` (the conversation is not
    /// going to make another LLM call, so injecting a synthetic `tool_result`
    /// only adds noise to history).
    async fn repair_orphaned_tool_use(&self, now: &DateTime<Utc>) -> DbResult<()> {
        use phoenix_core::domain::llm_types::ContentBlock;

        // Skip conversations whose state is preserved across restarts; their
        // history is frozen and shouldn't be amended with synthetic results.
        let conv_rows: Vec<String> = sqlx::query(
            "SELECT c.id FROM conversations AS c
             WHERE c.state_kind NOT IN
                 ('context_exhausted', 'handed_off', 'terminal',
                  'awaiting_continuation', 'recoverable_continuation_failure',
                  'awaiting_recovery', 'awaiting_task_approval', 'awaiting_user_response')
               AND NOT EXISTS (
                   SELECT 1
                   FROM durable_turns AS t
                   JOIN direct_turn_terminal_obligations AS o ON o.turn_id = t.turn_id
                   WHERE t.disposition = 'Runtime'
                     AND (
                         t.conversation_id = c.id
                         OR t.conversation_id IN (
                             SELECT child.id FROM conversations AS child
                             WHERE child.parent_conversation_id = c.id
                         )
                     )
               )",
        )
        .try_map(|row: SqliteRow| row.try_get("id"))
        .fetch_all(&self.pool)
        .await?;

        for conv_id in conv_rows {
            // Get all messages for this conversation in order
            let messages: Vec<(String, i64, String, String)> = sqlx::query(
                "SELECT message_id, sequence_id, message_type, content
                 FROM messages WHERE conversation_id = ?1 ORDER BY sequence_id ASC",
            )
            .bind(&conv_id)
            .try_map(|row: SqliteRow| {
                Ok((
                    row.try_get("message_id")?,
                    row.try_get("sequence_id")?,
                    row.try_get("message_type")?,
                    row.try_get("content")?,
                ))
            })
            .fetch_all(&self.pool)
            .await?;

            // Find orphaned tool_use IDs
            let mut pending_tool_ids: Vec<String> = Vec::new();
            let mut max_sequence_id: i64 = 0;

            for (_, seq_id, msg_type, content) in &messages {
                max_sequence_id = *seq_id;

                if msg_type == "agent" {
                    // Parse agent content to find tool_use blocks
                    if let Ok(blocks) = serde_json::from_str::<Vec<ContentBlock>>(content) {
                        for block in blocks {
                            if let ContentBlock::ToolUse { id, .. } = block {
                                pending_tool_ids.push(id);
                            }
                        }
                    }
                } else if msg_type == "tool" {
                    // Parse tool content to find tool_use_id
                    if let Ok(tool_content) = serde_json::from_str::<ToolContent>(content) {
                        pending_tool_ids.retain(|id| id != &tool_content.tool_use_id);
                    }
                }
            }

            // Insert synthetic tool_result for any remaining orphaned tool_use
            for tool_id in pending_tool_ids {
                max_sequence_id += 1;
                let msg_id = uuid::Uuid::new_v4().to_string();
                let tool_content = ToolContent::new(
                    &tool_id,
                    "[Tool execution interrupted by server restart]",
                    true,
                );
                let content_json =
                    serde_json::to_string(&tool_content).unwrap_or_else(|_| "{}".to_string());

                sqlx::query(
                    "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at)
                     VALUES (?1, ?2, ?3, 'tool', ?4, ?5)",
                )
                .bind(&msg_id)
                .bind(&conv_id)
                .bind(max_sequence_id)
                .bind(&content_json)
                .bind(now.to_rfc3339())
                .execute(&self.pool)
                .await?;

                tracing::info!(
                    conv_id = %conv_id,
                    tool_id = %tool_id,
                    "Injected synthetic tool_result for orphaned tool_use"
                );
            }
        }

        Ok(())
    }

    // ==================== Message Operations ====================

    /// Add a message to a conversation
    ///
    /// The `message_id` is the canonical identifier for this message, typically
    /// generated by the client for user messages (enabling idempotent retries)
    /// or by the server for agent/tool messages.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn add_message(
        &self,
        message_id: &str,
        conversation_id: &str,
        content: &MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
    ) -> DbResult<Message> {
        // Allocate sequence_id from the DB watermark. Callers that also
        // broadcast the message over SSE must instead use
        // `add_message_with_seq` with a sequence pre-allocated from the
        // broadcaster's counter — see the PersistBeforeBroadcast invariant
        // in specs/sse_wire/sse_wire.allium.
        let row = sqlx::query(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?;
        let sequence_id: i64 = row.get(0);

        self.add_message_with_seq(
            message_id,
            conversation_id,
            sequence_id,
            content,
            display_data,
            usage_data,
        )
        .await
    }

    /// Persist one terminal transcript message and its exact direct-turn terminal
    /// obligation atomically before either can be observed independently after restart.
    ///
    /// # Errors
    ///
    /// Returns an error when message serialization, attachment persistence,
    /// authority validation, or transaction commit fails.
    ///
    /// # Panics
    ///
    /// Panics if typed message, display, or usage data cannot serialize.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_message_with_seq_and_terminal_obligation(
        &self,
        message_id: &str,
        conversation_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
        obligation: &workflow::DirectTurnTerminalObligationInput,
    ) -> DbResult<Message> {
        let now = Utc::now();
        let msg_type = content.message_type();
        let content_str = serde_json::to_string(&content.to_stored_json()).unwrap();
        let display_str = display_data.map(|value| serde_json::to_string(value).unwrap());
        let usage_str = usage_data.map(|usage| serde_json::to_string(usage).unwrap());
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type,
             content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(sequence_id)
        .bind(msg_type.to_string())
        .bind(&content_str)
        .bind(&display_str)
        .bind(&usage_str)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        insert_message_attachments(&mut tx, message_id, content).await?;
        workflow::WorkflowRepository::persist_terminal_obligation_tx(&mut tx, obligation).await?;
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(now.to_rfc3339())
            .bind(conversation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let message = Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: msg_type,
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: now,
        };
        if let Err(error) =
            retrieval::fts_upsert(&self.pool, &message, self.sqlite_workload_collector.clone())
                .await
        {
            tracing::warn!(
                message_id = %message.message_id,
                error = %error,
                "failed to index terminal message for retrieval; startup reconcile will repair"
            );
        }
        Ok(message)
    }

    /// Persist a message with an externally-allocated `sequence_id`.
    ///
    /// Used by the runtime executor and lifecycle handlers: the sequence
    /// is pre-allocated from `SseBroadcaster::next_seq()` *before* the DB
    /// write, so the message's own seq is strictly greater than any
    /// ephemeral event (token / `state_change` / error) broadcast earlier.
    /// This is what prevents the "message seq lower than client's
    /// `lastSequenceId` → dropped by `applyIfNewer`" failure mode behind
    /// task 02679.
    ///
    /// Formally: enforces the `PersistBeforeBroadcast` invariant in
    /// `specs/sse_wire/sse_wire.allium` at the sequence-allocation level,
    /// not just at the "DB write happens-before broadcast" level.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    pub async fn add_message_with_seq(
        &self,
        message_id: &str,
        conversation_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
    ) -> DbResult<Message> {
        let now = Utc::now();
        let msg_type = content.message_type();

        let content_str = serde_json::to_string(&content.to_stored_json()).unwrap();
        let display_str = display_data.map(|v| serde_json::to_string(v).unwrap());
        let usage_str = usage_data.map(|u| serde_json::to_string(u).unwrap());

        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(sequence_id)
        .bind(msg_type.to_string())
        .bind(&content_str)
        .bind(&display_str)
        .bind(&usage_str)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        {
            let mut conn = self.pool.acquire().await?;
            insert_message_attachments(&mut conn, message_id, content).await?;
        }

        // Update conversation timestamp
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(now.to_rfc3339())
            .bind(conversation_id)
            .execute(&self.pool)
            .await?;

        let message = Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: msg_type,
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: now,
        };
        // Index for retrieval (specs/conversation-retrieval/ REQ-RET-003).
        // The startup reconcile is the backstop if this ever fails after the
        // message row commits.
        if let Err(e) =
            retrieval::fts_upsert(&self.pool, &message, self.sqlite_workload_collector.clone())
                .await
        {
            tracing::warn!(
                message_id = %message.message_id, error = %e,
                "failed to index message for retrieval; startup reconcile will repair",
            );
        }
        Ok(message)
    }

    /// Like `add_message_with_seq`, but persists a caller-supplied
    /// `created_at` instead of `Utc::now()`. Used by `persist_checkpoint`
    /// to align the durable row's timestamp with the eager-broadcast
    /// timestamp atomically — a single INSERT, no transient
    /// `Utc::now()` value visible to a concurrent reconnect.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_message_with_seq_at(
        &self,
        message_id: &str,
        conversation_id: &str,
        sequence_id: i64,
        content: &MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
        created_at: DateTime<Utc>,
    ) -> DbResult<Message> {
        let msg_type = content.message_type();
        let content_str = serde_json::to_string(&content.to_stored_json()).unwrap();
        let display_str = display_data.map(|v| serde_json::to_string(v).unwrap());
        let usage_str = usage_data.map(|u| serde_json::to_string(u).unwrap());

        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .bind(sequence_id)
        .bind(msg_type.to_string())
        .bind(&content_str)
        .bind(&display_str)
        .bind(&usage_str)
        .bind(created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        {
            let mut conn = self.pool.acquire().await?;
            insert_message_attachments(&mut conn, message_id, content).await?;
        }

        // Mirror the `add_message_with_seq` side-effect: bump the
        // conversation's `updated_at` so list-ordering stays current.
        // Use Utc::now() here (not the message's created_at) — the
        // conversation's "last activity" is wall-clock, not the message
        // timestamp the UI displays.
        sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
            .bind(Utc::now().to_rfc3339())
            .bind(conversation_id)
            .execute(&self.pool)
            .await?;

        let message = Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: msg_type,
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at,
        };
        // Index for retrieval (specs/conversation-retrieval/ REQ-RET-003).
        if let Err(e) =
            retrieval::fts_upsert(&self.pool, &message, self.sqlite_workload_collector.clone())
                .await
        {
            tracing::warn!(
                message_id = %message.message_id, error = %e,
                "failed to index message for retrieval; startup reconcile will repair",
            );
        }
        Ok(message)
    }

    /// Get messages for a conversation
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_messages(&self, conversation_id: &str) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        hydrate_attachments(&self.pool, &mut rows).await?;
        Ok(rows)
    }

    /// Get the exact projection consumed by runtime recovery: the newest agent
    /// plus the suffix beginning at the newest user or skill boundary. If no
    /// boundary exists, returns the full pre-turn transcript.
    ///
    /// Including the newest agent even when it predates the boundary preserves
    /// `should_auto_continue` for malformed/legacy histories while avoiding all
    /// unrelated earlier messages in ordinary turn-shaped transcripts.
    ///
    /// # Errors
    ///
    /// Returns an error if the message query or attachment hydration fails.
    pub async fn get_recovery_messages(&self, conversation_id: &str) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1
               AND (
                   sequence_id >= COALESCE(
                       (SELECT MAX(sequence_id)
                        FROM messages
                        WHERE conversation_id = ?1 AND message_type IN ('user', 'skill')),
                       0
                   )
                   OR sequence_id = (
                       SELECT MAX(sequence_id)
                       FROM messages
                       WHERE conversation_id = ?1 AND message_type = 'agent'
                   )
               )
             ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        hydrate_attachments(&self.pool, &mut rows).await?;

        // Adopted wake results form a consecutive user-message tail. The newest
        // user boundary above contains only the last result, but recovery must
        // inspect the entire tail because any non-cancelled terminal requests
        // auto-resume. Expand only that semantic tail, stopping at the first
        // ordinary message rather than hydrating older transcript turns.
        if rows.last().is_some_and(is_adopted_wake_result_message) {
            let mut cursor = rows.last().map_or(0, |message| message.sequence_id);
            let mut wake_tail = Vec::new();
            'tail: loop {
                let older = self
                    .get_messages_before(conversation_id, cursor, 64)
                    .await?;
                if older.is_empty() {
                    break;
                }
                cursor = older.first().map_or(cursor, |message| message.sequence_id);
                for message in older.into_iter().rev() {
                    if !is_adopted_wake_result_message(&message) {
                        break 'tail;
                    }
                    wake_tail.push(message);
                }
            }
            rows.extend(wake_tail);
            rows.sort_by_key(|message| message.sequence_id);
            rows.dedup_by_key(|message| message.sequence_id);
        }

        Ok(rows)
    }

    /// Return the current transcript tail and its typed settlement, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence contains an unknown settlement reason.
    pub async fn get_recovery_tail_status(
        &self,
        conversation_id: &str,
    ) -> DbResult<phoenix_core::domain::db_schema::RecoveryTailStatus> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT tail.message_id, settlement.reason
             FROM messages tail
             LEFT JOIN conversation_recovery_settlements settlement
               ON settlement.conversation_id = tail.conversation_id
              AND settlement.terminal_message_id = tail.message_id
             WHERE tail.conversation_id = ?1
               AND tail.sequence_id = (
                   SELECT MAX(candidate.sequence_id)
                   FROM messages candidate
                   WHERE candidate.conversation_id = tail.conversation_id
               )",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((message_id, reason)) = row else {
            return Ok(phoenix_core::domain::db_schema::RecoveryTailStatus::Empty);
        };
        let settlement = reason
            .map(|value| {
                phoenix_core::domain::db_schema::RecoverySettlementReason::from_db_str(&value)
                    .ok_or_else(|| {
                        DbError::Serialization(format!(
                            "unknown recovery settlement reason: {value}"
                        ))
                    })
            })
            .transpose()?;
        Ok(phoenix_core::domain::db_schema::RecoveryTailStatus::Tail {
            message_id,
            settlement,
        })
    }

    /// Get messages after a sequence ID
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_messages_after(
        &self,
        conversation_id: &str,
        after_sequence: i64,
    ) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 AND sequence_id > ?2 ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .bind(after_sequence)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        hydrate_attachments(&self.pool, &mut rows).await?;
        Ok(rows)
    }

    /// Get the latest messages for a conversation, capped by `limit`.
    ///
    /// Returns messages in ascending `sequence_id` order so callers can append
    /// them directly to an in-memory transcript.
    ///
    /// # Errors
    ///
    /// Returns an error if the message query or attachment hydration fails.
    pub async fn get_latest_messages(
        &self,
        conversation_id: &str,
        limit: i64,
    ) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY sequence_id DESC
             LIMIT ?2",
        )
        .bind(conversation_id)
        .bind(limit)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        rows.reverse();
        hydrate_attachments(&self.pool, &mut rows).await?;
        Ok(rows)
    }

    /// Get the newest usage payload for a conversation.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn get_latest_usage_data(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<UsageData>> {
        let row = sqlx::query(
            "SELECT usage_data
             FROM messages
             WHERE conversation_id = ?1 AND usage_data IS NOT NULL
             ORDER BY sequence_id DESC
             LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row
            .and_then(|row| {
                row.try_get::<Option<String>, _>("usage_data")
                    .ok()
                    .flatten()
            })
            .and_then(|usage| serde_json::from_str(&usage).ok()))
    }

    /// Get messages before a sequence ID, capped by `limit`.
    ///
    /// Returns the newest `limit` messages with `sequence_id < before_sequence`,
    /// re-sorted ascending before returning.
    ///
    /// # Errors
    ///
    /// Returns an error if the message query or attachment hydration fails.
    pub async fn get_messages_before(
        &self,
        conversation_id: &str,
        before_sequence: i64,
        limit: i64,
    ) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1 AND sequence_id < ?2
             ORDER BY sequence_id DESC
             LIMIT ?3",
        )
        .bind(conversation_id)
        .bind(before_sequence)
        .bind(limit)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        rows.reverse();
        hydrate_attachments(&self.pool, &mut rows).await?;
        Ok(rows)
    }

    /// Get messages after a sequence ID, capped by `limit`.
    ///
    /// # Errors
    ///
    /// Returns an error if the message query or attachment hydration fails.
    pub async fn get_messages_after_limited(
        &self,
        conversation_id: &str,
        after_sequence: i64,
        limit: i64,
    ) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1 AND sequence_id > ?2
             ORDER BY sequence_id ASC
             LIMIT ?3",
        )
        .bind(conversation_id)
        .bind(after_sequence)
        .bind(limit)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        hydrate_attachments(&self.pool, &mut rows).await?;
        Ok(rows)
    }

    /// Get the inclusive message range `[start_sequence, end_sequence]`.
    ///
    /// # Errors
    ///
    /// Returns an error if the message query or attachment hydration fails.
    pub async fn get_message_range(
        &self,
        conversation_id: &str,
        start_sequence: i64,
        end_sequence: i64,
    ) -> DbResult<Vec<Message>> {
        let mut rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1 AND sequence_id >= ?2 AND sequence_id <= ?3
             ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .bind(start_sequence)
        .bind(end_sequence)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        hydrate_attachments(&self.pool, &mut rows).await?;
        Ok(rows)
    }

    /// Get a window of messages around `pivot_sequence`, excluding the pivot.
    ///
    /// Returns `(before, after)` with both slices in ascending order.
    ///
    /// # Errors
    ///
    /// Returns an error if either message query or attachment hydration fails.
    pub async fn get_messages_around(
        &self,
        conversation_id: &str,
        pivot_sequence: i64,
        before_limit: i64,
        after_limit: i64,
    ) -> DbResult<(Vec<Message>, Vec<Message>)> {
        let mut before = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1 AND sequence_id < ?2
             ORDER BY sequence_id DESC
             LIMIT ?3",
        )
        .bind(conversation_id)
        .bind(pivot_sequence)
        .bind(before_limit)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;
        before.reverse();

        let mut after = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE conversation_id = ?1 AND sequence_id > ?2
             ORDER BY sequence_id ASC
             LIMIT ?3",
        )
        .bind(conversation_id)
        .bind(pivot_sequence)
        .bind(after_limit)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        hydrate_attachments(&self.pool, &mut before).await?;
        hydrate_attachments(&self.pool, &mut after).await?;
        Ok((before, after))
    }

    /// Get a message by its `message_id`
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_message_by_id(&self, message_id: &str) -> DbResult<Message> {
        self.get_message_by_id_for_conversation(None, message_id)
            .await
    }

    /// Returns a message only when both its conversation and message identities match.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::MessageNotFound`] when the scoped identity is absent,
    /// or a database/decoding error when retrieval fails.
    pub async fn get_message_by_id_in_conversation(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> DbResult<Message> {
        self.get_message_by_id_for_conversation(Some(conversation_id), message_id)
            .await
    }

    async fn get_message_by_id_for_conversation(
        &self,
        conversation_id: Option<&str>,
        message_id: &str,
    ) -> DbResult<Message> {
        let mut message = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages
             WHERE message_id = ?1 AND (?2 IS NULL OR conversation_id = ?2)",
        )
        .bind(message_id)
        .bind(conversation_id)
        .try_map(parse_message_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if matches!(e, sqlx::Error::RowNotFound) {
                DbError::MessageNotFound(message_id.to_string())
            } else {
                DbError::Sqlx(e)
            }
        })?;

        hydrate_attachments(&self.pool, std::slice::from_mut(&mut message)).await?;
        Ok(message)
    }

    /// Check if a message with the given `message_id` already exists
    /// Used for idempotent message sends - returns true if duplicate
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn message_exists(&self, message_id: &str) -> DbResult<bool> {
        let row = sqlx::query("SELECT COUNT(*) FROM messages WHERE message_id = ?1")
            .bind(message_id)
            .fetch_one(&self.pool)
            .await?;
        let count: i64 = row.get(0);
        Ok(count > 0)
    }

    /// Get the last sequence ID for a conversation
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_last_sequence_id(&self, conversation_id: &str) -> DbResult<i64> {
        let row = sqlx::query(
            "SELECT COALESCE(MAX(sequence_id), 0) FROM messages WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get(0))
    }

    /// Update `display_data` for an existing message
    /// Used to enrich tool results with additional data after execution (e.g., subagent outcomes)
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_message_display_data(
        &self,
        message_id: &str,
        display_data: &serde_json::Value,
    ) -> DbResult<i64> {
        let telemetry = self.sqlite_telemetry(
            SqliteOperation::UpdateMessageDisplayData,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
        );
        let (mut connection, acquisition) = telemetry
            .observe_pool_acquisition_sqlx(self.pool.acquire())
            .await?;
        let (mut tx, timing) = telemetry
            .observe_transaction_admission_db(acquisition, async {
                Ok(connection.begin_with("BEGIN IMMEDIATE").await?)
            })
            .await?;
        let body = async {
            let mut message = sqlx::query(
                "SELECT message_id, conversation_id, sequence_id, message_type, content,
                        display_data, usage_data, created_at
                 FROM messages WHERE message_id = ?1",
            )
            .bind(message_id)
            .try_map(parse_message_row)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(mut message) = message.take() else {
                return Ok(None);
            };
            hydrate_attachments_conn(&mut tx, std::slice::from_mut(&mut message)).await?;
            message.display_data = Some(display_data.clone());
            let display_str = serde_json::to_string(display_data)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            let conversation_id: String = sqlx::query_scalar(
                "UPDATE messages
                 SET display_data = ?1
                 WHERE message_id = ?2
                 RETURNING conversation_id",
            )
            .bind(&display_str)
            .bind(message_id)
            .fetch_one(&mut *tx)
            .await?;
            let transcript_generation: i64 = sqlx::query_scalar(
                "UPDATE conversations
                 SET transcript_generation = transcript_generation + 1
                 WHERE id = ?1
                 RETURNING transcript_generation",
            )
            .bind(conversation_id)
            .fetch_one(&mut *tx)
            .await?;
            let hidden = display_data
                .get("hidden")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if hidden {
                retrieval::fts_hide_message_tx(&mut tx, &message, telemetry.parent_observer())
                    .await?;
            } else {
                retrieval::fts_index_message_tx(&mut tx, &message, telemetry.parent_observer())
                    .await?;
            }
            Ok::<_, DbError>(Some(transcript_generation))
        }
        .await;

        match body {
            Ok(Some(transcript_generation)) => {
                telemetry
                    .observe_commit_db(timing, async { Ok(tx.commit().await?) })
                    .await?;
                Ok(transcript_generation)
            }
            Ok(None) => {
                telemetry
                    .observe_failure_rollback_db(timing, async { Ok(tx.rollback().await?) })
                    .await?;
                Err(DbError::MessageNotFound(message_id.to_string()))
            }
            Err(error) => {
                telemetry
                    .observe_failure_rollback_db(timing, async { Ok(tx.rollback().await?) })
                    .await?;
                Err(error)
            }
        }
    }

    /// Update the `content` text field inside a tool result message's JSON.
    /// Used to write actual sub-agent outcomes into the `spawn_agents` tool result
    /// so that `build_llm_messages_static` feeds them to the LLM.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_tool_message_content(
        &self,
        message_id: &str,
        new_content: &str,
    ) -> DbResult<i64> {
        let mut tx = self.pool.begin().await?;
        let conversation_id: Option<String> = sqlx::query_scalar(
            "UPDATE messages
             SET content = json_set(content, '$.content', ?1)
             WHERE message_id = ?2
             RETURNING conversation_id",
        )
        .bind(new_content)
        .bind(message_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(conversation_id) = conversation_id else {
            tx.rollback().await?;
            return Err(DbError::MessageNotFound(message_id.to_string()));
        };
        let transcript_generation: i64 = sqlx::query_scalar(
            "UPDATE conversations
             SET transcript_generation = transcript_generation + 1
             WHERE id = ?1
             RETURNING transcript_generation",
        )
        .bind(conversation_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        // Re-index the mutated message so the retrieval index reflects the new
        // content (specs/conversation-retrieval/ REQ-RET-003).
        let updated: Option<Message> = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE message_id = ?1",
        )
        .bind(message_id)
        .try_map(parse_message_row)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(mut message) = updated {
            // Hydrate attachments so the re-indexed text keeps file-context tags.
            hydrate_attachments(&self.pool, std::slice::from_mut(&mut message)).await?;
            if let Err(e) =
                retrieval::fts_upsert(&self.pool, &message, self.sqlite_workload_collector.clone())
                    .await
            {
                tracing::warn!(
                    message_id = %message.message_id, error = %e,
                    "failed to index message for retrieval; startup reconcile will repair",
                );
            }
        }
        Ok(transcript_generation)
    }

    /// Inserts or replaces one finalized provider-attempt metrics row, keyed by request and retry.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot persist the metrics row.
    pub async fn upsert_llm_request_metrics(&self, metrics: &LlmAttemptMetrics) -> DbResult<()> {
        let now_str = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO llm_request_metrics (\
             request_id, retry_attempt, conversation_id, root_conversation_id, provider, model, transport, total_duration_ms, \
             dispatch_to_first_provider_event_ms, dispatch_to_first_generation_event_ms, dispatch_to_first_visible_text_ms, \
             provider_event_count, generation_event_count, visible_text_event_count, max_provider_gap_ms, max_generation_gap_ms, \
             output_kind, stream_completed, outcome, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20) \
             ON CONFLICT(request_id, retry_attempt) DO UPDATE SET \
             conversation_id = excluded.conversation_id, \
             root_conversation_id = excluded.root_conversation_id, \
             provider = excluded.provider, \
             model = excluded.model, \
             transport = excluded.transport, \
             total_duration_ms = excluded.total_duration_ms, \
             dispatch_to_first_provider_event_ms = excluded.dispatch_to_first_provider_event_ms, \
             dispatch_to_first_generation_event_ms = excluded.dispatch_to_first_generation_event_ms, \
             dispatch_to_first_visible_text_ms = excluded.dispatch_to_first_visible_text_ms, \
             provider_event_count = excluded.provider_event_count, \
             generation_event_count = excluded.generation_event_count, \
             visible_text_event_count = excluded.visible_text_event_count, \
             max_provider_gap_ms = excluded.max_provider_gap_ms, \
             max_generation_gap_ms = excluded.max_generation_gap_ms, \
             output_kind = excluded.output_kind, \
             stream_completed = excluded.stream_completed, \
             outcome = excluded.outcome, \
             created_at = excluded.created_at"
        )
        .bind(&metrics.request_id)
        .bind(i64::from(metrics.retry_attempt))
        .bind(&metrics.conversation_id)
        .bind(&metrics.root_conversation_id)
        .bind(&metrics.provider)
        .bind(&metrics.model)
        .bind(metrics.transport.as_str())
        .bind(u64_to_i64(metrics.total_duration_ms)?)
        .bind(opt_u64_to_i64(metrics.stream.dispatch_to_first_provider_event_ms)?)
        .bind(opt_u64_to_i64(metrics.stream.dispatch_to_first_generation_event_ms)?)
        .bind(opt_u64_to_i64(metrics.stream.dispatch_to_first_visible_text_ms)?)
        .bind(i64::from(metrics.stream.provider_event_count))
        .bind(i64::from(metrics.stream.generation_event_count))
        .bind(i64::from(metrics.stream.visible_text_event_count))
        .bind(opt_u64_to_i64(metrics.stream.max_provider_gap_ms)?)
        .bind(opt_u64_to_i64(metrics.stream.max_generation_gap_ms)?)
        .bind(stream_output_kind_db(metrics.stream.output_kind))
        .bind(metrics.stream.completed)
        .bind(llm_attempt_outcome_db(&metrics.outcome))
        .bind(&now_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns all persisted attempts for a request in retry order.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read or decode the rows.
    pub async fn llm_request_metrics_for_request(
        &self,
        request_id: &str,
    ) -> DbResult<Vec<LlmRequestMetricsRow>> {
        let rows = sqlx::query(
            "SELECT request_id, retry_attempt, conversation_id, root_conversation_id, provider, model, transport, total_duration_ms, \
             dispatch_to_first_provider_event_ms, dispatch_to_first_generation_event_ms, dispatch_to_first_visible_text_ms, \
             provider_event_count, generation_event_count, visible_text_event_count, max_provider_gap_ms, max_generation_gap_ms, \
             output_kind, stream_completed, outcome, created_at \
             FROM llm_request_metrics WHERE request_id = ?1 ORDER BY retry_attempt ASC"
        )
        .bind(request_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| row_to_llm_request_metrics(&r))
            .collect()
    }

    /// Returns every provider-attempt analytics row in the requested time window.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read or decode the rows.
    pub async fn usage_recent_llm_metrics(
        &self,
        since_rfc3339: &str,
    ) -> DbResult<Vec<UsageRecentLlmMetricRow>> {
        let rows = sqlx::query(
            "SELECT request_id, retry_attempt, provider, model, transport, \
             dispatch_to_first_generation_event_ms, outcome, created_at \
             FROM llm_request_metrics \
             WHERE created_at >= ?1 \
             ORDER BY created_at DESC, request_id DESC, retry_attempt DESC",
        )
        .bind(since_rfc3339)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| row_to_usage_recent_llm_metric(&r))
            .collect()
    }

    /// Insert one row into `turn_usage` for token accounting.
    ///
    /// `root_conversation_id` is the top-level conversation that owns the work
    /// tree; for a top-level conversation it equals `conversation_id`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        effective_effort: EffectiveEffort,
        usage: &phoenix_core::domain::llm_types::Usage,
        first_byte_at: Option<DateTime<Utc>>,
    ) -> DbResult<()> {
        let now_str = Utc::now().to_rfc3339();
        let first_byte_str = first_byte_at.map(|t| t.to_rfc3339());
        sqlx::query(
            "INSERT INTO turn_usage \
             (conversation_id, root_conversation_id, model, effort_source, effort_level, \
              input_tokens, output_tokens, reasoning_tokens, cache_creation_tokens, cache_read_tokens, created_at, first_byte_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(conversation_id)
        .bind(root_conversation_id)
        .bind(model)
        .bind(effective_effort.source().as_str())
        .bind(effective_effort.level().map(ModelEffort::as_wire_name))
        .bind(usage.input_tokens.cast_signed())
        .bind(usage.output_tokens.cast_signed())
        .bind(usage.reasoning_tokens.map(u64::cast_signed))
        .bind(usage.cache_creation_tokens.cast_signed())
        .bind(usage.cache_read_tokens.cast_signed())
        .bind(&now_str)
        .bind(first_byte_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The total prompt size of the most recent turn for `conversation_id`:
    /// `input_tokens + cache_read_tokens + cache_creation_tokens` (the full
    /// context the model saw, cached portion included — the cached prefix still
    /// counts against the window). `None` when the conversation has no turns yet.
    ///
    /// Used as the stale-tool-result clearing pressure signal (REQ-STR-001): the
    /// provider's reported size is ground truth, so the trigger tracks reality
    /// instead of a re-estimate that can drift below it (omitting system prompt
    /// and tool schemas, undercounting on a well-cached turn).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_last_turn_prompt_tokens(
        &self,
        conversation_id: &str,
    ) -> DbResult<Option<i64>> {
        let row: Option<i64> = sqlx::query_scalar(
            "SELECT input_tokens + cache_read_tokens + cache_creation_tokens \
             FROM turn_usage WHERE conversation_id = ?1 \
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    ///
    /// `own` covers only rows where `conversation_id` matches; `total` covers
    /// all rows under the same root (i.e. the top-level conversation plus all
    /// its sub-agents).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Callers added in Phase 4
    pub async fn get_conversation_usage(
        &self,
        conversation_id: &str,
    ) -> DbResult<ConversationUsage> {
        // --- own ---
        let own_row = sqlx::query(
            "SELECT \
             COALESCE(SUM(input_tokens), 0) AS input_tokens, \
             COALESCE(SUM(output_tokens), 0) AS output_tokens, \
             COALESCE(SUM(cache_creation_tokens), 0) AS cache_creation_tokens, \
             COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens, \
             COUNT(*) AS turns \
             FROM turn_usage WHERE conversation_id = ?1",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?;

        let own = UsageTotals {
            input_tokens: own_row.try_get("input_tokens")?,
            output_tokens: own_row.try_get("output_tokens")?,
            cache_creation_tokens: own_row.try_get("cache_creation_tokens")?,
            cache_read_tokens: own_row.try_get("cache_read_tokens")?,
            turns: own_row.try_get("turns")?,
        };

        // --- total: find root_conversation_id, fall back to conversation_id ---
        let root_id: String = sqlx::query_scalar(
            "SELECT root_conversation_id FROM turn_usage \
             WHERE conversation_id = ?1 LIMIT 1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or_else(|| conversation_id.to_string());

        let total_row = sqlx::query(
            "SELECT \
             COALESCE(SUM(input_tokens), 0) AS input_tokens, \
             COALESCE(SUM(output_tokens), 0) AS output_tokens, \
             COALESCE(SUM(cache_creation_tokens), 0) AS cache_creation_tokens, \
             COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens, \
             COUNT(*) AS turns \
             FROM turn_usage WHERE root_conversation_id = ?1",
        )
        .bind(&root_id)
        .fetch_one(&self.pool)
        .await?;

        let total = UsageTotals {
            input_tokens: total_row.try_get("input_tokens")?,
            output_tokens: total_row.try_get("output_tokens")?,
            cache_creation_tokens: total_row.try_get("cache_creation_tokens")?,
            cache_read_tokens: total_row.try_get("cache_read_tokens")?,
            turns: total_row.try_get("turns")?,
        };

        Ok(ConversationUsage { own, total })
    }

    /// All `turn_usage` aggregated by `(UTC day, model)`, oldest day first.
    ///
    /// This single rollup feeds every aggregate the `/usage` page needs —
    /// rolling-window totals, by-model and by-provider breakdowns, the daily
    /// timeseries, and the cache-hit-rate trend. Pricing is applied per row by
    /// the API layer, so mixed-model days cost correctly.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn usage_daily_by_model(&self) -> DbResult<Vec<UsageDailyModelRow>> {
        let rows = sqlx::query(
            "SELECT date(created_at) AS day, model, \
             COALESCE(SUM(input_tokens), 0) AS input_tokens, \
             COALESCE(SUM(output_tokens), 0) AS output_tokens, \
             COALESCE(SUM(cache_creation_tokens), 0) AS cache_creation_tokens, \
             COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens, \
             COUNT(*) AS turns \
             FROM turn_usage GROUP BY day, model ORDER BY day ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(UsageDailyModelRow {
                    day: r.try_get("day")?,
                    model: r.try_get("model")?,
                    input_tokens: r.try_get("input_tokens")?,
                    output_tokens: r.try_get("output_tokens")?,
                    cache_creation_tokens: r.try_get("cache_creation_tokens")?,
                    cache_read_tokens: r.try_get("cache_read_tokens")?,
                    turns: r.try_get("turns")?,
                })
            })
            .collect()
    }

    /// `turn_usage` aggregated by `(root conversation, model)`, joined to the
    /// conversation's display metadata. Sub-agent turns roll into their root
    /// (matching the `total` scope of [`Self::get_conversation_usage`]); a
    /// mixed-model conversation yields one row per model.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn usage_by_conversation(&self) -> DbResult<Vec<UsageConversationModelRow>> {
        let rows = sqlx::query(
            "SELECT tu.root_conversation_id AS rid, tu.model AS model, \
             c.slug AS slug, c.title AS title, c.project_id AS project_id, \
             e.worktree_path AS worktree_path, MIN(tu.created_at) AS started_at, \
             COALESCE(SUM(tu.input_tokens), 0) AS input_tokens, \
             COALESCE(SUM(tu.output_tokens), 0) AS output_tokens, \
             COALESCE(SUM(tu.cache_creation_tokens), 0) AS cache_creation_tokens, \
             COALESCE(SUM(tu.cache_read_tokens), 0) AS cache_read_tokens, \
             COUNT(*) AS turns \
             FROM turn_usage tu \
             LEFT JOIN conversations c ON c.id = tu.root_conversation_id \
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id \
             GROUP BY tu.root_conversation_id, tu.model",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(UsageConversationModelRow {
                    root_conversation_id: r.try_get("rid")?,
                    model: r.try_get("model")?,
                    slug: r.try_get("slug").ok().flatten(),
                    title: r.try_get("title").ok().flatten(),
                    project_id: r.try_get("project_id").ok().flatten(),
                    worktree_path: r.try_get("worktree_path").ok().flatten(),
                    started_at: r.try_get("started_at")?,
                    input_tokens: r.try_get("input_tokens")?,
                    output_tokens: r.try_get("output_tokens")?,
                    cache_creation_tokens: r.try_get("cache_creation_tokens")?,
                    cache_read_tokens: r.try_get("cache_read_tokens")?,
                    turns: r.try_get("turns")?,
                })
            })
            .collect()
    }

    /// Per-turn total token counts (input + output + cache) across all of
    /// `turn_usage`, for the tokens-per-turn distribution. One element per
    /// turn; the API layer buckets them into a histogram.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn usage_turn_token_totals(&self) -> DbResult<Vec<i64>> {
        let rows = sqlx::query(
            "SELECT (input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens) \
             AS total FROM turn_usage",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(|r| Ok(r.try_get("total")?)).collect()
    }

    /// Every `turn_usage` row under one root conversation, oldest first, for the
    /// per-conversation drill-down. `root_id` is matched against
    /// `root_conversation_id`, so sub-agent turns are included.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn usage_conversation_turns(&self, root_id: &str) -> DbResult<Vec<UsageTurnRow>> {
        let rows = sqlx::query(
            "SELECT id, conversation_id, root_conversation_id, model, created_at, first_byte_at, \
             input_tokens, output_tokens, reasoning_tokens, effort_source, effort_level, cache_creation_tokens, cache_read_tokens \
             FROM turn_usage WHERE root_conversation_id = ?1 ORDER BY created_at ASC",
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(UsageTurnRow {
                    id: r.try_get("id")?,
                    conversation_id: r.try_get("conversation_id")?,
                    root_conversation_id: r.try_get("root_conversation_id")?,
                    model: r.try_get("model")?,
                    created_at: r.try_get("created_at")?,
                    first_byte_at: r.try_get("first_byte_at")?,
                    input_tokens: r.try_get("input_tokens")?,
                    output_tokens: r.try_get("output_tokens")?,
                    cache_creation_tokens: r.try_get("cache_creation_tokens")?,
                    reasoning_tokens: r.try_get("reasoning_tokens")?,
                    effort_source: EffortSource::from_str(r.try_get::<&str, _>("effort_source")?)
                        .map_err(|error| sqlx::Error::Decode(error.into()))?,
                    effort_level: r
                        .try_get::<Option<String>, _>("effort_level")?
                        .map(|value| {
                            ModelEffort::from_str(&value)
                                .map_err(|error| sqlx::Error::Decode(error.into()))
                        })
                        .transpose()?,
                    cache_read_tokens: r.try_get("cache_read_tokens")?,
                })
            })
            .collect()
    }
    /// Conversation ids that belong to one analytics session/root conversation.
    /// Includes the root id even when it has no token rows yet.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn analytics_conversation_ids_for_root(
        &self,
        root_id: &str,
    ) -> DbResult<Vec<String>> {
        let mut ids: Vec<String> = sqlx::query_scalar(
            "WITH RECURSIVE session_conversations(id) AS (\
                 SELECT ?1 \
                 UNION \
                 SELECT c.id FROM conversations c \
                 JOIN session_conversations sc ON c.parent_conversation_id = sc.id \
             ) \
             SELECT id FROM session_conversations ORDER BY id ASC",
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await?;
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// Timestamp-only non-agent message anchors for conversations in one root
    /// session. Avoids hydrating message content/attachments for usage latency.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn usage_anchor_messages(&self, root_id: &str) -> DbResult<Vec<UsageAnchorRow>> {
        let rows = sqlx::query(
            "WITH RECURSIVE session_conversations(id) AS (\
                 SELECT ?1 \
                 UNION \
                 SELECT c.id FROM conversations c \
                 JOIN session_conversations sc ON c.parent_conversation_id = sc.id \
             ) \
             SELECT m.conversation_id, m.created_at \
             FROM messages m \
             JOIN session_conversations sc ON sc.id = m.conversation_id \
             WHERE m.message_type != 'agent' \
             ORDER BY m.conversation_id ASC, m.created_at ASC, m.sequence_id ASC",
        )
        .bind(root_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(UsageAnchorRow {
                    conversation_id: r.try_get("conversation_id")?,
                    created_at: r.try_get("created_at")?,
                })
            })
            .collect()
    }
}

/// The `cm_*` column values projected from a [`ConvMode`]. `kind` is the
/// discriminator; the rest are the variant's fields (NULL where the variant
/// does not carry them). The `ConvMode` enum is the sole writer, so only valid
/// per-variant combinations are ever produced.
struct ConvModeCols<'a> {
    kind: &'static str,
    branch_name: Option<&'a str>,
    worktree_path: Option<&'a str>,
    base_branch: Option<&'a str>,
    task_id: Option<&'a str>,
    task_title: Option<&'a str>,
    next_taskmd_id_hint: Option<&'a str>,
}

/// Project a [`ConvMode`] into its persisted `cm_*` column values.
fn conv_mode_columns(mode: &ConvMode) -> ConvModeCols<'_> {
    match mode {
        ConvMode::Explore {
            worktree_path,
            next_taskmd_id_hint,
        } => ConvModeCols {
            kind: "explore",
            branch_name: None,
            worktree_path: worktree_path.as_ref().map(NonEmptyString::as_str),
            base_branch: None,
            task_id: None,
            task_title: None,
            next_taskmd_id_hint: next_taskmd_id_hint.as_ref().map(NonEmptyString::as_str),
        },
        ConvMode::Direct => ConvModeCols {
            kind: "direct",
            branch_name: None,
            worktree_path: None,
            base_branch: None,
            task_id: None,
            task_title: None,
            next_taskmd_id_hint: None,
        },
        ConvMode::Work {
            branch_name,
            worktree_path,
            base_branch,
            task_id,
            task_title,
        } => ConvModeCols {
            kind: "work",
            branch_name: Some(branch_name.as_str()),
            worktree_path: Some(worktree_path.as_str()),
            base_branch: Some(base_branch.as_str()),
            task_id: Some(task_id.as_str()),
            task_title: Some(task_title.as_str()),
            next_taskmd_id_hint: None,
        },
        ConvMode::Branch {
            branch_name,
            worktree_path,
            base_branch,
        } => ConvModeCols {
            kind: "branch",
            branch_name: Some(branch_name.as_str()),
            worktree_path: Some(worktree_path.as_str()),
            base_branch: Some(base_branch.as_str()),
            task_id: None,
            task_title: None,
            next_taskmd_id_hint: None,
        },
    }
}

/// Reconstruct a [`ConvMode`] from a conversation row's `cm_*` columns. An
/// unknown/NULL `cm_kind`, or a Work/Branch row missing a required field
/// (structurally impossible from the `ConvMode` writer, but defended for
/// hand-edited rows), falls back to the default `Explore` with a warning —
/// mirroring the prior tolerant blob-deserialization behavior.
fn conv_mode_from_row(row: &SqliteRow, conv_id: &str) -> ConvMode {
    let col = |c: &str| row.try_get::<Option<String>, _>(c).ok().flatten();
    let ne = |c: &str| col(c).and_then(|v| NonEmptyString::new(v).ok());
    let env = |c: &str| row.try_get::<Option<String>, _>(c).ok().flatten();
    let ne_env = |c: &str| env(c).and_then(|v| NonEmptyString::new(v).ok());
    match col("cm_kind").as_deref() {
        Some("direct") => ConvMode::Direct,
        Some("work") => {
            if let (
                Some(branch_name),
                Some(worktree_path),
                Some(base_branch),
                Some(task_id),
                Some(task_title),
            ) = (
                ne_env("env_branch_name"),
                ne_env("env_worktree_path"),
                ne_env("env_base_branch"),
                ne("cm_task_id"),
                ne("cm_task_title"),
            ) {
                ConvMode::Work {
                    branch_name,
                    worktree_path,
                    base_branch,
                    task_id,
                    task_title,
                }
            } else {
                tracing::warn!(conv_id = %conv_id, "work conv_mode row missing required fields, defaulting to Explore");
                ConvMode::default()
            }
        }
        Some("branch") => {
            if let (Some(branch_name), Some(worktree_path), Some(base_branch)) = (
                ne_env("env_branch_name"),
                ne_env("env_worktree_path"),
                ne_env("env_base_branch"),
            ) {
                ConvMode::Branch {
                    branch_name,
                    worktree_path,
                    base_branch,
                }
            } else {
                tracing::warn!(conv_id = %conv_id, "branch conv_mode row missing required fields, defaulting to Explore");
                ConvMode::default()
            }
        }
        Some("explore") => ConvMode::Explore {
            worktree_path: ne_env("env_worktree_path"),
            next_taskmd_id_hint: ne("cm_next_taskmd_id_hint"),
        },
        // An unknown kind or NULL (legacy/malformed) → bare default Explore,
        // discarding any stray field columns. This mirrors the prior blob
        // parser, which fell back to ConvMode::default() for unrecognized modes
        // rather than carrying their fields forward.
        _ => ConvMode::default(),
    }
}

/// Parse a conversation row from the database
#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_conversation_row(row: SqliteRow) -> Result<Conversation, sqlx::Error> {
    let id: String = row.try_get("id")?;
    let product_conversation_id = row
        .try_get::<String, _>("product_conversation_id")?
        .parse::<phoenix_core::domain::product_conversation::ProductConversationId>()
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;

    let state_json: String = row.try_get("state")?;
    let state: ConvState = match serde_json::from_str(&state_json) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(conv_id = %id, error = %e, raw = %state_json, "Failed to deserialize ConvState, defaulting to Idle");
            ConvState::Idle
        }
    };

    // conv_mode: reconstruct from the normalized cm_* columns.
    let conv_mode: ConvMode = conv_mode_from_row(&row, &id);

    let slug: Option<String> = row.try_get("slug")?;
    let title: Option<String> = row
        .try_get::<Option<String>, _>("title")
        .unwrap_or(None)
        .or_else(|| slug.as_deref().map(schema::title_from_slug));

    let desired_base_branch: Option<String> = row
        .try_get::<Option<String>, _>("desired_base_branch")
        .unwrap_or(None);

    let seed_parent_id: Option<String> = row
        .try_get::<Option<String>, _>("seed_parent_id")
        .unwrap_or(None);
    let seed_label: Option<String> = row
        .try_get::<Option<String>, _>("seed_label")
        .unwrap_or(None);
    let continued_in_conv_id: Option<String> = row
        .try_get::<Option<String>, _>("continued_in_conv_id")
        .unwrap_or(None);
    let chain_name: Option<String> = row
        .try_get::<Option<String>, _>("chain_name")
        .unwrap_or(None);
    let spawned_from_conversation_id: Option<String> = row
        .try_get::<Option<String>, _>("spawned_from_conversation_id")
        .unwrap_or(None);

    let llm_language = row
        .try_get::<Option<String>, _>("llm_language")
        .unwrap_or(None)
        .as_deref()
        .map(phoenix_core::llm_language::LlmLanguage::parse_or_default)
        .unwrap_or_default();
    let runtime_role = row
        .try_get::<Option<String>, _>("runtime_role")
        .unwrap_or(None)
        .as_deref()
        .and_then(phoenix_core::work_scope::RuntimeRole::from_db_str)
        .unwrap_or_default();
    let work_scope_id = row
        .try_get::<Option<String>, _>("work_scope_id")
        .unwrap_or(None)
        .and_then(|raw| phoenix_core::work_scope::WorkScopeId::parse(raw).ok());

    Ok(Conversation {
        id,
        product_conversation_id,
        slug,
        title,
        cwd: row.try_get("cwd")?,
        parent_conversation_id: row.try_get("parent_conversation_id")?,
        user_initiated: row.try_get("user_initiated")?,
        state,
        state_updated_at: parse_datetime(&row.try_get::<String, _>("state_updated_at")?),
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
        updated_at: parse_datetime(&row.try_get::<String, _>("updated_at")?),
        archived: row.try_get("archived")?,
        model: row.try_get("model")?,
        effort: row
            .try_get::<Option<String>, _>("effort")
            .unwrap_or(None)
            .map(|value| {
                ModelEffort::from_str(&value).map_err(|error| sqlx::Error::Decode(error.into()))
            })
            .transpose()?,
        service_tier: row
            .try_get::<Option<String>, _>("service_tier")
            .unwrap_or(None)
            .map_or(Ok(ServiceTier::Standard), |value| {
                ServiceTier::from_str(&value).map_err(|error| sqlx::Error::Decode(error.into()))
            })?,
        project_id: row
            .try_get::<Option<String>, _>("project_id")
            .unwrap_or(None),
        conv_mode,
        runtime_role,
        attached_work_scope_id: work_scope_id,
        desired_base_branch,
        message_count: row.try_get("message_count")?,
        transcript_generation: row
            .try_get::<Option<i64>, _>("transcript_generation")
            .unwrap_or(Some(1))
            .unwrap_or(1),
        seed_parent_id,
        seed_label,
        continued_in_conv_id,
        chain_name,
        llm_language,
        spawned_from_conversation_id,
    })
}

fn normalize_hidden_git_repository_path(raw: &str) -> DbResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DbError::Serialization(
            "hidden git repository path must not be empty".to_string(),
        ));
    }
    let path = std::path::Path::new(trimmed);
    if !path.is_absolute() {
        return Err(DbError::Serialization(format!(
            "hidden git repository path must be absolute: {trimmed}"
        )));
    }
    let normalized = if trimmed == "/" {
        "/".to_string()
    } else {
        trimmed.trim_end_matches('/').to_string()
    };
    Ok(normalized)
}

fn datetime_to_unix_micros(value: chrono::DateTime<Utc>) -> i64 {
    value.timestamp_micros()
}

fn unix_micros_to_datetime(value: i64, field: &str) -> DbResult<chrono::DateTime<Utc>> {
    chrono::DateTime::<Utc>::from_timestamp_micros(value)
        .ok_or_else(|| DbError::Serialization(format!("invalid {field}: {value}")))
}

fn claim_generation_i64(claim: &CreationClaim) -> DbResult<i64> {
    i64::try_from(claim.generation).map_err(|_| {
        DbError::Serialization("creation generation exceeds SQLite integer".to_string())
    })
}

fn creation_stage_db_str(stage: CreationStage) -> &'static str {
    match stage {
        CreationStage::ValidateIntent => "validate_intent",
        CreationStage::ResolveRepository => "resolve_repository",
        CreationStage::ReserveResources => "reserve_resources",
        CreationStage::MaterializeWorktree => "materialize_worktree",
        CreationStage::FinalizeAttachments => "finalize_attachments",
        CreationStage::ExpandInitialMessage => "expand_initial_message",
        CreationStage::CommitMetadata => "commit_metadata",
        CreationStage::BootstrapInitialTurn => "bootstrap_initial_turn",
        CreationStage::Finalize => "finalize",
    }
}

fn creation_stage_from_db(value: &str) -> Result<CreationStage, sqlx::Error> {
    Ok(match value {
        "validate_intent" => CreationStage::ValidateIntent,
        "resolve_repository" => CreationStage::ResolveRepository,
        "reserve_resources" => CreationStage::ReserveResources,
        "materialize_worktree" => CreationStage::MaterializeWorktree,
        "finalize_attachments" => CreationStage::FinalizeAttachments,
        "expand_initial_message" => CreationStage::ExpandInitialMessage,
        "commit_metadata" => CreationStage::CommitMetadata,
        "bootstrap_initial_turn" => CreationStage::BootstrapInitialTurn,
        "finalize" => CreationStage::Finalize,
        _ => {
            return Err(sqlx::Error::Decode(
                format!("unknown conversation creation stage: {value:?}").into(),
            ));
        }
    })
}

fn creation_time(value: &str) -> u64 {
    u64::try_from(parse_datetime(value).timestamp_millis()).unwrap_or(0)
}

/// Parse a `conversation_creation_jobs` row from the database.
#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_conversation_creation_job_row(
    row: SqliteRow,
) -> Result<ConversationCreationJob, sqlx::Error> {
    let status_str: String = row.try_get("status")?;
    let stage = creation_stage_from_db(&row.try_get::<String, _>("stage")?)?;
    let generation_i64: i64 = row.try_get("generation")?;
    let generation = u64::try_from(generation_i64).map_err(|_| {
        sqlx::Error::Decode(format!("negative creation generation: {generation_i64}").into())
    })?;
    let status = match status_str.as_str() {
        "accepted" => CreationStatus::Accepted,
        "claimed" => CreationStatus::Claimed(CreationClaim {
            worker_id: CreationWorkerId(row.try_get("claim_worker_id")?),
            generation,
            token: CreationClaimToken(row.try_get("claim_token")?),
            lease_until: creation_time(&row.try_get::<String, _>("lease_until")?),
        }),
        "retry_scheduled" => CreationStatus::RetryScheduled {
            next_attempt_at: creation_time(&row.try_get::<String, _>("next_attempt_at")?),
            last_error: CreationError {
                kind: "transient".to_string(),
                message: row
                    .try_get::<Option<String>, _>("error")?
                    .unwrap_or_default(),
            },
        },
        "cancelling" => CreationStatus::Cancelling,
        "cancelled" => CreationStatus::Cancelled,
        "deletion_pending" => CreationStatus::DeletionPending,
        "ready" => CreationStatus::Ready,
        "failed" => CreationStatus::Failed(CreationError {
            kind: "permanent".to_string(),
            message: row
                .try_get::<Option<String>, _>("error")?
                .unwrap_or_default(),
        }),
        _ => {
            return Err(sqlx::Error::Decode(
                format!("unknown conversation creation status: {status_str:?}").into(),
            ));
        }
    };

    let intent_json: String = row.try_get("intent_json")?;
    let intent = serde_json::from_str::<ConversationCreationIntent>(&intent_json).map_err(|e| {
        sqlx::Error::Decode(format!("invalid conversation_creation_jobs.intent_json: {e}").into())
    })?;

    Ok(ConversationCreationJob {
        id: row.try_get("id")?,
        conversation_id: row.try_get("conversation_id")?,
        message_id: row.try_get("message_id")?,
        protocol: CreationProtocolState {
            kind: match &row.try_get::<Option<String>, _>("message_id")? {
                Some(message_id) => CreationKind::InitialTurn {
                    message_id: message_id.clone(),
                },
                None => CreationKind::SeededEmpty,
            },
            status,
            stage,
            attempt: u32::try_from(row.try_get::<i64, _>("attempt")?)
                .map_err(|_| sqlx::Error::Decode("invalid creation attempt".into()))?,
            generation,
        },
        intent,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
        updated_at: parse_datetime(&row.try_get::<String, _>("updated_at")?),
        accepted_at: row
            .try_get::<Option<String>, _>("accepted_at")?
            .as_deref()
            .map(parse_datetime),
        provisioning_started_at: row
            .try_get::<Option<String>, _>("provisioning_started_at")?
            .as_deref()
            .map(parse_datetime),
        completed_at: row
            .try_get::<Option<String>, _>("completed_at")?
            .as_deref()
            .map(parse_datetime),
        failed_at: row
            .try_get::<Option<String>, _>("failed_at")?
            .as_deref()
            .map(parse_datetime),
        error: row.try_get("error")?,
    })
}

/// Parse a `chain_qa` row from the database (REQ-CHN-005).
///
/// Unknown `status` values are surfaced as typed errors rather than silently
/// coerced — `ChainQaStatus::from_db_str` is the single source of truth for
/// the column's allowed alphabet.
#[allow(clippy::needless_pass_by_value, dead_code)] // dead_code: caller is `list_chain_qa`, itself dead-allowed until Phase 4
fn parse_chain_qa_row(row: SqliteRow) -> Result<ChainQaRow, sqlx::Error> {
    let status_str: String = row.try_get("status")?;
    let status = ChainQaStatus::from_db_str(&status_str).ok_or_else(|| {
        sqlx::Error::Decode(format!("unknown chain_qa.status value: {status_str:?}").into())
    })?;
    let completed_at: Option<String> = row.try_get("completed_at")?;
    Ok(ChainQaRow {
        id: row.try_get("id")?,
        root_conv_id: row.try_get("root_conv_id")?,
        question: row.try_get("question")?,
        answer: row.try_get("answer")?,
        model: row.try_get("model")?,
        status,
        chain_members_at_answer: row.try_get("chain_members_at_answer")?,
        chain_messages_at_answer: row.try_get("chain_messages_at_answer")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
        completed_at: completed_at.as_deref().map(parse_datetime),
    })
}

/// Single-row SELECT for a fork proposal addressed by `?1` = id.
const FORK_PROPOSAL_SELECT_COLUMNS: &str =
    "SELECT id, origin_conv_id, task_file, title, priority, body, status, \
     fork_conv_id, refinement_conv_id, created_at, resolved_at \
     FROM fork_proposals WHERE id = ?1";

/// The raw child conversation id a resolved proposal points at: `fork_conv_id`
/// for `spawned`, `refinement_conv_id` for `promoted`, `None` otherwise.
fn resolution_child_id(p: &ForkProposal) -> Option<&str> {
    match p.status {
        ForkProposalStatus::Spawned => p.fork_conversation_id.as_deref(),
        ForkProposalStatus::Promoted => p.refinement_conversation_id.as_deref(),
        ForkProposalStatus::Pending | ForkProposalStatus::Dismissed => None,
    }
}

/// Parse a fork-proposal row. Unknown `status` values surface as a typed
/// decode error rather than being silently coerced.
#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_fork_proposal_row(row: SqliteRow) -> Result<ForkProposal, sqlx::Error> {
    let status_str: String = row.try_get("status")?;
    let status = ForkProposalStatus::from_db_str(&status_str).ok_or_else(|| {
        sqlx::Error::Decode(format!("unknown fork_proposals.status value: {status_str:?}").into())
    })?;
    let resolved_at: Option<String> = row.try_get("resolved_at")?;
    Ok(ForkProposal {
        id: row.try_get("id")?,
        origin_conversation_id: row.try_get("origin_conv_id")?,
        task_file: row.try_get("task_file")?,
        title: row.try_get("title")?,
        priority: row.try_get("priority")?,
        body: row.try_get("body")?,
        status,
        fork_conversation_id: row.try_get("fork_conv_id")?,
        refinement_conversation_id: row.try_get("refinement_conv_id")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
        resolved_at: resolved_at.as_deref().map(parse_datetime),
    })
}

/// Insert a fully-formed `Conversation` row inside a transaction, writing every
/// persisted column (including `spawned_from_conversation_id`). Idempotent on
/// the PRIMARY KEY ONLY via `ON CONFLICT(id) DO NOTHING`, so a crash-retry that
/// Insert one steering entry plus its attachments into the normalized tables,
/// inside a transaction. The `skill_*` columns are written as an all-or-nothing
/// trio (enforced by the table CHECK).
async fn insert_steering_entry_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conversation_id: &str,
    ordinal: i64,
    entry: &phoenix_core::domain::sm_event::SteerEntry,
) -> DbResult<()> {
    let (skill_name, skill_body, skill_dir) = match &entry.skill_invocation {
        Some(s) => (
            Some(s.name.as_str()),
            Some(s.body.as_str()),
            Some(s.skill_dir.as_str()),
        ),
        None => (None, None, None),
    };
    sqlx::query(
        "INSERT INTO steering_messages
            (message_id, conversation_id, ordinal, text, llm_text, user_agent,
             skill_name, skill_body, skill_dir)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&entry.message_id)
    .bind(conversation_id)
    .bind(ordinal)
    .bind(&entry.text)
    .bind(&entry.llm_text)
    .bind(&entry.user_agent)
    .bind(skill_name)
    .bind(skill_body)
    .bind(skill_dir)
    .execute(&mut **tx)
    .await?;
    for (file_ordinal, file) in entry.files.iter().enumerate() {
        sqlx::query(
            "INSERT INTO steering_message_files
                (message_id, file_ordinal, original_name, media_type, size_bytes, stored_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&entry.message_id)
        .bind(i64::try_from(file_ordinal).unwrap_or(i64::MAX))
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(i64::try_from(file.size_bytes).unwrap_or(i64::MAX))
        .bind(&file.stored_path)
        .execute(&mut **tx)
        .await?;
    }
    for (image_ordinal, image) in entry.images.iter().enumerate() {
        sqlx::query(
            "INSERT INTO steering_message_images (message_id, image_ordinal, media_type, data)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&entry.message_id)
        .bind(i64::try_from(image_ordinal).unwrap_or(i64::MAX))
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// finds the deterministic child id already present is a no-op (REQ-PROJ-034
/// recovery model) — but a UNIQUE `slug` collision with a DIFFERENT conversation
/// is NOT swallowed; it surfaces as an error rather than silently skipping the
/// insert (which would FK-fail the following seed-message insert and roll back
/// the whole resolve). The caller owns id uniqueness; the fork slug is derived
/// from the deterministic conv id (see `build_child_conversation`) so it cannot
/// realistically clash with a distinct conversation.
async fn insert_conversation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    conv: &Conversation,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
         VALUES (?1, 'ordinary', 'open')
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(conv.product_conversation_id.as_str())
    .execute(&mut **tx)
    .await?;
    let state_json =
        serde_json::to_string(&conv.state).map_err(|e| DbError::Serialization(e.to_string()))?;
    let cm = conv_mode_columns(&conv.conv_mode);
    let conversation_exists =
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)")
            .bind(&conv.id)
            .fetch_one(&mut **tx)
            .await?
            != 0;
    let generated_scope = if conv.runtime_role == RuntimeRole::Coordinator
        || conv.attached_work_scope_id.is_some()
        || conversation_exists
    {
        None
    } else {
        let (scope_id, authority_kind, environment) =
            Database::new_scope_for_conversation(&conv.cwd, &cm);
        Database::insert_work_scope_tx(
            tx,
            &scope_id,
            authority_kind,
            environment,
            &conv.created_at.to_rfc3339(),
        )
        .await?;
        Some(scope_id)
    };
    let work_scope_id = conv
        .attached_work_scope_id
        .as_ref()
        .or(generated_scope.as_ref());

    // A forked/copied conversation starts with an empty steering queue (pending
    // steers are not inherited), so the steering_messages tables are not written
    // here. The legacy `steering_queue` column defaults to '[]'.
    sqlx::query(
        "INSERT INTO conversations (
            id, product_conversation_id, slug, title, parent_conversation_id, user_initiated, state, state_kind,
            state_updated_at, created_at, updated_at, archived, transcript_generation, model, effort, project_id,
            desired_base_branch, seed_parent_id, seed_label,
            continued_in_conv_id, chain_name, llm_language,
            spawned_from_conversation_id,
            cm_kind, cm_task_id, cm_task_title, cm_next_taskmd_id_hint,
            runtime_role, work_scope_id, service_tier
        ) VALUES (?1, ?31, ?2, ?3, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)
        ON CONFLICT(id) DO NOTHING",
    )
    .bind(&conv.id)
    .bind(&conv.slug)
    .bind(&conv.title)
    .bind(&conv.cwd)
    .bind(&conv.parent_conversation_id)
    .bind(conv.user_initiated)
    .bind(&state_json)
    .bind(conv_state_kind(&conv.state))
    .bind(conv.state_updated_at.to_rfc3339())
    .bind(conv.created_at.to_rfc3339())
    .bind(conv.updated_at.to_rfc3339())
    .bind(conv.archived)
    .bind(conv.transcript_generation)
    .bind(&conv.model)
    .bind(conv.effort.map(ModelEffort::as_wire_name))
    .bind(&conv.project_id)
    .bind(&conv.desired_base_branch)
    .bind(&conv.seed_parent_id)
    .bind(&conv.seed_label)
    .bind(&conv.continued_in_conv_id)
    .bind(&conv.chain_name)
    .bind(conv.llm_language.as_str())
    .bind(&conv.spawned_from_conversation_id)
    .bind(cm.kind)
    .bind(cm.task_id)
    .bind(cm.task_title)
    .bind(cm.next_taskmd_id_hint)
    .bind(conv.runtime_role.as_str())
    .bind(work_scope_id.map(WorkScopeId::as_str))
    .bind(conv.service_tier.as_wire_name())
    .bind(conv.product_conversation_id.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert a seed `Message` row inside a transaction, reusing the same column
/// mapping as [`Database::add_message_with_seq`]. `INSERT OR IGNORE` keyed on
/// `message_id` makes a crash-retry a no-op rather than a duplicate.
/// Derive the message ID used to persist a tool result. Must match the
/// runtime executor's convention (`phoenix-state-machine`'s
/// `tool_result_message_id`) so the restart-materialized result shares identity
/// with the row the live path would have written: `{tool_use_id}-result`.
fn tool_result_message_id(tool_use_id: &str) -> String {
    format!("{tool_use_id}-result")
}

/// Fold a tool result's `duration_ms` into its `display_data` JSON, mirroring
/// the runtime executor's `merge_duration_into_display_data` so a
/// restart-materialized tool result carries the same baked-in duration the
/// live persist path would have written.
fn merge_duration_into_display(
    existing: Option<&serde_json::Value>,
    duration_ms: Option<u64>,
) -> Option<serde_json::Value> {
    match (existing, duration_ms) {
        (None, None) => None,
        (Some(v), None) => Some(v.clone()),
        (None, Some(ms)) => Some(serde_json::json!({ "duration_ms": ms })),
        (Some(v), Some(ms)) => {
            let mut merged = v.clone();
            if let Some(obj) = merged.as_object_mut() {
                obj.insert(
                    "duration_ms".to_string(),
                    serde_json::Value::Number(ms.into()),
                );
            }
            Some(merged)
        }
    }
}

/// The four pieces a recovered in-flight tool round contributes to
/// materialization: the held assistant turn, the results of tools that already
/// finished, the ids of tools that were interrupted (need a synthetic result),
/// and any sub-agents spawned earlier in the same round that never reached
/// fan-in.
struct NormalizedRound {
    assistant_message: phoenix_core::domain::sm_state::AssistantMessage,
    completed_results: Vec<ToolResult>,
    interrupted_tool_ids: Vec<String>,
    pending_sub_agents: Vec<phoenix_core::domain::sm_state::PendingSubAgent>,
}

/// Normalize a recovered in-flight tool round (`ToolExecuting` or
/// `CancellingTool`) into the assistant message, the completed results, the flat
/// list of interrupted `tool_use` ids, and any pending sub-agents — everything
/// [`build_materialized_tool_round`] consumes. Returns `None` for any other
/// state — the materialization caller filters to these two states via SQL, so
/// `None` is unreachable there, but keeping this total (rather than panicking)
/// means a future caller can't trip an `unwrap`.
fn normalize_in_flight_round(
    state: phoenix_core::domain::sm_state::ConvState,
) -> Option<NormalizedRound> {
    use phoenix_core::domain::sm_state::ConvState;

    if let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        pending_sub_agents,
        assistant_message,
    } = state
    {
        let interrupted_tool_ids = std::iter::once(current_tool.id)
            .chain(remaining_tools.into_iter().map(|t| t.id))
            .collect::<Vec<_>>();
        return Some(NormalizedRound {
            assistant_message,
            completed_results,
            interrupted_tool_ids,
            pending_sub_agents,
        });
    }
    if let ConvState::CancellingTool {
        tool_use_id,
        skipped_tools,
        completed_results,
        assistant_message,
        pending_sub_agents,
    } = state
    {
        let interrupted_tool_ids = std::iter::once(tool_use_id)
            .chain(skipped_tools.into_iter().map(|t| t.id))
            .collect::<Vec<_>>();
        return Some(NormalizedRound {
            assistant_message,
            completed_results,
            interrupted_tool_ids,
            pending_sub_agents,
        });
    }
    None
}

/// For each `spawn_agents` placeholder result whose sub-agents were still
/// pending at restart, build the interrupted fan-in that replaces it: the
/// LLM-readable `(content, display_data)` pair matching the live
/// `PersistSubAgentResults` shape, keyed by the spawn tool's `tool_use_id`.
///
/// Returns an empty map when no sub-agents were pending — the common case, where
/// the round materializes exactly as before.
///
/// Identifying spawn placeholders is structural: a `completed_results` entry is
/// a `spawn_agents` result iff the assistant turn's `tool_use` block with the
/// same id has `name == "spawn_agents"`. Partitioning pending sub-agents to the
/// right spawn (a round may carry several, e.g. `[spawn_agents A, bash,
/// spawn_agents B]`) uses the placeholder's `"Spawning …: <agent_ids>"` text:
/// the spawn that launched an agent names that agent's (UUID) id in its output.
/// A pending sub-agent that matches no placeholder is folded into the single
/// spawn result when exactly one exists, otherwise dropped from the fan-in (it
/// still cannot orphan a `tool_use` — every spawn result is paired regardless).
fn synthesize_spawn_fan_ins(
    assistant_message: &phoenix_core::domain::sm_state::AssistantMessage,
    completed_results: &[ToolResult],
    pending_sub_agents: &[phoenix_core::domain::sm_state::PendingSubAgent],
    sub_agent_outcomes: &std::collections::HashMap<
        String,
        phoenix_core::domain::sm_state::SubAgentOutcome,
    >,
) -> std::collections::HashMap<String, (String, serde_json::Value)> {
    use phoenix_core::domain::llm_types::ContentBlock;
    use std::collections::HashMap;

    if pending_sub_agents.is_empty() {
        return HashMap::new();
    }

    // tool_use_id -> tool name, from the held assistant turn.
    let tool_names: HashMap<&str, &str> = assistant_message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            ContentBlock::Text { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::WebSearchToolResult { .. }
            | ContentBlock::WebFetchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. }
            | ContentBlock::BashCodeExecutionToolResult { .. }
            | ContentBlock::TextEditorCodeExecutionToolResult { .. }
            | ContentBlock::McpToolUse { .. }
            | ContentBlock::McpToolResult { .. } => None,
        })
        .collect();

    // The spawn_agents placeholder results, in round order.
    let spawn_results: Vec<&ToolResult> = completed_results
        .iter()
        .filter(|r| tool_names.get(r.tool_use_id.as_str()) == Some(&"spawn_agents"))
        .collect();

    if spawn_results.is_empty() {
        // Pending sub-agents but no spawn placeholder to attach them to — the
        // tool that spawned them isn't in `completed_results` (e.g. it is the
        // interrupted tool itself). Nothing to rewrite; the pairing guard and
        // orphan repair still cover every tool_use.
        return HashMap::new();
    }

    // Partition pending sub-agents to their originating spawn placeholder by
    // matching the agent's UUID against the placeholder's output text. A
    // sub-agent matching no placeholder is assigned to the sole spawn when
    // exactly one exists (the unambiguous common case).
    let mut by_spawn: HashMap<String, Vec<&phoenix_core::domain::sm_state::PendingSubAgent>> =
        HashMap::new();
    let single_spawn_id = if spawn_results.len() == 1 {
        Some(spawn_results[0].tool_use_id.clone())
    } else {
        None
    };
    for agent in pending_sub_agents {
        let target = spawn_results
            .iter()
            .find(|r| r.output().contains(agent.agent_id.as_str()))
            .map(|r| r.tool_use_id.clone())
            .or_else(|| single_spawn_id.clone());
        if let Some(id) = target {
            by_spawn.entry(id).or_default().push(agent);
        }
    }

    // Build the fan-in for every spawn placeholder. A spawn that ended up with
    // no matched agents still gets an (empty-results) fan-in so its placeholder
    // is rewritten consistently rather than left as raw "Spawning …" text.
    spawn_results
        .iter()
        .map(|r| {
            let agents = by_spawn.remove(&r.tool_use_id).unwrap_or_default();
            let results: Vec<phoenix_core::domain::sm_state::SubAgentResult> =
                agents
                    .iter()
                    .map(|a| phoenix_core::domain::sm_state::SubAgentResult {
                        agent_id: a.agent_id.clone(),
                        task: a.task.clone(),
                        // The child conversation's real terminal outcome if it
                        // already finished (success/failure) before the restart;
                        // otherwise it was genuinely interrupted mid-flight. Without
                        // this, a sub-agent that succeeded would be reported as a
                        // failure.
                        outcome: sub_agent_outcomes.get(&a.agent_id).cloned().unwrap_or_else(
                            || phoenix_core::domain::sm_state::SubAgentOutcome::Failure {
                                error: "Sub-agent interrupted by server restart".to_string(),
                                error_kind:
                                    phoenix_core::domain::db_schema::ErrorKind::SubAgentError,
                            },
                        ),
                    })
                    .collect();
            (r.tool_use_id.clone(), build_sub_agent_fan_in(&results))
        })
        .collect()
}

/// Build the `(llm_content, display_data)` pair for a `spawn_agents` fan-in,
/// mirroring the runtime executor's `persist_sub_agent_results`: an LLM-readable
/// outcome summary and a `subagent_summary` display blob carrying the typed
/// results. Kept in lockstep with that path so a restart-materialized fan-in is
/// indistinguishable from one the live persist would have written.
fn build_sub_agent_fan_in(
    results: &[phoenix_core::domain::sm_state::SubAgentResult],
) -> (String, serde_json::Value) {
    use phoenix_core::domain::sm_state::SubAgentOutcome;

    let body = results
        .iter()
        .map(|r| {
            let outcome = match &r.outcome {
                SubAgentOutcome::Success { result } => format!("Result: {result}"),
                SubAgentOutcome::Failure { error, .. } => format!("Failed: {error}"),
                SubAgentOutcome::TimedOut => {
                    "Timed out: sub-agent exceeded its time limit".to_string()
                }
            };
            format!("Task: \"{}\"\n{outcome}", r.task)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let llm_content = format!("Sub-agent results ({} completed):\n\n{body}", results.len());
    let display_data = serde_json::json!({
        "type": "subagent_summary",
        "results": results,
    });
    (llm_content, display_data)
}

/// Build the materialized message rows for an in-flight tool round recovered
/// from a `ToolExecuting` or `CancellingTool` state on restart: the assistant
/// message followed by one tool-result row per `tool_use`, in round order.
/// Completed tools contribute their real result; every interrupted tool
/// (the in-flight/cancelling tool plus every not-yet-started tool) gets a
/// synthetic interrupted error so each `tool_use` is paired. Sequence ids are
/// allocated contiguously from `start_seq` (assistant lowest), matching the seq
/// ordering the live persist path would have produced.
///
/// `interrupted_tool_ids` carries only the `tool_use` ids of the unfinished
/// tools — both recovered states identify those tools by id (`ToolExecuting`
/// via `current_tool.id` + `remaining_tools`, `CancellingTool` via
/// `tool_use_id` + `skipped_tools`), so the builder needs nothing more than the
/// ids to pair a synthetic result.
///
/// `pending_sub_agents` carries sub-agents spawned earlier in this same round
/// (by a `spawn_agents` tool) that never reached fan-in because a later tool was
/// still executing / cancelling when the process exited. Their originating
/// `spawn_agents` result sits in `completed_results` as a raw "Spawning N
/// sub-agent(s): …" placeholder. Materializing that placeholder verbatim would
/// leave an orphaned spawn in history — the LLM sees agents launched but no
/// outcomes. Instead, each pending sub-agent is synthesized as an
/// interrupted-by-restart `SubAgentResult`, and the `spawn_agents` placeholder
/// result is rewritten into the same fan-in shape the live
/// `PersistSubAgentResults` path produces (LLM-readable summary + a
/// `subagent_summary` `display_data`), so the recovered chain reflects "agents
/// spawned, interrupted by restart" rather than a dangling spawn.
#[allow(clippy::too_many_arguments)]
fn build_materialized_tool_round(
    conv_id: &str,
    start_seq: i64,
    now: &DateTime<Utc>,
    assistant_message: &phoenix_core::domain::sm_state::AssistantMessage,
    completed_results: &[ToolResult],
    interrupted_tool_ids: &[String],
    pending_sub_agents: &[phoenix_core::domain::sm_state::PendingSubAgent],
    sub_agent_outcomes: &std::collections::HashMap<
        String,
        phoenix_core::domain::sm_state::SubAgentOutcome,
    >,
) -> (Message, Vec<Message>) {
    let mut next_seq = start_seq;

    let agent_content = MessageContent::agent(assistant_message.content.clone());
    let agent_msg = Message {
        message_id: assistant_message.message_id.clone(),
        conversation_id: conv_id.to_string(),
        sequence_id: next_seq,
        message_type: agent_content.message_type(),
        content: agent_content,
        display_data: assistant_message.display_data.clone(),
        usage_data: assistant_message.usage.clone(),
        created_at: assistant_message.created_at,
    };
    next_seq += 1;

    // Map each completed result to its tool's name (from the assistant turn's
    // `tool_use` blocks) so we can identify `spawn_agents` placeholders
    // structurally rather than by matching their output text.
    let spawn_fan_ins = synthesize_spawn_fan_ins(
        assistant_message,
        completed_results,
        pending_sub_agents,
        sub_agent_outcomes,
    );

    let mut tool_msgs: Vec<Message> = Vec::new();

    // Completed tools keep their real output — except a `spawn_agents`
    // placeholder whose sub-agents were still pending, which is rewritten into
    // the interrupted fan-in (content + display) computed above.
    for result in completed_results {
        let (output, is_error, display): (String, bool, Option<serde_json::Value>) =
            match spawn_fan_ins.get(&result.tool_use_id) {
                Some((content, display)) => (content.clone(), false, Some(display.clone())),
                None => (
                    result.output().to_string(),
                    result.is_error(),
                    merge_duration_into_display(result.display_data(), result.duration_ms),
                ),
            };
        let content = MessageContent::tool_with_images(
            &result.tool_use_id,
            output,
            is_error,
            result.images().to_vec(),
        );
        tool_msgs.push(Message {
            message_id: tool_result_message_id(&result.tool_use_id),
            conversation_id: conv_id.to_string(),
            sequence_id: next_seq,
            message_type: content.message_type(),
            content,
            display_data: display,
            usage_data: None,
            created_at: *now,
        });
        next_seq += 1;
    }

    // The in-flight/cancelling tool and every queued-but-unstarted tool were
    // interrupted.
    for tool_id in interrupted_tool_ids {
        let content = MessageContent::tool(
            tool_id,
            "[Tool execution interrupted by server restart]",
            true,
        );
        tool_msgs.push(Message {
            message_id: tool_result_message_id(tool_id),
            conversation_id: conv_id.to_string(),
            sequence_id: next_seq,
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: *now,
        });
        next_seq += 1;
    }

    (agent_msg, tool_msgs)
}

async fn insert_message_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    msg: &Message,
) -> DbResult<()> {
    let observer = sqlite_telemetry::ParentSqliteObserver::UninstrumentedNested;
    let content_str = serde_json::to_string(&msg.content.to_stored_json())
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let display_str = msg
        .display_data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let usage_str = msg
        .usage_data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| DbError::Serialization(e.to_string()))?;

    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&msg.message_id)
    .bind(&msg.conversation_id)
    .bind(msg.sequence_id)
    .bind(msg.message_type.to_string())
    .bind(&content_str)
    .bind(&display_str)
    .bind(&usage_str)
    .bind(msg.created_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let mut existing = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE message_id = ?1",
        )
        .bind(&msg.message_id)
        .try_map(parse_message_row)
        .fetch_one(&mut **tx)
        .await?;
        hydrate_message_attachments_tx(tx, &mut existing).await?;
        let exact = existing.message_id == msg.message_id
            && existing.conversation_id == msg.conversation_id
            && existing.sequence_id == msg.sequence_id
            && existing.message_type == msg.message_type
            && existing.content == msg.content
            && existing.display_data == msg.display_data
            && existing.usage_data == msg.usage_data
            && existing.created_at == msg.created_at;
        if !exact {
            return Err(DbError::Serialization(format!(
                "message {} conflicts with first durable payload",
                msg.message_id
            )));
        }
        retrieval::fts_upsert_conn(
            tx,
            msg,
            retrieval::FtsObservation::ParentTransaction(observer),
        )
        .await?;
        return Ok(());
    }
    insert_message_attachments(tx, &msg.message_id, &msg.content).await?;
    // Index for retrieval atomically with the message insert, so tx-based
    // persists (fork-resolution seed messages, checkpoint replays) get the
    // same FTS coverage as `add_message_with_seq` — no message reaches a chain
    // unindexed before the startup reconcile (specs/conversation-retrieval/
    // REQ-RET-003).
    retrieval::fts_upsert_conn(
        tx,
        msg,
        retrieval::FtsObservation::ParentTransaction(observer),
    )
    .await?;
    Ok(())
}

async fn steering_message_matches_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message: &Message,
) -> DbResult<Option<bool>> {
    let row = sqlx::query(
        "SELECT conversation_id, message_type, content, display_data, usage_data
         FROM messages WHERE message_id = ?1",
    )
    .bind(&message.message_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };

    let stored_content: serde_json::Value = serde_json::from_str(&row.get::<String, _>("content"))
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let stored_display = row
        .get::<Option<String>, _>("display_data")
        .map(|value| serde_json::from_str::<serde_json::Value>(&value))
        .transpose()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let stored_usage = row
        .get::<Option<String>, _>("usage_data")
        .map(|value| serde_json::from_str::<serde_json::Value>(&value))
        .transpose()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let expected_usage = message
        .usage_data
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    if row.get::<String, _>("conversation_id") != message.conversation_id
        || row.get::<String, _>("message_type") != message.message_type.to_string()
        || stored_content != message.content.to_stored_json()
        || stored_display != message.display_data
        || stored_usage != expected_usage
    {
        return Ok(Some(false));
    }

    let (images, files) = message.content.attachments();
    let stored_files = sqlx::query(
        "SELECT original_name, media_type, size_bytes, stored_path
         FROM message_files WHERE message_id = ?1 ORDER BY ordinal",
    )
    .bind(&message.message_id)
    .fetch_all(&mut **tx)
    .await?;
    if stored_files.len() != files.len()
        || stored_files.iter().zip(files).any(|(row, file)| {
            row.get::<String, _>("original_name") != file.original_name
                || row.get::<String, _>("media_type") != file.media_type
                || row.get::<i64, _>("size_bytes")
                    != i64::try_from(file.size_bytes).unwrap_or(i64::MAX)
                || row.get::<String, _>("stored_path") != file.stored_path
        })
    {
        return Ok(Some(false));
    }

    let stored_images = sqlx::query(
        "SELECT media_type, data FROM message_images WHERE message_id = ?1 ORDER BY ordinal",
    )
    .bind(&message.message_id)
    .fetch_all(&mut **tx)
    .await?;
    if stored_images.len() != images.len()
        || stored_images.iter().zip(images).any(|(row, image)| {
            row.get::<String, _>("media_type") != image.media_type
                || row.get::<String, _>("data") != image.data
        })
    {
        return Ok(Some(false));
    }

    Ok(Some(true))
}

fn cleared_creation_intent_json() -> String {
    serde_json::json!({
        "cwd": "",
        "model": null,
        "text": "",
        "expansion_preflighted": false,
        "llm_text": null,
        "skill_invocation": null,
        "images": [],
        "mode": null,
        "base_branch": null,
        "checkout_ref": null,
        "reserved_checkout_oid": null,
        "seed_parent_id": null,
        "seed_label": null
    })
    .to_string()
}

async fn update_claimed_creation_job_ready(
    tx: &mut Transaction<'_, Sqlite>,
    job_id: &str,
    conversation_id: &str,
    claim: &CreationClaim,
    cleared_intent: &str,
    now: &str,
) -> DbResult<u64> {
    let result = sqlx::query(
        "UPDATE conversation_creation_jobs
         SET status = 'ready', intent_json = ?1, updated_at = ?2, completed_at = ?2,
             claim_worker_id = NULL, claim_token = NULL, lease_until = NULL
         WHERE id = ?3 AND conversation_id = ?4 AND status = 'claimed' AND generation = ?5
           AND claim_worker_id = ?6 AND claim_token = ?7 AND lease_until > ?2",
    )
    .bind(cleared_intent)
    .bind(now)
    .bind(job_id)
    .bind(conversation_id)
    .bind(claim_generation_i64(claim)?)
    .bind(&claim.worker_id.0)
    .bind(&claim.token.0)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

async fn update_creation_runtime_state(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    state: &ConvState,
    state_json: &str,
    state_updated_at: &str,
    now: &str,
) -> DbResult<()> {
    let result = sqlx::query(
        "UPDATE conversations
         SET state = ?1, state_kind = ?2, state_updated_at = ?3, updated_at = ?4
         WHERE id = ?5",
    )
    .bind(state_json)
    .bind(conv_state_kind(state))
    .bind(state_updated_at)
    .bind(now)
    .bind(conversation_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::ConversationNotFound(conversation_id.to_string()));
    }
    Ok(())
}

async fn clear_creation_job_attachments(
    tx: &mut Transaction<'_, Sqlite>,
    job_id: &str,
) -> DbResult<()> {
    sqlx::query("DELETE FROM conversation_creation_job_files WHERE job_id = ?1")
        .bind(job_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM conversation_creation_job_images WHERE job_id = ?1")
        .bind(job_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Write a message's user/skill attachments to the `message_files` /
/// `message_images` child tables. `INSERT OR IGNORE` keyed on
/// `(message_id, ordinal)` makes this idempotent under retry, matching the
/// `INSERT OR IGNORE` on the parent message row.
async fn hydrate_message_attachments_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message: &mut Message,
) -> DbResult<()> {
    let files = sqlx::query(
        "SELECT original_name, media_type, size_bytes, stored_path
         FROM message_files WHERE message_id = ?1 ORDER BY ordinal",
    )
    .bind(&message.message_id)
    .map(|row: SqliteRow| FileAttachment {
        original_name: row.get("original_name"),
        media_type: row.get("media_type"),
        size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).unwrap_or(0),
        stored_path: row.get("stored_path"),
    })
    .fetch_all(&mut **tx)
    .await?;
    let images = if message.message_type == MessageType::User {
        sqlx::query(
            "SELECT media_type, data
             FROM message_images WHERE message_id = ?1 ORDER BY ordinal",
        )
        .bind(&message.message_id)
        .map(|row: SqliteRow| ImageData {
            media_type: row.get("media_type"),
            data: row.get("data"),
        })
        .fetch_all(&mut **tx)
        .await?
    } else {
        Vec::new()
    };
    message.content.set_attachments(images, files);
    Ok(())
}

async fn insert_message_attachments(
    conn: &mut sqlx::SqliteConnection,
    message_id: &str,
    content: &MessageContent,
) -> DbResult<()> {
    let (images, files) = content.attachments();
    for (ordinal, file) in files.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO message_files
             (message_id, ordinal, original_name, media_type, size_bytes, stored_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(message_id)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&file.original_name)
        .bind(&file.media_type)
        .bind(i64::try_from(file.size_bytes).unwrap_or(i64::MAX))
        .bind(&file.stored_path)
        .execute(&mut *conn)
        .await?;
    }
    for (ordinal, image) in images.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO message_images (message_id, ordinal, media_type, data)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(message_id)
        .bind(i64::try_from(ordinal).unwrap_or(i64::MAX))
        .bind(&image.media_type)
        .bind(&image.data)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Load each message's attachments from the child tables onto its runtime
/// content. User/skill rows read from the DB come back with empty attachments
/// (the blob no longer carries them); this restores them. Non-user/skill
/// messages are left untouched.
pub(crate) async fn hydrate_attachments(
    pool: &SqlitePool,
    messages: &mut [Message],
) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    hydrate_attachments_conn(&mut conn, messages).await
}

pub(crate) async fn hydrate_attachments_conn(
    conn: &mut sqlx::SqliteConnection,
    messages: &mut [Message],
) -> Result<(), sqlx::Error> {
    for msg in messages.iter_mut() {
        if !matches!(msg.message_type, MessageType::User | MessageType::Skill) {
            continue;
        }
        let files = sqlx::query(
            "SELECT original_name, media_type, size_bytes, stored_path
             FROM message_files WHERE message_id = ?1 ORDER BY ordinal",
        )
        .bind(&msg.message_id)
        .map(|row: SqliteRow| FileAttachment {
            original_name: row.get("original_name"),
            media_type: row.get("media_type"),
            size_bytes: u64::try_from(row.get::<i64, _>("size_bytes")).unwrap_or(0),
            stored_path: row.get("stored_path"),
        })
        .fetch_all(&mut *conn)
        .await?;

        // SkillContent has no images; skip the query for skill rows.
        let images = if matches!(msg.message_type, MessageType::User) {
            sqlx::query(
                "SELECT media_type, data FROM message_images WHERE message_id = ?1 ORDER BY ordinal",
            )
            .bind(&msg.message_id)
            .map(|row: SqliteRow| ImageData {
                data: row.get("data"),
                media_type: row.get("media_type"),
            })
            .fetch_all(&mut *conn)
            .await?
        } else {
            Vec::new()
        };

        msg.content.set_attachments(images, files);
    }
    Ok(())
}

/// Insert a `fork_proposals` row inside a transaction, reusing the same column
/// mapping as [`Database::insert_fork_proposal`].
async fn insert_fork_proposal_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    p: &ForkProposal,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO fork_proposals (
            id, origin_conv_id, task_file, title, priority, body, status,
            fork_conv_id, refinement_conv_id, created_at, resolved_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(&p.id)
    .bind(&p.origin_conversation_id)
    .bind(&p.task_file)
    .bind(&p.title)
    .bind(&p.priority)
    .bind(&p.body)
    .bind(p.status.as_str())
    .bind(p.fork_conversation_id.as_deref())
    .bind(p.refinement_conversation_id.as_deref())
    .bind(p.created_at.to_rfc3339())
    .bind(p.resolved_at.map(|t| t.to_rfc3339()))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Parse a project row from the database
#[allow(clippy::needless_pass_by_value)]
fn parse_project_row(row: SqliteRow) -> Result<Project, sqlx::Error> {
    Ok(Project {
        id: row.try_get("id")?,
        canonical_path: row.try_get("canonical_path")?,
        main_ref: row.try_get("main_ref")?,
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
        conversation_count: row.try_get("conversation_count")?,
    })
}

fn is_adopted_wake_result_message(message: &Message) -> bool {
    message.message_type == MessageType::User
        && message.display_data.as_ref().is_some_and(|data| {
            data.get("type").and_then(serde_json::Value::as_str) == Some("wake_result")
                && data.get("adopted").and_then(serde_json::Value::as_bool) == Some(true)
        })
}

/// Parse a message row from the database
#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_message_row(row: SqliteRow) -> Result<Message, sqlx::Error> {
    let msg_type = parse_message_type(&row.try_get::<String, _>("message_type")?);
    let content_str: String = row.try_get("content")?;
    let content_value: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();

    // Parse the persisted (attachment-free) content; the caller hydrates
    // user/skill attachments from the child tables via `hydrate_attachments`.
    let content = MessageContent::from_stored_json(msg_type, content_value)
        .unwrap_or_else(|_| MessageContent::error(format!("Failed to parse {msg_type} message")));

    Ok(Message {
        message_id: row.try_get("message_id")?,
        conversation_id: row.try_get("conversation_id")?,
        sequence_id: row.try_get("sequence_id")?,
        message_type: msg_type,
        content,
        display_data: row
            .try_get::<Option<String>, _>("display_data")?
            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
        usage_data: row
            .try_get::<Option<String>, _>("usage_data")?
            .and_then(|s| serde_json::from_str(&s).ok()),
        created_at: parse_datetime(&row.try_get::<String, _>("created_at")?),
    })
}

fn parse_message_type(s: &str) -> MessageType {
    // Use serde to ensure we stay in sync with MessageType's Deserialize impl
    // The JSON string format "type" matches our snake_case serde config
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(MessageType::System)
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

pub(crate) const fn conv_state_kind(state: &ConvState) -> &'static str {
    match state {
        ConvState::Idle => "idle",
        ConvState::LlmRequesting { .. } => "llm_requesting",
        ConvState::ToolExecuting { .. } => "tool_executing",
        ConvState::CancellingTool { .. } => "cancelling_tool",
        ConvState::AwaitingSubAgents { .. } => "awaiting_sub_agents",
        ConvState::CancellingSubAgents { .. } => "cancelling_sub_agents",
        ConvState::Error { .. } => "error",
        ConvState::AwaitingContinuation { .. } => "awaiting_continuation",
        ConvState::RecoverableContinuationFailure { .. } => "recoverable_continuation_failure",
        ConvState::AwaitingRecovery { .. } => "awaiting_recovery",
        ConvState::AwaitingTaskApproval { .. } => "awaiting_task_approval",
        ConvState::AwaitingUserResponse { .. } => "awaiting_user_response",
        ConvState::ContextExhausted { .. } => "context_exhausted",
        ConvState::HandedOff { .. } => "handed_off",
        ConvState::Terminal => "terminal",
        ConvState::Completed { .. } => "completed",
        ConvState::Failed { .. } => "failed",
        ConvState::Provisioning { .. } => "provisioning",
        ConvState::CreationFailed { .. } => "creation_failed",
        ConvState::CreationCancelled { .. } => "creation_cancelled",
        ConvState::SeededLlmRequesting { .. } => "seeded_llm_requesting",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::llm_language::LlmLanguage;

    #[test]
    fn worktree_fingerprint_tracks_git_administrative_incarnation() {
        let root = std::env::temp_dir().join(format!(
            "phoenix-worktree-fingerprint-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join(".git");
        std::fs::write(&marker, "gitdir: /tmp/first\n").unwrap();
        let first = Database::observe_worktree_fingerprint(root.to_str().unwrap()).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::fs::write(&marker, "gitdir: /tmp/second\n").unwrap();
        let replacement = Database::observe_worktree_fingerprint(root.to_str().unwrap()).unwrap();
        assert_ne!(first, replacement);
        let max_pointer_bytes = usize::try_from(MAX_GIT_POINTER_BYTES).unwrap();
        let mut maximum_pointer = b"gitdir: ".to_vec();
        maximum_pointer.resize(max_pointer_bytes - 1, b'x');
        maximum_pointer.push(b'\n');
        std::fs::write(&marker, maximum_pointer).unwrap();
        assert!(Database::observe_worktree_fingerprint(root.to_str().unwrap()).is_some());
        std::fs::write(&marker, vec![b'x'; max_pointer_bytes + 1]).unwrap();
        assert!(Database::observe_worktree_fingerprint(root.to_str().unwrap()).is_none());
        std::fs::write(&marker, "not-a-git-pointer\n").unwrap();
        assert!(Database::observe_worktree_fingerprint(root.to_str().unwrap()).is_none());
        std::fs::write(&marker, "gitdir: \n").unwrap();
        assert!(Database::observe_worktree_fingerprint(root.to_str().unwrap()).is_none());
        std::fs::write(&marker, "gitdir: /tmp/windows-linked-worktree\r\n").unwrap();
        assert!(Database::observe_worktree_fingerprint(root.to_str().unwrap()).is_some());
        let crlf = Database::observe_worktree_fingerprint(root.to_str().unwrap()).unwrap();
        std::fs::write(&marker, "gitdir: /tmp/windows-linked-worktree\n").unwrap();
        let lf = Database::observe_worktree_fingerprint(root.to_str().unwrap()).unwrap();
        assert_eq!(crlf, lf);
        std::fs::write(&marker, "gitdir: /tmp/second\n").unwrap();
        std::fs::rename(&root, root.with_extension("moved")).unwrap();
        let moved = root.with_extension("moved");
        assert_eq!(
            replacement,
            Database::observe_worktree_fingerprint(moved.to_str().unwrap()).unwrap()
        );
        std::fs::remove_dir_all(moved).unwrap();
    }

    #[tokio::test]
    async fn worktree_reconciliation_releases_stale_owner_before_claim_arbitration() {
        let db = Database::open_in_memory().await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let claimant = temp.path().join("claimant");
        let stale_owner = temp.path().join("stale-owner");
        std::fs::create_dir_all(&claimant).unwrap();
        std::fs::create_dir_all(&stale_owner).unwrap();
        std::fs::write(claimant.join(".git"), "gitdir: /tmp/original\n").unwrap();
        std::fs::write(stale_owner.join(".git"), "gitdir: /tmp/replacement\n").unwrap();
        let released_fingerprint =
            Database::observe_worktree_fingerprint(claimant.to_str().unwrap()).unwrap();
        let replacement_fingerprint =
            Database::observe_worktree_fingerprint(stale_owner.to_str().unwrap()).unwrap();

        sqlx::query(
            "INSERT INTO work_scopes (
                 id, authority_kind, lifecycle, environment_kind, cwd, worktree_path,
                 created_at, updated_at, worktree_id, worktree_fingerprint
             ) VALUES
                 ('a-claimant', 'work', 'active', 'allocated_worktree', ?1, ?1,
                  ?3, ?3, NULL, NULL),
                 ('b-stale-owner', 'work', 'active', 'allocated_worktree', ?2, ?2,
                  ?3, ?3, 'stale-id', ?4)",
        )
        .bind(claimant.to_str().unwrap())
        .bind(stale_owner.to_str().unwrap())
        .bind(Utc::now().to_rfc3339())
        .bind(&released_fingerprint)
        .execute(db.pool())
        .await
        .unwrap();

        db.reconcile_worktree_identities().await.unwrap();

        let identities = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            "SELECT id, worktree_id, worktree_fingerprint
             FROM work_scopes
             WHERE id IN ('a-claimant', 'b-stale-owner')
             ORDER BY id",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(identities.len(), 2);
        assert!(identities[0].1.is_some());
        assert_eq!(
            identities[0].2.as_deref(),
            Some(released_fingerprint.as_str())
        );
        assert!(identities[1].1.is_some());
        assert_eq!(
            identities[1].2.as_deref(),
            Some(replacement_fingerprint.as_str())
        );
    }

    #[tokio::test]
    async fn worktree_reconciliation_preserves_retired_identity_evidence() {
        let db = Database::open_in_memory().await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("retired-worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: /tmp/replacement\n").unwrap();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO work_scopes (
                 id, authority_kind, lifecycle, environment_kind, cwd, worktree_path,
                 created_at, updated_at, retired_at, worktree_id, worktree_fingerprint
             ) VALUES ('retired-scope', 'work', 'retired', 'allocated_worktree',
                       ?1, ?1, ?2, ?2, ?2, 'retired-id', 'retired-fingerprint')",
        )
        .bind(worktree.to_str().unwrap())
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();

        db.reconcile_worktree_identities().await.unwrap();

        let identity = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT worktree_id, worktree_fingerprint
             FROM work_scopes WHERE id = 'retired-scope'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(identity.0.as_deref(), Some("retired-id"));
        assert_eq!(identity.1.as_deref(), Some("retired-fingerprint"));
    }

    fn test_creation_intent(cwd: &str, message_id: &str) -> ConversationCreationIntent {
        ConversationCreationIntent {
            cwd: cwd.to_string(),
            profile: None,
            model: None,
            effort: None,
            text: "test creation".to_string(),
            expansion_preflighted: false,
            llm_text: None,
            skill_invocation: None,
            message_id: message_id.to_string(),
            images: Vec::new(),
            files: Vec::new(),
            mode: None,
            base_branch: None,
            checkout_ref: None,
            reserved_checkout_oid: None,
            reserved_repo_root: None,
            reserved_root_freshness: None,
            seed_parent_id: None,
            reserved_root_failure: None,
            seed_label: None,
            approved_task: None,
        }
    }

    async fn attach_hidden_repository_for_test(
        db: &Database,
        conversation_id: &str,
        common_dir: &str,
        management_root: &str,
        default_branch: GitRepositoryDefaultBranchObservation,
        observed_at: chrono::DateTime<Utc>,
    ) -> AttachedHiddenGitRepository {
        let message_id = format!(
            "message-{conversation_id}-{}",
            observed_at.timestamp_micros()
        );
        let existing_job = db
            .get_conversation_creation_job_for_conversation(conversation_id)
            .await
            .unwrap();
        let job_id = if let Some(job) = existing_job {
            sqlx::query(
                "UPDATE conversation_creation_jobs
                    SET status = 'accepted', stage = 'validate_intent', claim_worker_id = NULL,
                        claim_token = NULL, lease_until = NULL, next_attempt_at = NULL,
                        updated_at = ?2, intent_json = ?3, message_id = ?4
                  WHERE id = ?1",
            )
            .bind(&job.id)
            .bind(Utc::now().to_rfc3339())
            .bind(
                serde_json::to_string(&test_creation_intent(management_root, &message_id)).unwrap(),
            )
            .bind(&message_id)
            .execute(db.pool())
            .await
            .unwrap();
            job.id
        } else {
            let job = InsertConversationCreationJob {
                id: format!("job-{conversation_id}"),
                conversation_id: conversation_id.to_string(),
                message_id: Some(message_id.clone()),
                intent: test_creation_intent(management_root, &message_id),
            };
            db.insert_conversation_creation_job(&job).await.unwrap();
            job.id
        };
        let claim = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker".into()),
                &CreationClaimToken("token".into()),
                Utc::now(),
                chrono::Duration::minutes(5),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(claimed_job) = claim else {
            panic!("creation claim");
        };
        let CreationStatus::Claimed(claim) = claimed_job.protocol.status else {
            panic!("creation claim authority");
        };
        let (_, attachment) = db
            .attach_hidden_git_repository_to_conversation_work_scope(
                &AttachHiddenGitRepositoryInput {
                    conversation_id: conversation_id.to_string(),
                    common_dir: common_dir.to_string(),
                    management_root: management_root.to_string(),
                    materialized_worktree: management_root.to_string(),
                    default_branch,
                    observed_at,
                },
                &job_id,
                &claim,
            )
            .await
            .unwrap();
        attachment.expect("hidden repository attached")
    }

    async fn insert_test_creation_job(db: &Database, job_id: &str, conversation_id: &str) {
        db.create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_conversation_creation_job(&InsertConversationCreationJob {
            id: job_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: Some(format!("message-{job_id}")),
            intent: test_creation_intent("/tmp", &format!("message-{job_id}")),
        })
        .await
        .unwrap();
    }

    async fn claim_test_cleanup(db: &Database) -> CreationCleanupJob {
        db.claim_next_conversation_creation_cleanup(
            "cleanup-worker",
            "cleanup-token",
            Utc::now(),
            chrono::Duration::minutes(1),
        )
        .await
        .unwrap()
        .expect("cleanup claim")
    }

    #[tokio::test]
    async fn recovery_settlement_applies_only_while_terminal_message_is_tail() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("settled", "settled", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "terminal-result",
            "settled",
            &MessageContent::tool("tool-use", "retired", true),
            None,
            None,
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversation_recovery_settlements (
                 conversation_id, terminal_message_id, reason
             ) VALUES ('settled', 'terminal-result', 'retired_tool_call')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        assert_eq!(
            db.get_recovery_tail_status("settled").await.unwrap(),
            phoenix_core::domain::db_schema::RecoveryTailStatus::Tail {
                message_id: "terminal-result".to_string(),
                settlement: Some(
                    phoenix_core::domain::db_schema::RecoverySettlementReason::RetiredToolCall,
                ),
            }
        );

        db.add_message(
            "later-user",
            "settled",
            &MessageContent::user("continue"),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            db.get_recovery_tail_status("settled").await.unwrap(),
            phoenix_core::domain::db_schema::RecoveryTailStatus::Tail {
                message_id: "later-user".to_string(),
                settlement: None,
            }
        );
    }

    #[tokio::test]
    async fn recovery_messages_start_at_newest_user_or_skill_boundary() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("recovery-suffix", "slug", "/tmp", true, None, None)
            .await
            .unwrap();
        let contents = [
            MessageContent::user("old user"),
            MessageContent::System(SystemContent {
                text: "old system".into(),
            }),
            MessageContent::user("new user"),
            MessageContent::System(SystemContent {
                text: "tail system".into(),
            }),
        ];
        for (index, content) in contents.iter().enumerate() {
            db.add_message(
                &format!("recovery-message-{index}"),
                "recovery-suffix",
                content,
                None,
                None,
            )
            .await
            .unwrap();
        }

        let messages = db.get_recovery_messages("recovery-suffix").await.unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.sequence_id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    #[tokio::test]
    async fn recovery_messages_include_the_complete_adopted_wake_tail() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("recovery-wake-tail", "slug", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "wake-boundary",
            "recovery-wake-tail",
            &MessageContent::user("ordinary"),
            None,
            None,
        )
        .await
        .unwrap();
        for (index, terminal) in ["Completed", "Cancelled"].iter().enumerate() {
            db.add_message(
                &format!("wake-result-{index}"),
                "recovery-wake-tail",
                &MessageContent::user("wake"),
                Some(&serde_json::json!({
                    "type": "wake_result",
                    "adopted": true,
                    "terminal": { (*terminal): {} },
                })),
                None,
            )
            .await
            .unwrap();
        }

        let messages = db
            .get_recovery_messages("recovery-wake-tail")
            .await
            .unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.sequence_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[tokio::test]
    async fn creation_claim_has_one_winner_and_fences_late_results() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-claim", "conv-claim").await;
        let now = Utc::now();
        let first = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(first_job) = first else {
            panic!("first worker must claim accepted job");
        };
        let CreationStatus::Claimed(first_claim) = first_job.protocol.status else {
            panic!("claimed job must carry authority");
        };

        let concurrent = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                now,
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        assert!(matches!(concurrent, CreationClaimOutcome::NoEligibleJob));

        let takeover = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                now + chrono::Duration::seconds(11),
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(second_job) = takeover else {
            panic!("expired claim must be recoverable");
        };
        let CreationStatus::Claimed(second_claim) = second_job.protocol.status else {
            panic!("replacement must carry authority");
        };
        assert_eq!(first_claim.generation + 1, second_claim.generation);
        assert_eq!(second_job.protocol.attempt, 1, "takeover is not a retry");

        let stale_failure = db
            .fail_conversation_creation_job(
                "job-claim",
                &first_claim,
                "late failure",
                &ErrorKind::ServerError,
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap();
        assert_eq!(stale_failure, CreationCasOutcome::ClaimLost);

        let completed = db
            .complete_conversation_creation_job(
                "job-claim",
                &second_claim,
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap();
        assert_eq!(completed, CreationCasOutcome::Applied);
        let late_completion = db
            .complete_conversation_creation_job(
                "job-claim",
                &first_claim,
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap();
        assert_eq!(late_completion, CreationCasOutcome::ClaimLost);
        assert!(matches!(
            db.get_conversation_creation_job("job-claim")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Ready
        ));
    }

    async fn setup_runtime_settlement_job(db: &Database) -> (CreationClaim, DateTime<Utc>) {
        insert_test_creation_job(db, "job-runtime-settle", "conv-runtime-settle").await;
        db.update_conversation_state(
            "conv-runtime-settle",
            &ConvState::Provisioning {
                job_id: "job-runtime-settle".to_string(),
                phase: ConversationCreationPhase::Provisioning,
            },
        )
        .await
        .unwrap();
        let now = Utc::now();
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker".into()),
                &CreationClaimToken("token".into()),
                now,
                chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected creation claim");
        };
        let CreationStatus::Claimed(claim) = job.protocol.status else {
            panic!("expected claim authority");
        };
        (claim, now)
    }

    #[tokio::test]
    async fn stale_creation_claim_cannot_materialize_message_state_or_job() {
        let db = Database::open_in_memory().await.unwrap();
        let (claim, now) = setup_runtime_settlement_job(&db).await;
        assert_eq!(
            db.schedule_conversation_creation_retry(
                "job-runtime-settle",
                &claim,
                "claim revoked",
                now,
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );

        assert!(matches!(
            db.materialize_conversation_creation_runtime(
                "job-runtime-settle",
                &claim,
                "conv-runtime-settle",
                "stale-message",
                |_| panic!("stale claim must not allocate an SSE sequence"),
                &MessageContent::user("stale"),
                None,
                None,
                &ConvState::LlmRequesting { attempt: 1 },
                now,
            )
            .await
            .unwrap(),
            CreationRuntimeMaterialization::ClaimLost
        ));
        assert!(db
            .get_messages("conv-runtime-settle")
            .await
            .unwrap()
            .is_empty());
        assert!(matches!(
            db.get_conversation_creation_job("job-runtime-settle")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::RetryScheduled { .. }
        ));
        assert!(matches!(
            db.get_conversation("conv-runtime-settle")
                .await
                .unwrap()
                .state,
            ConvState::Provisioning { .. }
        ));
    }

    #[tokio::test]
    async fn creation_runtime_materialization_allocates_above_sequence_floor() {
        let db = Database::open_in_memory().await.unwrap();
        let (claim, now) = setup_runtime_settlement_job(&db).await;
        let requesting = ConvState::LlmRequesting { attempt: 1 };

        let materialized = db
            .materialize_conversation_creation_runtime(
                "job-runtime-settle",
                &claim,
                "conv-runtime-settle",
                "initial-message",
                |persisted_sequence_max| persisted_sequence_max.max(42) + 1,
                &MessageContent::user("initial"),
                None,
                None,
                &requesting,
                now,
            )
            .await
            .unwrap();
        let CreationRuntimeMaterialization::Materialized(message) = materialized else {
            panic!("current claim must materialize creation runtime");
        };

        assert_eq!(message.sequence_id, 43);
        assert_eq!(
            db.get_messages("conv-runtime-settle").await.unwrap()[0].sequence_id,
            43
        );
        assert!(matches!(
            db.get_conversation_creation_job("job-runtime-settle")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Ready
        ));
        assert_eq!(
            db.get_conversation("conv-runtime-settle")
                .await
                .unwrap()
                .state,
            requesting
        );
    }

    #[tokio::test]
    async fn creation_runtime_settlement_rolls_back_job_when_state_write_fails() {
        let db = Database::open_in_memory().await.unwrap();
        let (claim, now) = setup_runtime_settlement_job(&db).await;
        sqlx::query(
            "CREATE TEMP TRIGGER fail_creation_runtime_state
             BEFORE UPDATE OF state ON conversations
             WHEN OLD.id = 'conv-runtime-settle'
             BEGIN SELECT RAISE(ABORT, 'injected state failure'); END",
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.settle_conversation_creation_runtime(
            "job-runtime-settle",
            &claim,
            "conv-runtime-settle",
            &ConvState::LlmRequesting { attempt: 1 },
            now,
        )
        .await
        .unwrap_err();

        assert!(matches!(
            db.get_conversation_creation_job("job-runtime-settle")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Claimed(_)
        ));
        assert!(matches!(
            db.get_conversation("conv-runtime-settle")
                .await
                .unwrap()
                .state,
            ConvState::Provisioning { .. }
        ));
    }

    #[tokio::test]
    async fn hidden_repository_default_branch_observation_fences_stale_updates_and_bumps_generation(
    ) {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "conv-branch-fence",
            "conv-branch-fence",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let first_observed = Utc::now() - chrono::Duration::minutes(5);
        let attachment = attach_hidden_repository_for_test(
            &db,
            "conv-branch-fence",
            "/tmp/.git/worktrees/fence",
            "/tmp/.git/worktrees/fence",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            first_observed,
        )
        .await;
        let initial: (i64, String, i64) = sqlx::query_as(
            "SELECT generation, branch, observed_at_unix_micros
             FROM git_repository_default_branch_observations WHERE repository_id = ?1",
        )
        .bind(attachment.repository_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(initial.0, 1);
        assert_eq!(initial.1, "main");

        let stale = attach_hidden_repository_for_test(
            &db,
            "conv-branch-fence",
            "/tmp/.git/worktrees/fence",
            "/tmp/.git/worktrees/fence",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "stale-branch".to_string(),
                provenance: "user_selected".to_string(),
            },
            first_observed - chrono::Duration::minutes(1),
        )
        .await;
        assert_eq!(stale.repository_id, attachment.repository_id);
        let after_stale: (i64, String, String, i64) = sqlx::query_as(
            "SELECT generation, branch, provenance, observed_at_unix_micros
             FROM git_repository_default_branch_observations WHERE repository_id = ?1",
        )
        .bind(attachment.repository_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(after_stale.0, 1);
        assert_eq!(after_stale.1, "main");
        assert_eq!(after_stale.2, "remote_head_cache");
        assert_eq!(after_stale.3, initial.2);

        attach_hidden_repository_for_test(
            &db,
            "conv-branch-fence",
            "/tmp/.git/worktrees/fence",
            "/tmp/.git/worktrees/fence",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "develop".to_string(),
                provenance: "user_selected".to_string(),
            },
            first_observed + chrono::Duration::minutes(1),
        )
        .await;
        let updated: (i64, String, String, i64) = sqlx::query_as(
            "SELECT generation, branch, provenance, observed_at_unix_micros
             FROM git_repository_default_branch_observations WHERE repository_id = ?1",
        )
        .bind(attachment.repository_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(updated.0, 2);
        assert_eq!(updated.1, "develop");
        assert_eq!(updated.2, "user_selected");
        assert!(updated.3 > initial.2);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn hidden_repository_management_roots_break_ties_by_latest_ordinary_activity() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("repo-a-open", "repo-a-open", "/tmp/a", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "repo-a-history",
            "repo-a-history",
            "/tmp/a2",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        db.create_conversation("repo-b-open", "repo-b-open", "/tmp/b", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "repo-b-history",
            "repo-b-history",
            "/tmp/b2",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        attach_hidden_repository_for_test(
            &db,
            "repo-a-open",
            "/tmp/.git/worktrees/repo-a",
            "/tmp/.git/worktrees/repo-a",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now() - chrono::Duration::minutes(10),
        )
        .await;
        let repo_a = attach_hidden_repository_for_test(
            &db,
            "repo-a-history",
            "/tmp/.git/worktrees/repo-a",
            "/tmp/.git/worktrees/repo-a",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now() - chrono::Duration::minutes(10),
        )
        .await;
        attach_hidden_repository_for_test(
            &db,
            "repo-b-open",
            "/tmp/.git/worktrees/repo-b",
            "/tmp/.git/worktrees/repo-b",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now() - chrono::Duration::minutes(10),
        )
        .await;
        let repo_b = attach_hidden_repository_for_test(
            &db,
            "repo-b-history",
            "/tmp/.git/worktrees/repo-b",
            "/tmp/.git/worktrees/repo-b",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now() - chrono::Duration::minutes(10),
        )
        .await;
        assert_ne!(repo_a.repository_id, repo_b.repository_id);

        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = (SELECT product_conversation_id FROM conversations WHERE id = 'repo-a-history')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = (SELECT product_conversation_id FROM conversations WHERE id = 'repo-b-history')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET updated_at = '2026-01-01T00:00:00Z', state_updated_at = '2026-01-01T00:00:00Z' WHERE id IN ('repo-a-open', 'repo-a-history')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET updated_at = '2026-02-01T00:00:00Z', state_updated_at = '2026-02-01T00:00:00Z' WHERE id IN ('repo-b-open', 'repo-b-history')")
            .execute(db.pool())
            .await
            .unwrap();

        let roots = db
            .list_recent_hidden_repository_management_roots()
            .await
            .unwrap();
        let ranked: Vec<_> = roots
            .iter()
            .filter(|root| {
                root.repository_id == repo_a.repository_id
                    || root.repository_id == repo_b.repository_id
            })
            .collect();
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].repository_id, repo_b.repository_id);
        assert_eq!(ranked[1].repository_id, repo_a.repository_id);
    }

    #[tokio::test]
    async fn recent_hidden_repository_roots_include_unresolved_consumed_pre_scope_reservations() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-unresolved-recent-root".to_string(),
            cwd: "/tmp/unresolved-recent-root".to_string(),
            kind: "unresolved_exact_committed_tree".to_string(),
            repo_root: Some("/repo/unresolved-recent-root".to_string()),
            repository_id: Some("repo-id-unresolved-recent-root".to_string()),
            exact_checkout_oid: None,
            logical_base: None,
            freshness: Some("unresolved".to_string()),
            unresolved_reason: Some("no_merge_base".to_string()),
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();
        let job = InsertConversationCreationJob {
            id: "job-unresolved-recent-root".to_string(),
            conversation_id: "conv-unresolved-recent-root".to_string(),
            message_id: Some("message-unresolved-recent-root".to_string()),
            intent: test_creation_intent(
                "/tmp/unresolved-recent-root",
                "message-unresolved-recent-root",
            ),
        };
        db.create_conversation_with_creation_job(
            "conv-unresolved-recent-root",
            "conv-unresolved-recent-root",
            "/tmp/unresolved-recent-root",
            true,
            None,
            &ConvMode::Direct,
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
            &job,
            Some(&reservation.id),
        )
        .await
        .unwrap();
        let repo = attach_hidden_repository_for_test(
            &db,
            "conv-unresolved-recent-root",
            "/tmp/.git/worktrees/unresolved-recent-root",
            "/repo/unresolved-recent-root",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now(),
        )
        .await;
        sqlx::query("UPDATE conversations SET work_scope_id = NULL, updated_at = '2026-03-01T00:00:00Z', state_updated_at = '2026-03-01T00:00:00Z' WHERE id = 'conv-unresolved-recent-root'")
            .execute(db.pool())
            .await
            .unwrap();

        let roots = db
            .list_recent_hidden_repository_management_roots()
            .await
            .unwrap();
        assert!(roots
            .iter()
            .any(|root| root.repository_id == repo.repository_id
                && root.management_root == "/repo/unresolved-recent-root"));
    }

    #[tokio::test]
    async fn product_root_reservation_reclaims_abandoned_reserved_rows_on_read_and_write() {
        let db = Database::open_in_memory().await.unwrap();
        let reclaim_before = product_root_reservation_reclaim_before(Utc::now()) - 1;
        sqlx::query(
            "INSERT INTO product_root_reservations (
                id, cwd, kind, repo_root, exact_checkout_oid, logical_base, freshness, status,
                consumed_by_conversation_id, created_at_unix_micros, consumed_at_unix_micros
             ) VALUES (?1, ?2, 'direct', NULL, NULL, NULL, NULL, 'reserved', NULL, ?3, NULL)",
        )
        .bind("stale-reservation")
        .bind("/tmp/stale-reservation")
        .bind(reclaim_before)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(db
            .get_product_root_reservation("stale-reservation", "/tmp/stale-reservation")
            .await
            .unwrap()
            .is_none());
        let count_after_read: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_root_reservations WHERE id = 'stale-reservation'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count_after_read, 0);

        sqlx::query(
            "INSERT INTO product_root_reservations (
                id, cwd, kind, repo_root, exact_checkout_oid, logical_base, freshness, status,
                consumed_by_conversation_id, created_at_unix_micros, consumed_at_unix_micros
             ) VALUES (?1, ?2, 'direct', NULL, NULL, NULL, NULL, 'reserved', NULL, ?3, NULL)",
        )
        .bind("stale-reservation-write")
        .bind("/tmp/stale-reservation-write")
        .bind(reclaim_before)
        .execute(db.pool())
        .await
        .unwrap();
        db.insert_product_root_reservation(&ProductRootReservationRecord {
            id: "fresh-reservation".to_string(),
            cwd: "/tmp/fresh-reservation".to_string(),
            kind: "direct".to_string(),
            repo_root: None,
            repository_id: None,
            exact_checkout_oid: None,
            logical_base: None,
            freshness: None,
            unresolved_reason: None,
        })
        .await
        .unwrap();
        let count_after_write: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_root_reservations WHERE id = 'stale-reservation-write'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count_after_write, 0);
    }

    #[tokio::test]
    async fn consumed_product_root_reservation_is_deleted_with_hard_deleted_conversation() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-delete".to_string(),
            cwd: "/tmp/reservation-delete".to_string(),
            kind: "direct".to_string(),
            repo_root: None,
            repository_id: None,
            exact_checkout_oid: None,
            logical_base: None,
            freshness: None,
            unresolved_reason: None,
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();
        let job = InsertConversationCreationJob {
            id: "job-reservation-delete".to_string(),
            conversation_id: "conv-reservation-delete".to_string(),
            message_id: Some("message-reservation-delete".to_string()),
            intent: test_creation_intent("/tmp/reservation-delete", "message-reservation-delete"),
        };
        db.create_conversation_with_creation_job(
            "conv-reservation-delete",
            "conv-reservation-delete",
            "/tmp/reservation-delete",
            true,
            None,
            &ConvMode::Direct,
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
            &job,
            Some(&reservation.id),
        )
        .await
        .unwrap();
        db.delete_conversation("conv-reservation-delete")
            .await
            .unwrap();
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_root_reservations WHERE id = ?1")
                .bind(&reservation.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn attach_hidden_repository_allocates_deferred_work_scope_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-deferred-scope".to_string(),
            cwd: "/tmp/deferred-scope".to_string(),
            kind: "exact_committed_tree".to_string(),
            repo_root: Some("/repo/deferred-scope".to_string()),
            repository_id: Some("repo-id-deferred-scope".to_string()),
            exact_checkout_oid: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()),
            logical_base: Some("main".to_string()),
            freshness: Some("fresh".to_string()),
            unresolved_reason: None,
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();
        let job = InsertConversationCreationJob {
            id: "job-deferred-scope".to_string(),
            conversation_id: "conv-deferred-scope".to_string(),
            message_id: Some("message-deferred-scope".to_string()),
            intent: test_creation_intent("/tmp/deferred-scope", "message-deferred-scope"),
        };
        let created = db
            .create_conversation_with_creation_job(
                "conv-deferred-scope",
                "conv-deferred-scope",
                "/tmp/deferred-scope",
                true,
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
                &job,
                Some(&reservation.id),
            )
            .await
            .unwrap();
        assert_eq!(created.attached_work_scope_id, None);

        let attachment = attach_hidden_repository_for_test(
            &db,
            "conv-deferred-scope",
            "/tmp/.git/worktrees/deferred-scope",
            "/repo/deferred-scope",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now(),
        )
        .await;
        let conversation = db.get_conversation("conv-deferred-scope").await.unwrap();
        let scope_id = conversation
            .attached_work_scope_id
            .clone()
            .expect("scope allocated at attachment time");
        assert_eq!(scope_id, attachment.work_scope_id);
        let attachments = db
            .conversation_work_scope_attachments(&scope_id)
            .await
            .unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].id, "conv-deferred-scope");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn approval_handoff_uses_consumed_reservation_as_immutable_root_evidence() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-immutable-root".to_string(),
            cwd: "/tmp/immutable-root".to_string(),
            kind: "exact_committed_tree".to_string(),
            repo_root: Some("/repo/from-reservation".to_string()),
            repository_id: Some("repo-id-from-reservation".to_string()),
            exact_checkout_oid: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            logical_base: Some("main".to_string()),
            freshness: Some("fresh".to_string()),
            unresolved_reason: None,
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();
        let job = InsertConversationCreationJob {
            id: "job-immutable-root".to_string(),
            conversation_id: "conv-immutable-root".to_string(),
            message_id: Some("message-immutable-root".to_string()),
            intent: test_creation_intent("/tmp/immutable-root", "message-immutable-root"),
        };
        db.create_conversation_with_creation_job(
            "conv-immutable-root",
            "conv-immutable-root",
            "/tmp/immutable-root",
            true,
            None,
            &ConvMode::Direct,
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
            &job,
            Some(&reservation.id),
        )
        .await
        .unwrap();
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-immutable-root".into()),
                &CreationClaimToken("token-immutable-root".into()),
                Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(claimed_job) = claimed else {
            panic!("claim immutable-root job");
        };
        let CreationStatus::Claimed(claim) = claimed_job.protocol.status else {
            panic!("creation claim authority");
        };
        db.update_conversation_creation_metadata_and_mode(
            &job.id,
            &claim,
            "conv-immutable-root",
            &ConversationCreationMetadataUpdate {
                cwd: Some("/tmp/immutable-root".to_string()),
                ..Default::default()
            },
            &ConvMode::Direct,
            "test-model",
            CreationStage::ValidateIntent,
            CreationStage::ResolveRepository,
        )
        .await
        .unwrap();

        let attachment = attach_hidden_repository_for_test(
            &db,
            "conv-immutable-root",
            "/tmp/.git/worktrees/immutable",
            "/repo/current-management-root",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now() - chrono::Duration::minutes(1),
        )
        .await;
        sqlx::query(
            "UPDATE git_repository_default_branch_observations
                SET branch = 'drifted-base', provenance = 'user_selected'
              WHERE repository_id = ?1",
        )
        .bind(attachment.repository_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE git_repository_locator_observations
                SET path = '/repo/drifted-management-root'
              WHERE repository_id = ?1 AND locator_kind = 'management_root'",
        )
        .bind(attachment.repository_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();

        let root = db
            .root_reservation_for_attached_hidden_repository("conv-immutable-root")
            .await
            .unwrap()
            .expect("consumed reservation evidence");
        assert_eq!(root.repository_id.as_str(), "repo-id-from-reservation");
        assert_eq!(root.repository_root, "/repo/from-reservation");
        assert_eq!(root.logical_base, "main");
        assert_eq!(
            root.exact_checkout_oid,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn approval_handoff_from_continuation_transcript_uses_shared_product_evidence() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-continuation-approval".to_string(),
            cwd: "/tmp/continuation-approval".to_string(),
            kind: "exact_committed_tree".to_string(),
            repo_root: Some("/repo/continuation-approval".to_string()),
            repository_id: Some("repo-id-continuation-approval".to_string()),
            exact_checkout_oid: Some("cccccccccccccccccccccccccccccccccccccccc".to_string()),
            logical_base: Some("main".to_string()),
            freshness: Some("fresh".to_string()),
            unresolved_reason: None,
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();
        let job = InsertConversationCreationJob {
            id: "job-continuation-approval".to_string(),
            conversation_id: "conv-continuation-root".to_string(),
            message_id: Some("message-continuation-approval".to_string()),
            intent: test_creation_intent(
                "/tmp/continuation-approval",
                "message-continuation-approval",
            ),
        };
        db.create_conversation_with_creation_job(
            "conv-continuation-root",
            "conv-continuation-root",
            "/tmp/continuation-approval",
            true,
            None,
            &ConvMode::Direct,
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
            &job,
            Some(&reservation.id),
        )
        .await
        .unwrap();
        attach_hidden_repository_for_test(
            &db,
            "conv-continuation-root",
            "/tmp/.git/worktrees/continuation-approval",
            "/repo/continuation-approval",
            GitRepositoryDefaultBranchObservation::Resolved {
                branch: "main".to_string(),
                provenance: "remote_head_cache".to_string(),
            },
            Utc::now(),
        )
        .await;

        db.update_conversation_state(
            "conv-continuation-root",
            &ConvState::ContextExhausted {
                summary: "continued approval evidence".to_string(),
            },
        )
        .await
        .unwrap();

        let (outcome, _) = db
            .continue_conversation_with_intent(
                "conv-continuation-root",
                NewContinuationDispatchIntent {
                    message_id: "message-continuation-successor".to_string(),
                    handoff: "continued approval".to_string(),
                    user_agent: Some("test-agent".to_string()),
                },
            )
            .await
            .unwrap();
        let ContinueOutcome::Created(successor) = outcome else {
            panic!("expected created continuation");
        };
        let approval = phoenix_core::task_handoff::TaskApprovalHandoffData {
            task_id: "27099".to_string(),
            task_title: "Continued Approval".to_string(),
            branch_name: "task-27099-continued-approval".to_string(),
            approved_commit_oid: "cccccccccccccccccccccccccccccccccccccccc".to_string(),
            worktree_path: "/ignored".to_string(),
            base_branch: "main".to_string(),
            title: "Continued Approval".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "reviewed plan from continuation".to_string(),
            task_file: "tasks/27099-p1-ready--continued-approval.md".to_string(),
        };

        let handoff = db
            .create_task_approval_handoff_creation_job(&successor.id, &approval)
            .await
            .unwrap();
        let job = db
            .get_conversation_creation_job_for_conversation(&handoff.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            job.intent.reserved_repo_root.as_deref(),
            Some("/repo/continuation-approval")
        );
        assert_eq!(
            job.intent.reserved_checkout_oid.as_deref(),
            Some("cccccccccccccccccccccccccccccccccccccccc")
        );
        assert_eq!(
            job.intent.approved_task.as_ref().map(|s| s.plan.as_str()),
            Some("reviewed plan from continuation")
        );
    }

    #[tokio::test]
    async fn root_reservation_consumption_rolls_back_when_conversation_insert_fails() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-rollback".to_string(),
            cwd: "/tmp/reservation-rollback".to_string(),
            kind: "direct".to_string(),
            repo_root: None,
            repository_id: None,
            exact_checkout_oid: None,
            logical_base: None,
            freshness: None,
            unresolved_reason: None,
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();
        insert_test_creation_job(&db, "job-existing-conv", "existing-conv").await;

        let duplicate = InsertConversationCreationJob {
            id: "job-duplicate-conversation".to_string(),
            conversation_id: "existing-conv".to_string(),
            message_id: Some("message-duplicate-conversation".to_string()),
            intent: test_creation_intent(
                "/tmp/reservation-rollback",
                "message-duplicate-conversation",
            ),
        };
        let error = db
            .create_conversation_with_creation_job(
                "existing-conv",
                "existing-conv",
                "/tmp/reservation-rollback",
                true,
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
                &duplicate,
                Some(&reservation.id),
            )
            .await
            .expect_err("duplicate conversation id must abort the transaction");
        assert!(matches!(error, DbError::ConversationAlreadyExists(_)));

        let status: (String, Option<String>) = sqlx::query_as(
            "SELECT status, consumed_by_conversation_id
             FROM product_root_reservations WHERE id = ?1",
        )
        .bind(&reservation.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(status, ("reserved".to_string(), None));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn root_reservation_accepts_same_conversation_idempotently_but_rejects_another() {
        let db = Database::open_in_memory().await.unwrap();
        let reservation = ProductRootReservationRecord {
            id: "reservation-idempotent".to_string(),
            cwd: "/tmp/reservation-idempotent".to_string(),
            kind: "direct".to_string(),
            repo_root: None,
            repository_id: None,
            exact_checkout_oid: None,
            logical_base: None,
            freshness: None,
            unresolved_reason: None,
        };
        db.insert_product_root_reservation(&reservation)
            .await
            .unwrap();

        let duplicate_job = InsertConversationCreationJob {
            id: "job-idempotent".to_string(),
            conversation_id: "conv-idempotent".to_string(),
            message_id: Some("message-idempotent".to_string()),
            intent: test_creation_intent("/tmp/reservation-idempotent", "message-idempotent"),
        };
        let duplicate_conversation = db
            .create_conversation_with_creation_job(
                "conv-idempotent",
                "idempotent",
                "/tmp/reservation-idempotent",
                true,
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
                &duplicate_job,
                Some(&reservation.id),
            )
            .await
            .unwrap();
        assert_eq!(duplicate_conversation.id, "conv-idempotent");

        let consumed_at_before: Option<i64> = sqlx::query_scalar(
            "SELECT consumed_at_unix_micros FROM product_root_reservations WHERE id = ?1",
        )
        .bind(&reservation.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(consumed_at_before.is_some());

        sqlx::query(
            "UPDATE product_root_reservations
             SET consumed_at_unix_micros = 42
             WHERE id = ?1",
        )
        .bind(&reservation.id)
        .execute(db.pool())
        .await
        .unwrap();

        let duplicate_error = db
            .create_conversation_with_creation_job(
                "conv-idempotent",
                "idempotent",
                "/tmp/reservation-idempotent",
                true,
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
                &duplicate_job,
                Some(&reservation.id),
            )
            .await
            .expect_err("same conversation id re-create still conflicts after idempotent reservation acceptance");
        assert!(matches!(
            duplicate_error,
            DbError::ConversationAlreadyExists(_)
        ));

        let consumed_at_after: i64 = sqlx::query_scalar(
            "SELECT consumed_at_unix_micros FROM product_root_reservations WHERE id = ?1",
        )
        .bind(&reservation.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            consumed_at_after, 42,
            "same-conversation acceptance must not rewrite consumption metadata"
        );

        let another_job = InsertConversationCreationJob {
            id: "job-other-conversation".to_string(),
            conversation_id: "conv-other".to_string(),
            message_id: Some("message-other".to_string()),
            intent: test_creation_intent("/tmp/reservation-idempotent", "message-other"),
        };
        let another_error = db
            .create_conversation_with_creation_job(
                "conv-other",
                "other",
                "/tmp/reservation-idempotent",
                true,
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
                &another_job,
                Some(&reservation.id),
            )
            .await
            .expect_err("different conversation must not reuse consumed reservation");
        let DbError::Serialization(message) = another_error else {
            panic!("expected reservation ownership error");
        };
        assert!(message.contains("belongs to another conversation"));
    }

    #[tokio::test]
    async fn creation_runtime_retry_reclaims_before_committing_dispatchable_state() {
        let db = Database::open_in_memory().await.unwrap();
        let (claim, now) = setup_runtime_settlement_job(&db).await;
        let retry_at = now + chrono::Duration::seconds(1);
        assert_eq!(
            db.schedule_conversation_creation_retry(
                "job-runtime-settle",
                &claim,
                "injected completion failure",
                now,
                retry_at,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        assert!(matches!(
            db.get_conversation_creation_job("job-runtime-settle")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::RetryScheduled { .. }
        ));
        assert!(matches!(
            db.get_conversation("conv-runtime-settle")
                .await
                .unwrap()
                .state,
            ConvState::Provisioning { .. }
        ));

        let reclaimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("replacement".into()),
                &CreationClaimToken("replacement-token".into()),
                retry_at,
                chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(reclaimed_job) = reclaimed else {
            panic!("retry must be reclaimable");
        };
        let CreationStatus::Claimed(reclaimed_claim) = reclaimed_job.protocol.status else {
            panic!("replacement must own current claim");
        };
        let requesting = ConvState::LlmRequesting { attempt: 1 };
        assert_eq!(
            db.settle_conversation_creation_runtime(
                "job-runtime-settle",
                &reclaimed_claim,
                "conv-runtime-settle",
                &requesting,
                retry_at,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        assert!(matches!(
            db.get_conversation_creation_job("job-runtime-settle")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Ready
        ));
        assert_eq!(
            db.get_conversation("conv-runtime-settle")
                .await
                .unwrap()
                .state,
            requesting
        );
    }

    #[tokio::test]
    async fn creation_stage_checkpoint_rejects_stale_generation() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-stage", "conv-stage").await;
        let now = Utc::now();
        let first = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(first_job) = first else {
            panic!("expected first claim");
        };
        let CreationStatus::Claimed(first_claim) = first_job.protocol.status else {
            panic!("expected first authority");
        };
        assert_eq!(
            db.advance_conversation_creation_stage(
                "job-stage",
                &first_claim,
                CreationStage::ValidateIntent,
                CreationStage::ResolveRepository,
                now,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        let takeover = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                now + chrono::Duration::seconds(11),
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(second_job) = takeover else {
            panic!("expected replacement claim");
        };
        let CreationStatus::Claimed(second_claim) = second_job.protocol.status else {
            panic!("expected replacement authority");
        };
        assert_eq!(second_job.protocol.stage, CreationStage::ResolveRepository);
        assert_eq!(
            db.advance_conversation_creation_stage(
                "job-stage",
                &first_claim,
                CreationStage::ResolveRepository,
                CreationStage::ReserveResources,
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap(),
            CreationCasOutcome::ClaimLost
        );
        assert_eq!(
            db.advance_conversation_creation_stage(
                "job-stage",
                &second_claim,
                CreationStage::ResolveRepository,
                CreationStage::ReserveResources,
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
    }

    #[tokio::test]
    async fn failed_creation_reconciles_resources_without_deleting_record() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-failed-cleanup", "conv-failed-cleanup").await;
        let now = Utc::now();
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected claim");
        };
        let CreationStatus::Claimed(claim) = job.protocol.status else {
            panic!("expected authority");
        };
        db.reserve_conversation_creation_resource(
            "reservation-failed",
            "job-failed-cleanup",
            &claim,
            "/repo",
            "/repo/worktree",
            now,
        )
        .await
        .unwrap();
        let takeover_at = now + chrono::Duration::seconds(31);
        let replacement = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                takeover_at,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(replacement_job) = replacement else {
            panic!("expected replacement claim");
        };
        let CreationStatus::Claimed(replacement_claim) = replacement_job.protocol.status else {
            panic!("expected replacement authority");
        };
        assert!(replacement_claim.generation > claim.generation);
        assert_eq!(
            db.fail_conversation_creation_job(
                "job-failed-cleanup",
                &replacement_claim,
                "permanent failure",
                &ErrorKind::InvalidRequest,
                now,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        assert!(matches!(
            db.get_conversation("conv-failed-cleanup")
                .await
                .unwrap()
                .state,
            ConvState::CreationFailed {
                ref job_id,
                ref error,
                error_kind: ErrorKind::InvalidRequest,
            } if job_id == "job-failed-cleanup" && error == "permanent failure"
        ));
        let reservation = db
            .get_creation_resource_reservations("job-failed-cleanup")
            .await
            .unwrap()
            .pop()
            .expect("reservation");
        assert_eq!(reservation.generation, replacement_claim.generation);
        let cleanup = claim_test_cleanup(&db).await;
        assert_eq!(cleanup.status, "failed");
        assert_eq!(
            db.release_creation_resource(&cleanup, "reservation-failed", Utc::now())
                .await
                .unwrap(),
            CreationCasOutcome::Applied
        );
        db.finish_conversation_creation_cleanup(&cleanup, now)
            .await
            .unwrap();
        assert!(db.get_conversation("conv-failed-cleanup").await.is_ok());
        assert!(matches!(
            db.get_conversation_creation_job("job-failed-cleanup")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Failed(_)
        ));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn creation_cancel_and_delete_revoke_claim_before_cleanup() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-cancel", "conv-cancel").await;
        let now = Utc::now();
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected claim");
        };
        let CreationStatus::Claimed(claim) = job.protocol.status else {
            panic!("expected authority");
        };
        db.reserve_conversation_creation_resource(
            "reservation-cancel",
            "job-cancel",
            &claim,
            "/repo",
            "/repo/worktree",
            now,
        )
        .await
        .unwrap();

        db.cancel_conversation_creation("conv-cancel", now)
            .await
            .unwrap();
        assert_eq!(
            db.fail_conversation_creation_job(
                "job-cancel",
                &claim,
                "late",
                &ErrorKind::ServerError,
                now,
            )
            .await
            .unwrap(),
            CreationCasOutcome::ClaimLost
        );
        let cleanup = claim_test_cleanup(&db).await;
        assert_eq!(cleanup.status, "cancelling");
        assert!(matches!(
            db.get_conversation("conv-cancel").await.unwrap().state,
            ConvState::CreationCancelled { .. }
        ));
        assert_eq!(
            db.release_creation_resource(&cleanup, "reservation-cancel", Utc::now())
                .await
                .unwrap(),
            CreationCasOutcome::Applied
        );
        db.finish_conversation_creation_cleanup(&cleanup, now)
            .await
            .unwrap();
        assert!(matches!(
            db.get_conversation_creation_job("job-cancel")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Cancelled
        ));

        let conversation_before_delete = db.get_conversation("conv-cancel").await.unwrap();
        let product_conversation_id = conversation_before_delete.product_conversation_id;
        let work_scope_id = conversation_before_delete
            .attached_work_scope_id
            .expect("creation shell has a work scope");
        db.add_message(
            "creation-delete-indexed-message",
            "conv-cancel",
            &MessageContent::user("index this before deletion"),
            None,
            None,
        )
        .await
        .unwrap();
        let indexed_before_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE conversation_id = ?1")
                .bind("conv-cancel")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(indexed_before_delete, 1);

        db.request_conversation_creation_deletion("conv-cancel", now)
            .await
            .unwrap();
        assert!(db.get_conversation("conv-cancel").await.unwrap().archived);
        let deletion = claim_test_cleanup(&db).await;
        assert_eq!(deletion.status, "deletion_pending");
        db.finish_conversation_creation_cleanup(&deletion, now)
            .await
            .unwrap();
        assert!(db.get_conversation("conv-cancel").await.is_err());
        let indexed_after_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE conversation_id = ?1")
                .bind("conv-cancel")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(indexed_after_delete, 0);
        let owner_after_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations WHERE id = ?1")
                .bind(product_conversation_id.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(owner_after_delete, 0);
        let scope_after_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM work_scopes WHERE id = ?1")
                .bind(work_scope_id.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(scope_after_delete, 0);
    }

    #[tokio::test]
    async fn creation_delete_rejects_ready_job_without_archiving() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-ready-delete", "conv-ready-delete").await;
        sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'ready', completed_at = ?1 WHERE id = ?2",
        )
        .bind(Utc::now().to_rfc3339())
        .bind("job-ready-delete")
        .execute(db.pool())
        .await
        .unwrap();

        let result = db
            .request_conversation_creation_deletion("conv-ready-delete", Utc::now())
            .await;

        assert!(matches!(
            result,
            Err(DbError::Sqlx(sqlx::Error::RowNotFound))
        ));
        assert!(
            !db.get_conversation("conv-ready-delete")
                .await
                .unwrap()
                .archived
        );
    }

    #[tokio::test]
    async fn seeded_empty_completion_is_atomic_and_claim_fenced() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-seeded", "conv-seeded").await;
        let now = Utc::now();
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected claim");
        };
        let CreationStatus::Claimed(claim) = job.protocol.status else {
            panic!("expected authority");
        };

        assert_eq!(
            db.complete_seeded_empty_conversation_creation(
                "job-seeded",
                &claim,
                "conv-seeded",
                now,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        assert!(matches!(
            db.get_conversation("conv-seeded").await.unwrap().state,
            ConvState::Idle
        ));
        assert!(matches!(
            db.get_conversation_creation_job("job-seeded")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Ready
        ));

        insert_test_creation_job(&db, "job-seeded-stale", "conv-seeded-stale").await;
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-stale".into()),
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected stale claim");
        };
        let CreationStatus::Claimed(stale_claim) = job.protocol.status else {
            panic!("expected stale authority");
        };
        db.cancel_conversation_creation("conv-seeded-stale", now)
            .await
            .unwrap();
        assert_eq!(
            db.complete_seeded_empty_conversation_creation(
                "job-seeded-stale",
                &stale_claim,
                "conv-seeded-stale",
                now,
            )
            .await
            .unwrap(),
            CreationCasOutcome::ClaimLost
        );
        assert!(matches!(
            db.get_conversation("conv-seeded-stale")
                .await
                .unwrap()
                .state,
            ConvState::CreationCancelled { .. }
        ));
    }

    #[tokio::test]
    async fn creation_cancel_rejects_ready_job_without_mutating_conversation() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-ready-cancel", "conv-ready-cancel").await;
        sqlx::query(
            "UPDATE conversation_creation_jobs
             SET status = 'ready', completed_at = ?1 WHERE id = ?2",
        )
        .bind(Utc::now().to_rfc3339())
        .bind("job-ready-cancel")
        .execute(db.pool())
        .await
        .unwrap();

        let state_before = db
            .get_conversation("conv-ready-cancel")
            .await
            .unwrap()
            .state;

        let result = db
            .cancel_conversation_creation("conv-ready-cancel", Utc::now())
            .await;

        assert!(matches!(
            result,
            Err(DbError::Sqlx(sqlx::Error::RowNotFound))
        ));
        assert_eq!(
            db.get_conversation("conv-ready-cancel")
                .await
                .unwrap()
                .state,
            state_before
        );
        assert!(matches!(
            db.get_conversation_creation_job("job-ready-cancel")
                .await
                .unwrap()
                .protocol
                .status,
            CreationStatus::Ready
        ));
    }

    #[tokio::test]
    async fn deletion_pending_creation_is_hidden_from_archived_listing() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-hidden-delete", "conv-hidden-delete").await;
        db.request_conversation_creation_deletion("conv-hidden-delete", Utc::now())
            .await
            .unwrap();

        assert!(db
            .list_archived_conversations()
            .await
            .unwrap()
            .iter()
            .all(|conversation| conversation.id != "conv-hidden-delete"));
        assert!(db.get_conversation("conv-hidden-delete").await.is_ok());
    }

    #[tokio::test]
    async fn deletion_pending_creation_is_hidden_from_search_metadata() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-hidden-search", "conv-hidden-search").await;
        db.request_conversation_creation_deletion("conv-hidden-search", Utc::now())
            .await
            .unwrap();

        let metadata = db
            .get_conversation_search_metadata(&["conv-hidden-search".to_string()])
            .await
            .unwrap();

        assert!(!metadata.contains_key("conv-hidden-search"));
    }

    #[tokio::test]
    async fn creation_metadata_mode_and_normalized_environment_commit_together() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-mode-environment", "conv-mode-environment").await;
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-environment".into()),
                &CreationClaimToken("token-environment".into()),
                Utc::now(),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected claim");
        };
        let CreationStatus::Claimed(claim) = job.protocol.status else {
            panic!("expected claim authority");
        };
        let mode = ConvMode::Branch {
            branch_name: NonEmptyString::new("feature/environment").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/worktree-environment").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
        };

        let outcome = db
            .update_conversation_creation_metadata_and_mode(
                "job-mode-environment",
                &claim,
                "conv-mode-environment",
                &ConversationCreationMetadataUpdate {
                    slug: None,
                    title: None,
                    cwd: Some("/tmp/worktree-environment".into()),
                    project_id: None,
                    desired_base_branch: None,
                },
                &mode,
                "test-model",
                CreationStage::ValidateIntent,
                CreationStage::ResolveRepository,
            )
            .await
            .unwrap();
        assert_eq!(outcome, CreationCasOutcome::Applied);

        let persisted: (String, String, String, String, String, String, String) = sqlx::query_as(
            "SELECT c.cm_kind, e.cwd, e.environment_kind, e.worktree_path,
                    e.branch_name, e.base_branch, scope.authority_kind
             FROM conversations c
             JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             JOIN work_scopes scope ON scope.id = c.work_scope_id
             WHERE c.id = 'conv-mode-environment'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            persisted,
            (
                "branch".into(),
                "/tmp/worktree-environment".into(),
                "allocated_worktree".into(),
                "/tmp/worktree-environment".into(),
                "feature/environment".into(),
                "main".into(),
                "work".into(),
            )
        );
    }

    #[tokio::test]
    async fn stale_claim_cannot_commit_creation_metadata() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-stale-metadata", "conv-stale-metadata").await;
        let now = Utc::now();
        let claimed = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(job) = claimed else {
            panic!("expected claim");
        };
        let CreationStatus::Claimed(claim) = job.protocol.status else {
            panic!("expected claim authority");
        };
        db.cancel_conversation_creation("conv-stale-metadata", now)
            .await
            .unwrap();

        let outcome = db
            .update_conversation_creation_metadata_and_mode(
                "job-stale-metadata",
                &claim,
                "conv-stale-metadata",
                &ConversationCreationMetadataUpdate {
                    slug: Some("stale-slug".to_string()),
                    title: Some(Some("stale title".to_string())),
                    cwd: Some("/stale".to_string()),
                    project_id: Some(None),
                    desired_base_branch: Some(None),
                },
                &ConvMode::Direct,
                "stale-model",
                CreationStage::ValidateIntent,
                CreationStage::ResolveRepository,
            )
            .await
            .unwrap();

        assert_eq!(outcome, CreationCasOutcome::ClaimLost);
        let conversation = db.get_conversation("conv-stale-metadata").await.unwrap();
        assert_ne!(conversation.slug.as_deref(), Some("stale-slug"));
        assert_ne!(conversation.cwd, "/stale");
        assert!(matches!(
            conversation.state,
            ConvState::CreationCancelled { .. }
        ));
    }

    #[tokio::test]
    async fn creation_resource_reservation_is_fenced_by_generation() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-resource", "conv-resource").await;
        let now = Utc::now();
        let first = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(first_job) = first else {
            panic!("expected first claim");
        };
        let CreationStatus::Claimed(first_claim) = first_job.protocol.status else {
            panic!("expected claim authority");
        };
        assert_eq!(
            db.reserve_conversation_creation_resource(
                "reservation-1",
                "job-resource",
                &first_claim,
                "/repo",
                "/repo/.phoenix/worktrees/conv-resource",
                now,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        let takeover = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                now + chrono::Duration::seconds(11),
                chrono::Duration::seconds(10),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(second_job) = takeover else {
            panic!("expected takeover");
        };
        let CreationStatus::Claimed(second_claim) = second_job.protocol.status else {
            panic!("expected replacement authority");
        };
        assert_eq!(
            db.mark_creation_resource_present(
                "job-resource",
                &first_claim,
                "/repo/.phoenix/worktrees/conv-resource",
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap(),
            CreationCasOutcome::ClaimLost
        );
        assert_eq!(
            db.reserve_conversation_creation_resource(
                "reservation-2",
                "job-resource",
                &second_claim,
                "/repo",
                "/repo/.phoenix/worktrees/conv-resource",
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        assert_eq!(
            db.mark_creation_resource_present(
                "job-resource",
                &second_claim,
                "/repo/.phoenix/worktrees/conv-resource",
                now + chrono::Duration::seconds(12),
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        let reservations = db
            .get_creation_resource_reservations("job-resource")
            .await
            .unwrap();
        assert_eq!(reservations.len(), 1);
        assert_eq!(reservations[0].generation, second_claim.generation);
        assert_eq!(reservations[0].status, "present");
    }

    #[tokio::test]
    async fn creation_cleanup_retry_clears_lease_and_uses_retry_deadline() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-cleanup-retry", "conv-cleanup-retry").await;
        let now = Utc::now();
        db.cancel_conversation_creation("conv-cleanup-retry", now)
            .await
            .unwrap();
        let cleanup = db
            .claim_next_conversation_creation_cleanup(
                "cleanup-a",
                "cleanup-token-a",
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .expect("cleanup claim");
        let retry_at = now + chrono::Duration::seconds(60);

        assert_eq!(
            db.schedule_conversation_creation_cleanup_retry(&cleanup, retry_at)
                .await
                .unwrap(),
            CreationCasOutcome::Applied
        );
        assert_eq!(
            db.next_conversation_creation_deadline().await.unwrap(),
            Some(retry_at)
        );
        assert!(db
            .claim_next_conversation_creation_cleanup(
                "cleanup-b",
                "cleanup-token-b",
                now + chrono::Duration::seconds(31),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_none());
        assert!(db
            .claim_next_conversation_creation_cleanup(
                "cleanup-b",
                "cleanup-token-b",
                retry_at,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn creation_cleanup_deadline_waits_for_live_cleanup_lease() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-cleanup-deadline", "conv-cleanup-deadline").await;
        let now = Utc::now();
        db.cancel_conversation_creation("conv-cleanup-deadline", now)
            .await
            .unwrap();
        let lease_duration = chrono::Duration::seconds(30);
        let cleanup = db
            .claim_next_conversation_creation_cleanup(
                "cleanup-a",
                "cleanup-token-a",
                now,
                lease_duration,
            )
            .await
            .unwrap()
            .expect("cleanup claim");

        assert_eq!(cleanup.lease_until, now + lease_duration);
        assert_eq!(
            db.next_conversation_creation_deadline().await.unwrap(),
            Some(cleanup.lease_until)
        );
        assert!(db
            .claim_next_conversation_creation_cleanup(
                "cleanup-b",
                "cleanup-token-b",
                now + chrono::Duration::seconds(1),
                lease_duration,
            )
            .await
            .unwrap()
            .is_none());
        assert!(db
            .claim_next_conversation_creation_cleanup(
                "cleanup-b",
                "cleanup-token-b",
                cleanup.lease_until,
                lease_duration,
            )
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn creation_retry_is_durable_and_due_without_a_kick() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-retry", "conv-retry").await;
        let now = Utc::now();
        let first = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-a".into()),
                &CreationClaimToken("token-a".into()),
                now,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(first_job) = first else {
            panic!("expected first claim");
        };
        let CreationStatus::Claimed(first_claim) = first_job.protocol.status else {
            panic!("expected claim authority");
        };
        let retry_at = now + chrono::Duration::seconds(2);
        assert_eq!(
            db.schedule_conversation_creation_retry(
                "job-retry",
                &first_claim,
                "temporary failure",
                now,
                retry_at,
            )
            .await
            .unwrap(),
            CreationCasOutcome::Applied
        );
        assert_eq!(
            db.next_conversation_creation_deadline().await.unwrap(),
            Some(retry_at)
        );
        assert!(matches!(
            db.claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                now + chrono::Duration::seconds(1),
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap(),
            CreationClaimOutcome::NoEligibleJob
        ));
        let second = db
            .claim_next_conversation_creation_job(
                &CreationWorkerId("worker-b".into()),
                &CreationClaimToken("token-b".into()),
                retry_at,
                chrono::Duration::seconds(30),
            )
            .await
            .unwrap();
        let CreationClaimOutcome::Claimed(second_job) = second else {
            panic!("due retry must be claimable");
        };
        assert_eq!(second_job.protocol.attempt, 2);
        assert_eq!(second_job.protocol.generation, 2);
    }

    #[tokio::test]
    async fn migrated_creation_database_reopens_with_rerunnable_schema() {
        let dir = std::env::temp_dir().join(format!(
            "phoenix-db-reopen-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_file = dir.join("reopen.db");
        let db_path = db_file.to_string_lossy().to_string();

        let db = Database::open(&db_path).await.unwrap();
        run_pending_migrations(db.pool()).await.unwrap();
        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversation_creation_jobs)")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(columns.iter().any(|column| column == "status"));
        assert!(!columns.iter().any(|column| column == "phase"));
        drop(db);

        let reopened = Database::open(&db_path)
            .await
            .expect("post-migration database must reopen");
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn restrict_db_permissions_sets_owner_only_and_skips_missing_sidecars() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("phoenix-db-perms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        std::fs::write(&db_path, b"x").unwrap();
        // World-readable to start, so the chmod is observable.
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        // No `-wal`/`-shm` sidecars exist — the helper must not error on them.
        restrict_db_permissions(&db_path.to_string_lossy());

        let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "db file should be owner read/write only");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: the `-wal` sidecar that migrations create after `open`'s
    /// early chmod must still end up 0600 after `restrict_file_permissions`.
    #[cfg(unix)]
    #[tokio::test]
    async fn restrict_file_permissions_tightens_wal_sidecar() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("phoenix-db-wal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        let db_path_str = db_path.to_string_lossy().to_string();

        // `open` connects in WAL mode and runs `run_migrations`, which writes to
        // the DB and so materializes the `-wal`/`-shm` sidecars.
        let db = Database::open(&db_path_str).await.unwrap();
        let wal_path = dir.join("test.db-wal");
        assert!(
            wal_path.exists(),
            "WAL sidecar should exist after migrations"
        );

        // Loosen the sidecar to simulate a permissive umask leaving it
        // group/world-readable, then re-tighten and assert it is owner-only.
        std::fs::set_permissions(&wal_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        db.restrict_file_permissions();

        let wal_mode = std::fs::metadata(&wal_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            wal_mode, 0o600,
            "WAL sidecar should be owner read/write only"
        );
        let db_mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(db_mode, 0o600, "db file should be owner read/write only");

        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn app_setting_roundtrips_through_db() {
        let db = Database::open_in_memory().await.unwrap();

        // Missing key reads back as None.
        assert!(db.get_app_setting("never_set").await.unwrap().is_none());

        // Insert.
        db.set_app_setting("key", "value-1").await.unwrap();
        assert_eq!(
            db.get_app_setting("key").await.unwrap().as_deref(),
            Some("value-1")
        );

        // Upsert overwrites.
        db.set_app_setting("key", "value-2").await.unwrap();
        assert_eq!(
            db.get_app_setting("key").await.unwrap().as_deref(),
            Some("value-2")
        );
    }

    #[tokio::test]
    async fn auth_session_is_valid_after_insert_and_unknown_tokens_are_not() {
        let db = Database::open_in_memory().await.unwrap();
        db.insert_auth_session("tok-a", "fp", chrono::Duration::hours(1))
            .await
            .unwrap();
        assert!(db.is_auth_session_valid("tok-a", "fp").await.unwrap());
        assert!(!db
            .is_auth_session_valid("never-minted", "fp")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn auth_session_rejected_when_password_fingerprint_changes() {
        // A token minted under one password must not authenticate once the
        // configured password (and thus its fingerprint) changes — rotating
        // PHOENIX_PASSWORD invalidates every prior session.
        let db = Database::open_in_memory().await.unwrap();
        db.insert_auth_session("tok", "old-password-fp", chrono::Duration::hours(1))
            .await
            .unwrap();
        assert!(db
            .is_auth_session_valid("tok", "old-password-fp")
            .await
            .unwrap());
        assert!(!db
            .is_auth_session_valid("tok", "new-password-fp")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn auth_session_expiry_is_enforced_and_swept() {
        let db = Database::open_in_memory().await.unwrap();
        // Already-expired token (negative TTL): never authenticates.
        db.insert_auth_session("stale", "fp", chrono::Duration::seconds(-1))
            .await
            .unwrap();
        assert!(!db.is_auth_session_valid("stale", "fp").await.unwrap());

        // A live token alongside it survives the sweep; the expired one is gone.
        db.insert_auth_session("fresh", "fp", chrono::Duration::hours(1))
            .await
            .unwrap();
        db.delete_expired_auth_sessions().await.unwrap();
        assert!(db.is_auth_session_valid("fresh", "fp").await.unwrap());

        // The expired row is physically gone: re-inserting the same token must
        // not collide on the primary key.
        db.insert_auth_session("stale", "fp", chrono::Duration::hours(1))
            .await
            .unwrap();
        assert!(db.is_auth_session_valid("stale", "fp").await.unwrap());
    }

    #[tokio::test]
    async fn mcp_oauth_registration_roundtrips_keyed_by_auth_server() {
        let db = Database::open_in_memory().await.unwrap();

        assert!(db
            .get_mcp_oauth_registration("https://as.example.com")
            .await
            .unwrap()
            .is_none());

        let registration = McpOAuthRegistrationRow {
            auth_server: "https://as.example.com".to_string(),
            client_id: "cid-1".to_string(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_string(),
            redirect_uri: Some("https://phoenix.example/api/mcp/oauth/callback".to_string()),
        };
        db.upsert_mcp_oauth_registration(&registration)
            .await
            .unwrap();
        assert_eq!(
            db.get_mcp_oauth_registration("https://as.example.com")
                .await
                .unwrap(),
            Some(registration.clone())
        );

        // Upsert replaces in place (still one row per authorization server).
        let confidential = McpOAuthRegistrationRow {
            client_secret: Some("sec".to_string()),
            token_endpoint_auth_method: "client_secret_post".to_string(),
            ..registration
        };
        db.upsert_mcp_oauth_registration(&confidential)
            .await
            .unwrap();
        assert_eq!(
            db.get_mcp_oauth_registration("https://as.example.com")
                .await
                .unwrap(),
            Some(confidential)
        );
    }

    #[tokio::test]
    async fn mcp_oauth_token_roundtrips_and_deletes() {
        let db = Database::open_in_memory().await.unwrap();

        assert!(db.get_mcp_oauth_token("linear").await.unwrap().is_none());

        let token = McpOAuthTokenRow {
            server_name: "linear".to_string(),
            resource_uri: "https://mcp.linear.app/mcp".to_string(),
            scopes: "read write".to_string(),
            access_token: "at-1".to_string(),
            refresh_token: Some("rt-1".to_string()),
            expires_at: 1_900_000_000,
        };
        db.upsert_mcp_oauth_token(&token).await.unwrap();
        assert_eq!(
            db.get_mcp_oauth_token("linear").await.unwrap(),
            Some(token.clone())
        );

        // Upsert (e.g. a refresh persisting a rotated refresh token) replaces
        // the row — OneTokenPerServer.
        let rotated = McpOAuthTokenRow {
            access_token: "at-2".to_string(),
            refresh_token: Some("rt-2".to_string()),
            ..token
        };
        db.upsert_mcp_oauth_token(&rotated).await.unwrap();
        assert_eq!(
            db.get_mcp_oauth_token("linear").await.unwrap(),
            Some(rotated)
        );

        db.delete_mcp_oauth_token("linear").await.unwrap();
        assert!(db.get_mcp_oauth_token("linear").await.unwrap().is_none());
        // Idempotent.
        db.delete_mcp_oauth_token("linear").await.unwrap();
    }

    #[tokio::test]
    async fn default_llm_language_unset_returns_phoenix_native() {
        let db = Database::open_in_memory().await.unwrap();
        assert_eq!(
            db.get_default_llm_language().await.unwrap(),
            LlmLanguage::PhoenixNative
        );
    }

    #[tokio::test]
    async fn default_llm_language_set_persists_and_reads_back() {
        let db = Database::open_in_memory().await.unwrap();

        db.set_default_llm_language(LlmLanguage::Caveman)
            .await
            .unwrap();
        assert_eq!(
            db.get_default_llm_language().await.unwrap(),
            LlmLanguage::Caveman
        );

        // Switch back.
        db.set_default_llm_language(LlmLanguage::PhoenixNative)
            .await
            .unwrap();
        assert_eq!(
            db.get_default_llm_language().await.unwrap(),
            LlmLanguage::PhoenixNative
        );
    }

    #[tokio::test]
    async fn default_llm_language_falls_back_when_value_unknown() {
        let db = Database::open_in_memory().await.unwrap();
        // Forge a value this build doesn't recognize (forward-compat: an
        // older binary reading a DB written by a newer one). Should fall
        // back to the default rather than poison startup.
        db.set_app_setting("default_llm_language", "klingon")
            .await
            .unwrap();
        assert_eq!(
            db.get_default_llm_language().await.unwrap(),
            LlmLanguage::default()
        );
    }

    fn pr_observation(
        repo_owner: &str,
        repo_name: &str,
        pr_number: u64,
        display_state: phoenix_core::domain::pr_display_state::PrDisplayState,
        head: &str,
    ) -> WorkScopePrObservation {
        let state = match display_state {
            phoenix_core::domain::pr_display_state::PrDisplayState::Open
            | phoenix_core::domain::pr_display_state::PrDisplayState::Draft => "OPEN",
            phoenix_core::domain::pr_display_state::PrDisplayState::Merged => "MERGED",
            phoenix_core::domain::pr_display_state::PrDisplayState::Closed => "CLOSED",
        };
        WorkScopePrObservation {
            repo_owner: repo_owner.to_string(),
            repo_name: repo_name.to_string(),
            pr_number,
            title: format!("pr-{pr_number}"),
            url: format!("https://example.test/{repo_owner}/{repo_name}/{pr_number}"),
            state: state.to_string(),
            draft: matches!(
                display_state,
                phoenix_core::domain::pr_display_state::PrDisplayState::Draft
            ),
            display_state,
            base: "main".to_string(),
            head: head.to_string(),
            github_updated_at: Some(format!("2024-01-{pr_number:02}T00:00:00Z")),
        }
    }

    async fn seed_latest_observed_branch(
        db: &Database,
        scope: &phoenix_core::work_scope::WorkScopeId,
        repository_identity: &str,
        branch_name: &str,
        head_oid: &str,
    ) {
        db.upsert_work_scope_observed_branch(
            scope,
            &WorkScopeObservedBranchUpsert {
                repository_identity: repository_identity.to_string(),
                branch_name: branch_name.to_string(),
                head_oid: head_oid.to_string(),
            },
        )
        .await
        .unwrap();
    }

    async fn open_test_db_pair() -> (tempfile::TempDir, Database, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("active-pr-cas.sqlite");
        let db_path = db_path.to_string_lossy().into_owned();
        let first = Database::open(&db_path).await.unwrap();
        migrations::run_pending_migrations(&first.pool)
            .await
            .unwrap();
        let second = Database::open(&db_path).await.unwrap();
        migrations::run_pending_migrations(&second.pool)
            .await
            .unwrap();
        (dir, first, second)
    }

    #[tokio::test]
    async fn active_pr_pin_advances_generation_and_stale_derive_cannot_overwrite_newer_pin() {
        let (_dir, writer, reader) = open_test_db_pair().await;
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-pin-cas".to_string())
                .unwrap();
        writer
            .upsert_work_scope_pr_observations(
                &scope,
                &[
                    pr_observation(
                        "owner",
                        "repo",
                        1,
                        phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                        "feature/a",
                    ),
                    pr_observation(
                        "owner",
                        "repo",
                        2,
                        phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                        "feature/b",
                    ),
                ],
            )
            .await
            .unwrap();

        let derived = reader
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(derived.inference_generation, 1);
        assert_eq!(derived.selection.as_ref().unwrap().pr.pr_number, 1);

        let pinned = writer
            .pin_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner: "owner".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 2,
                },
            )
            .await
            .unwrap();
        assert_eq!(pinned.inference_generation, 2);
        assert_eq!(pinned.selection.as_ref().unwrap().pr.pr_number, 2);
        assert_eq!(
            pinned.selection.as_ref().unwrap().provenance,
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned
        );

        let stale = reader
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                Some(derived.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale, pinned);
    }

    #[tokio::test]
    async fn active_pr_stale_clear_pin_cannot_overwrite_newer_pin() {
        let (_dir, first, second) = open_test_db_pair().await;
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-clear-stale-pin".to_string(),
        )
        .unwrap();
        first
            .upsert_work_scope_pr_observations(
                &scope,
                &[
                    pr_observation(
                        "owner",
                        "repo",
                        1,
                        phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                        "feature/a",
                    ),
                    pr_observation(
                        "owner",
                        "repo",
                        2,
                        phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                        "feature/b",
                    ),
                ],
            )
            .await
            .unwrap();
        let original_pin = first
            .pin_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner: "owner".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 1,
                },
            )
            .await
            .unwrap();
        seed_latest_observed_branch(&first, &scope, "owner/repo", "feature/b", "bbbb").await;

        let newer_pin = second
            .pin_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner: "owner".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 2,
                },
            )
            .await
            .unwrap();

        let work_scope_id = scope.clone();
        let cleared = first
            .derive_active_work_scope_pr_selection_for_scope_id(
                &work_scope_id,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                Some(original_pin.inference_generation),
                original_pin.selection.as_ref(),
                false,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cleared.selection, newer_pin.selection);
        assert_eq!(cleared.inference_generation, newer_pin.inference_generation);
    }

    #[tokio::test]
    async fn active_pr_pinned_selection_survives_association_updates() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-pinned".to_string())
                .unwrap();
        let pr = pr_observation(
            "owner",
            "repo",
            7,
            phoenix_core::domain::pr_display_state::PrDisplayState::Open,
            "feature/a",
        );
        db.upsert_work_scope_pr_observations(&scope, std::slice::from_ref(&pr))
            .await
            .unwrap();
        seed_latest_observed_branch(&db, &scope, "owner/repo", "feature/a", "aaaa").await;

        let pinned = db
            .pin_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner: "owner".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 7,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            pinned.selection.unwrap().provenance,
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned
        );

        let updated = pr_observation(
            "owner",
            "repo",
            7,
            phoenix_core::domain::pr_display_state::PrDisplayState::Draft,
            "feature/a",
        );
        db.upsert_work_scope_pr_observations(&scope, &[updated])
            .await
            .unwrap();
        let derived = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            derived.selection.unwrap().provenance,
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned
        );
    }

    #[tokio::test]
    async fn active_pr_infers_unique_branch_match_from_local_repository_identity_when_scope_maps_to_one_repo(
    ) {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-local-map".to_string())
                .unwrap();
        let local_repo = tempfile::tempdir().unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: std::fs::canonicalize(local_repo.path())
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.selection.unwrap().pr.pr_number, 2);
    }

    #[tokio::test]
    async fn active_pr_conflicting_slug_still_uses_sole_actionable_fallback() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-local-slug-conflict".to_string(),
        )
        .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[pr_observation(
                "owner",
                "repo",
                2,
                phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                "feature/b",
            )],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "other/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state
                .selection
                .as_ref()
                .map(|selection| selection.pr.pr_number),
            Some(2),
            "a conflicting branch identity must not block the sole-actionable fallback"
        );
    }

    #[tokio::test]
    async fn active_pr_does_not_map_local_repository_identity_when_scope_spans_multiple_repos() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-local-ambiguous".to_string(),
        )
        .unwrap();
        let local_repo = tempfile::tempdir().unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo-a",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/shared",
                ),
                pr_observation(
                    "owner",
                    "repo-b",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/shared",
                ),
            ],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: std::fs::canonicalize(local_repo.path())
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                            branch_name: "feature/shared".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(state.selection.is_none());
    }

    #[tokio::test]
    async fn active_pr_infers_unique_branch_match() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-branch".to_string())
                .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.selection.unwrap().pr.pr_number, 2);
    }

    #[tokio::test]
    async fn active_pr_only_actionable_ignores_merged_and_closed() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-actionable".to_string())
                .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Merged,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Closed,
                    "feature/b",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    3,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/c",
                ),
            ],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: None,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.selection.unwrap().pr.pr_number, 3);
    }

    #[tokio::test]
    async fn active_pr_ambiguity_leaves_selection_unset() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-ambiguous".to_string())
                .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Draft,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: None,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(state.selection.is_none());
    }

    #[tokio::test]
    async fn active_pr_unmatched_branch_falls_through_to_sole_actionable_pr() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-unmatched-sole".to_string(),
        )
        .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[pr_observation(
                "owner",
                "repo",
                7,
                phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                "feature/real",
            )],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "main".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.selection.unwrap().pr.pr_number, 7);
    }

    #[tokio::test]
    async fn active_pr_unmatched_branch_keeps_multiple_actionable_prs_ambiguous() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-unmatched-many".to_string(),
        )
        .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Draft,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let state = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "main".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(state.selection.is_none());
    }

    #[tokio::test]
    async fn active_pr_retains_prior_inferred_selection_when_still_valid() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-retain".to_string())
                .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let first = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.selection.as_ref().unwrap().pr.pr_number, 1);

        let retained = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: None,
                },
                Some(first.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.selection.unwrap().pr.pr_number, 1);
    }

    #[tokio::test]
    async fn active_pr_stale_generation_cas_prevents_separate_connection_overwrite() {
        let (_dir, db_a, db_b) = open_test_db_pair().await;
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-cas".to_string()).unwrap();
        db_a.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let first = db_a
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.inference_generation, 1);

        let winner = db_a
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                Some(first.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(winner.selection.as_ref().unwrap().pr.pr_number, 2);
        assert_eq!(winner.inference_generation, 2);

        let stale = db_b
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                Some(first.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale.selection.as_ref().unwrap().pr.pr_number, 2);
        assert_eq!(stale.inference_generation, 2);
        let persisted = db_b
            .active_work_scope_pr_selection(&scope)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.selection.unwrap().pr.pr_number, 2);
        assert_eq!(persisted.inference_generation, 2);
    }

    #[tokio::test]
    async fn active_pr_stale_generation_protection_returns_current_state() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-generation".to_string())
                .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();

        let first = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        let second = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                Some(first.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.selection.as_ref().unwrap().pr.pr_number, 2);

        let stale = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                Some(first.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale.selection.unwrap().pr.pr_number, 2);
        assert_eq!(stale.inference_generation, second.inference_generation);
    }

    #[tokio::test]
    async fn active_pr_pin_requires_existing_scope_association() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-pin-membership".to_string(),
        )
        .unwrap();

        let err = db
            .pin_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner: "owner".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 99,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlx(sqlx::Error::RowNotFound)));
    }

    #[tokio::test]
    async fn active_pr_pin_returns_persisted_generation() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-pin-generation".to_string(),
        )
        .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[pr_observation(
                "owner",
                "repo",
                1,
                phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                "feature/a",
            )],
        )
        .await
        .unwrap();
        let derived = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/a".to_string(),
                        },
                    ),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(derived.inference_generation, 1);

        db.upsert_work_scope_pr_observations(
            &scope,
            &[pr_observation(
                "owner",
                "repo",
                2,
                phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                "feature/b",
            )],
        )
        .await
        .unwrap();
        let advanced = db
            .derive_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
                Some(derived.inference_generation),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(advanced.inference_generation, 2);

        let pinned = db
            .pin_active_work_scope_pr_selection(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                    repo_owner: "owner".to_string(),
                    repo_name: "repo".to_string(),
                    pr_number: 2,
                },
            )
            .await
            .unwrap();
        assert_eq!(pinned.inference_generation, 3);
        assert_eq!(
            pinned.selection.unwrap().provenance,
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Pinned
        );
    }

    #[tokio::test]
    async fn active_pr_clear_pin_uses_latest_durable_branch_evidence_when_input_missing() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-clear-durable".to_string(),
        )
        .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();
        db.pin_active_work_scope_pr_selection(
            &scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: "owner".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 1,
            },
        )
        .await
        .unwrap();
        seed_latest_observed_branch(&db, &scope, "owner/repo", "feature/b", "bbbb").await;

        let resumed = db
            .clear_active_work_scope_pr_pin(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
        let selection = resumed.selection.unwrap();
        assert_eq!(selection.pr.pr_number, 2);
        assert_eq!(
            selection.provenance,
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Inferred
        );
    }

    #[tokio::test]
    async fn active_pr_clear_pin_respects_compatible_repository_mapping() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse(
            "/tmp/ws-active-clear-compatible".to_string(),
        )
        .unwrap();
        let local_repo = tempfile::tempdir().unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo-a",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/shared",
                ),
                pr_observation(
                    "owner",
                    "repo-b",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/shared",
                ),
            ],
        )
        .await
        .unwrap();
        db.pin_active_work_scope_pr_selection(
            &scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: "owner".to_string(),
                repo_name: "repo-a".to_string(),
                pr_number: 1,
            },
        )
        .await
        .unwrap();
        seed_latest_observed_branch(
            &db,
            &scope,
            &std::fs::canonicalize(local_repo.path())
                .unwrap()
                .to_string_lossy(),
            "feature/shared",
            "bbbb",
        )
        .await;

        let resumed = db
            .clear_active_work_scope_pr_pin(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            resumed.selection.is_none(),
            "ambiguous local-repo evidence must not be remapped across multiple GitHub repos"
        );
    }

    #[tokio::test]
    async fn active_pr_clear_pin_resumes_inference() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-active-clear".to_string())
                .unwrap();
        db.upsert_work_scope_pr_observations(
            &scope,
            &[
                pr_observation(
                    "owner",
                    "repo",
                    1,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/a",
                ),
                pr_observation(
                    "owner",
                    "repo",
                    2,
                    phoenix_core::domain::pr_display_state::PrDisplayState::Open,
                    "feature/b",
                ),
            ],
        )
        .await
        .unwrap();
        db.pin_active_work_scope_pr_selection(
            &scope,
            &phoenix_core::domain::active_pr_selection::ActivePrIdentity {
                repo_owner: "owner".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 1,
            },
        )
        .await
        .unwrap();

        let resumed = db
            .clear_active_work_scope_pr_pin(
                &scope,
                &phoenix_core::domain::active_pr_selection::ActivePrInferenceInput {
                    latest_observed_branch: Some(
                        phoenix_core::domain::active_pr_selection::ActivePrBranchContext {
                            repository_identity: "owner/repo".to_string(),
                            branch_name: "feature/b".to_string(),
                        },
                    ),
                },
            )
            .await
            .unwrap()
            .unwrap();
        let selection = resumed.selection.unwrap();
        assert_eq!(selection.pr.pr_number, 2);
        assert_eq!(
            selection.provenance,
            phoenix_core::domain::active_pr_selection::ActivePrSelectionProvenance::Inferred
        );
    }

    #[tokio::test]
    async fn work_scope_pr_association_upsert_preserves_first_seen_and_updates_primary() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-pr".to_string()).unwrap();
        let closed = WorkScopePrObservation {
            repo_owner: "owner".to_string(),
            repo_name: "repo".to_string(),
            pr_number: 1,
            title: "closed".to_string(),
            url: "https://example.test/1".to_string(),
            state: "CLOSED".to_string(),
            draft: false,
            display_state: phoenix_core::domain::pr_display_state::PrDisplayState::Closed,
            base: "main".to_string(),
            head: "branch".to_string(),
            github_updated_at: Some("2024-01-02T00:00:00Z".to_string()),
        };
        db.upsert_work_scope_pr_observations(&scope, std::slice::from_ref(&closed))
            .await
            .unwrap();
        let first = db.list_work_scope_pr_associations(&scope).await.unwrap();
        let first_seen = first[0].first_seen_at.clone();
        let last_seen = first[0].last_seen_at.clone();

        let mut updated = closed.clone();
        updated.title = "closed updated".to_string();
        db.upsert_work_scope_pr_observations(&scope, &[updated])
            .await
            .unwrap();
        let second = db.list_work_scope_pr_associations(&scope).await.unwrap();
        assert_eq!(second[0].first_seen_at, first_seen);
        assert!(second[0].last_seen_at >= last_seen);
        assert_eq!(second[0].title, "closed updated");

        let open = WorkScopePrObservation {
            pr_number: 2,
            title: "open".to_string(),
            url: "https://example.test/2".to_string(),
            state: "OPEN".to_string(),
            display_state: phoenix_core::domain::pr_display_state::PrDisplayState::Open,
            github_updated_at: Some("2024-01-01T00:00:00Z".to_string()),
            ..closed
        };
        db.upsert_work_scope_pr_observations(&scope, &[open])
            .await
            .unwrap();
        let primary = db
            .primary_work_scope_pr_association(&scope)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(primary.pr_number, 2);

        let mut primary_by_scope = db
            .primary_work_scope_pr_associations(&[
                scope.clone(),
                scope.clone(),
                phoenix_core::work_scope::WorkScopeId::parse("missing".to_string()).unwrap(),
            ])
            .await
            .unwrap();
        assert_eq!(primary_by_scope.len(), 1);
        assert_eq!(
            primary_by_scope
                .remove(
                    &phoenix_core::work_scope::ResourceScopeKey::Work(scope.clone()).stable_key(),
                )
                .unwrap()
                .pr_number,
            2
        );
    }

    #[tokio::test]
    async fn turn_usage_first_byte_at_is_nullable_and_roundtrips() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-fb", "slug-fb", "/tmp", true, None, None)
            .await
            .unwrap();

        let usage = phoenix_core::domain::llm_types::Usage {
            input_tokens: 10,
            output_tokens: 20,
            reasoning_tokens: None,
            cache_creation_tokens: 0,
            cache_read_tokens: 5,
        };
        db.insert_turn_usage(
            "conv-fb",
            "conv-fb",
            "mock",
            EffectiveEffort::native_unknown(),
            &usage,
            None,
        )
        .await
        .unwrap();
        let observed = Utc::now();
        db.insert_turn_usage(
            "conv-fb",
            "conv-fb",
            "mock",
            EffectiveEffort::native_unknown(),
            &usage,
            Some(observed),
        )
        .await
        .unwrap();

        let rows = db.usage_conversation_turns("conv-fb").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].first_byte_at, None);
        assert_eq!(
            rows[1].first_byte_at.as_deref(),
            Some(observed.to_rfc3339().as_str())
        );
    }

    #[tokio::test]
    async fn list_conversations_preserves_effort_projection() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-effort", "slug-effort", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_model_and_effort(
            "conv-effort",
            "gpt-5.4",
            Some(ModelEffort::Low),
            ServiceTier::Fast,
        )
        .await
        .unwrap();

        let listed = db.list_conversations().await.unwrap();
        assert_eq!(
            listed
                .into_iter()
                .find(|conversation| conversation.id == "conv-effort")
                .and_then(|conversation| conversation.effort),
            Some(ModelEffort::Low)
        );
        let listed = db.list_conversations().await.unwrap();
        assert_eq!(
            listed
                .into_iter()
                .find(|conversation| conversation.id == "conv-effort")
                .map(|conversation| conversation.service_tier),
            Some(ServiceTier::Fast)
        );
    }

    #[tokio::test]
    async fn compare_and_set_service_tier_preserves_concurrent_model_upgrade() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("tier-cas", "tier-cas", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_model_and_effort("tier-cas", "gpt-5.4", None, ServiceTier::Fast)
            .await
            .unwrap();
        db.update_conversation_model_and_effort("tier-cas", "gpt-5.6-sol", None, ServiceTier::Fast)
            .await
            .unwrap();

        let normalized = db
            .compare_and_set_conversation_service_tier(
                "tier-cas",
                Some("gpt-5.4"),
                ServiceTier::Fast,
                ServiceTier::Standard,
            )
            .await
            .unwrap();
        assert!(!normalized);

        let conversation = db.get_conversation("tier-cas").await.unwrap();
        assert_eq!(conversation.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(conversation.service_tier, ServiceTier::Fast);
    }

    #[tokio::test]
    async fn analytics_conversation_ids_include_root_without_usage() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-root-only", "slug-root-only", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "conv-child-no-usage",
            "slug-child-no-usage",
            "/tmp",
            false,
            Some("conv-root-only"),
            None,
        )
        .await
        .unwrap();

        let ids = db
            .analytics_conversation_ids_for_root("conv-root-only")
            .await
            .unwrap();
        assert_eq!(
            ids,
            vec![
                "conv-child-no-usage".to_string(),
                "conv-root-only".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn usage_anchor_messages_returns_non_agent_timestamps_without_content() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("root-anchor", "root-anchor", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "sub-anchor",
            "sub-anchor",
            "/tmp",
            false,
            Some("root-anchor"),
            None,
        )
        .await
        .unwrap();

        db.add_message(
            "root-user",
            "root-anchor",
            &MessageContent::user("root"),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "root-agent",
            "root-anchor",
            &MessageContent::agent(vec![phoenix_core::domain::llm_types::ContentBlock::Text {
                text: "agent".to_string(),
            }]),
            None,
            None,
        )
        .await
        .unwrap();
        db.add_message(
            "sub-tool",
            "sub-anchor",
            &MessageContent::tool("tu", "ok", false),
            None,
            None,
        )
        .await
        .unwrap();

        let usage = phoenix_core::domain::llm_types::Usage::default();
        db.insert_turn_usage(
            "sub-anchor",
            "root-anchor",
            "mock",
            EffectiveEffort::native_unknown(),
            &usage,
            None,
        )
        .await
        .unwrap();

        let anchors = db.usage_anchor_messages("root-anchor").await.unwrap();
        let ids: Vec<_> = anchors.iter().map(|a| a.conversation_id.as_str()).collect();
        assert_eq!(ids, vec!["root-anchor", "sub-anchor"]);
    }

    #[tokio::test]
    async fn work_scope_observed_branch_upsert_preserves_first_seen_and_updates_last_seen() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-observed".to_string()).unwrap();

        db.upsert_work_scope_observed_branch(
            &scope,
            &WorkScopeObservedBranchUpsert {
                repository_identity: "/tmp/repo-a".to_string(),
                branch_name: "feature/a".to_string(),
                head_oid: "aaaa".to_string(),
            },
        )
        .await
        .unwrap();
        let first = db.list_work_scope_observed_branches(&scope).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].first_observed_head_oid, "aaaa");
        assert_eq!(first[0].last_observed_head_oid, "aaaa");

        db.upsert_work_scope_observed_branch(
            &scope,
            &WorkScopeObservedBranchUpsert {
                repository_identity: "/tmp/repo-a".to_string(),
                branch_name: "feature/a".to_string(),
                head_oid: "bbbb".to_string(),
            },
        )
        .await
        .unwrap();
        let second = db.list_work_scope_observed_branches(&scope).await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].first_observed_head_oid, "aaaa");
        assert_eq!(second[0].last_observed_head_oid, "bbbb");
        assert_eq!(second[0].first_observed_at, first[0].first_observed_at);
        assert!(second[0].last_observed_at >= first[0].last_observed_at);
    }

    #[test]
    fn observed_branch_qualification_rejects_base_and_no_relative_work() {
        use phoenix_core::domain::observed_branch::LocalGitHeadObservation;

        let base = ObservedBranchQualificationInput {
            conversation_base_branch: "main".to_string(),
            task_relative_work_base_head_oid: "baseoid".to_string(),
        };

        assert!(qualifies_observed_branch(
            &LocalGitHeadObservation::NamedBranch {
                repository_identity: "/repo".to_string(),
                branch_name: "main".to_string(),
                head_oid: "headoid".to_string(),
            },
            &base,
        )
        .is_none());
        assert!(qualifies_observed_branch(
            &LocalGitHeadObservation::NamedBranch {
                repository_identity: "/repo".to_string(),
                branch_name: "feature/no-work".to_string(),
                head_oid: "baseoid".to_string(),
            },
            &base,
        )
        .is_none());
        assert_eq!(
            qualifies_observed_branch(
                &LocalGitHeadObservation::NamedBranch {
                    repository_identity: "/repo".to_string(),
                    branch_name: "feature/stacked".to_string(),
                    head_oid: "stackoid".to_string(),
                },
                &base,
            ),
            Some(WorkScopeObservedBranchUpsert {
                repository_identity: "/repo".to_string(),
                branch_name: "feature/stacked".to_string(),
                head_oid: "stackoid".to_string(),
            })
        );
    }

    #[test]
    fn observed_branch_qualification_rejects_non_named_head_states() {
        use phoenix_core::domain::observed_branch::LocalGitHeadObservation;

        let input = ObservedBranchQualificationInput {
            conversation_base_branch: "main".to_string(),
            task_relative_work_base_head_oid: "baseoid".to_string(),
        };
        for observed in [
            LocalGitHeadObservation::Detached {
                repository_identity: "/repo".to_string(),
                head_oid: "abcd".to_string(),
            },
            LocalGitHeadObservation::Unborn {
                repository_identity: "/repo".to_string(),
                branch_name: Some("main".to_string()),
            },
            LocalGitHeadObservation::Unavailable {
                repository_identity: Some("/repo".to_string()),
                error: "git failed".to_string(),
            },
        ] {
            assert!(qualifies_observed_branch(&observed, &input).is_none());
        }
    }

    #[tokio::test]
    async fn work_scope_pr_feedback_baseline_roundtrips_and_replaces() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-baseline".to_string()).unwrap();

        db.upsert_work_scope_pr_feedback_baseline(
            &scope,
            &WorkScopePrFeedbackBaselineInput {
                repo_owner: "owner".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 7,
                captured_at: "2026-01-01T00:00:00Z".to_string(),
                github_updated_at: Some("2026-01-01T00:00:00Z".to_string()),
                feedback_identities: vec!["b".to_string(), "a".to_string(), "a".to_string()],
                feedback_fingerprints: vec!["fb".to_string(), "fa".to_string(), "fa".to_string()],
            },
        )
        .await
        .unwrap();

        db.upsert_work_scope_pr_feedback_baseline(
            &scope,
            &WorkScopePrFeedbackBaselineInput {
                repo_owner: "owner".to_string(),
                repo_name: "repo".to_string(),
                pr_number: 7,
                captured_at: "2026-01-02T00:00:00Z".to_string(),
                github_updated_at: Some("2026-01-02T00:00:00Z".to_string()),
                feedback_identities: vec!["c".to_string()],
                feedback_fingerprints: vec!["fc".to_string()],
            },
        )
        .await
        .unwrap();

        let baseline = db
            .work_scope_pr_feedback_baseline(&scope, "owner", "repo", 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(baseline.pr_number, 7);
        assert_eq!(baseline.captured_at, "2026-01-02T00:00:00Z");
        assert_eq!(
            baseline.github_updated_at.as_deref(),
            Some("2026-01-02T00:00:00Z")
        );
        assert_eq!(baseline.feedback_identities, vec!["c".to_string()]);
        assert_eq!(baseline.feedback_fingerprints, vec!["fc".to_string()]);
    }

    #[tokio::test]
    async fn work_scope_pr_feedback_baselines_are_keyed_by_full_identity() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            phoenix_core::work_scope::WorkScopeId::parse("/tmp/ws-baseline-identities".to_string())
                .unwrap();

        for repo_name in ["repo-a", "repo-b"] {
            db.upsert_work_scope_pr_feedback_baseline(
                &scope,
                &WorkScopePrFeedbackBaselineInput {
                    repo_owner: "owner".to_string(),
                    repo_name: repo_name.to_string(),
                    pr_number: 7,
                    captured_at: format!(
                        "2026-01-0{}T00:00:00Z",
                        if repo_name == "repo-a" { 1 } else { 2 }
                    ),
                    github_updated_at: None,
                    feedback_identities: vec![repo_name.to_string()],
                    feedback_fingerprints: vec![format!("fp-{repo_name}")],
                },
            )
            .await
            .unwrap();
        }

        let a = db
            .work_scope_pr_feedback_baseline(&scope, "owner", "repo-a", 7)
            .await
            .unwrap()
            .unwrap();
        let b = db
            .work_scope_pr_feedback_baseline(&scope, "owner", "repo-b", 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a.feedback_identities, vec!["repo-a".to_string()]);
        assert_eq!(b.feedback_identities, vec!["repo-b".to_string()]);
    }

    #[test]
    fn direct_mode_receives_restricted_authority() {
        let cm = conv_mode_columns(&ConvMode::Direct);
        assert_eq!(
            Database::authority_for_mode(&cm),
            AuthorityKind::RestrictedExplore
        );
    }

    #[tokio::test]
    async fn test_create_and_get_conversation() {
        let db = Database::open_in_memory().await.unwrap();

        let conv = db
            .create_conversation("test-id", "test-slug", "/tmp/test", true, None, None)
            .await
            .unwrap();

        assert_eq!(conv.id, "test-id");
        assert_ne!(conv.product_conversation_id.as_str(), conv.id);
        assert_eq!(conv.slug, Some("test-slug".to_string()));
        assert_eq!(conv.cwd, "/tmp/test");
        assert!(matches!(conv.state, ConvState::Idle));

        let fetched = db.get_conversation("test-id").await.unwrap();
        assert_eq!(fetched.id, conv.id);
        assert_eq!(
            fetched.product_conversation_id,
            conv.product_conversation_id
        );
    }

    #[tokio::test]
    async fn begin_continuation_atomically_persists_response_and_operation() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("begin-continuation", "begin", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            "begin-continuation",
            &ConvState::LlmRequesting { attempt: 1 },
        )
        .await
        .unwrap();
        let request = phoenix_core::domain::sm_state::ContinuationSummaryRequest {
            operation_id: "begin-operation".to_string(),
            rejected_tool_calls: Vec::new(),
            attempt: 1,
        };
        let awaiting = ConvState::AwaitingContinuation {
            request: request.clone(),
        };
        let content =
            MessageContent::agent(vec![phoenix_core::domain::llm_types::ContentBlock::text(
                "threshold response",
            )]);
        let message = Message {
            message_id: request.operation_id.clone(),
            conversation_id: "begin-continuation".to_string(),
            sequence_id: 1,
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };

        assert_eq!(
            db.begin_continuation(
                "begin-continuation",
                &request.operation_id,
                &message,
                &awaiting,
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Applied
        );
        assert_eq!(
            db.get_conversation("begin-continuation")
                .await
                .unwrap()
                .state,
            awaiting
        );
        assert_eq!(
            db.get_messages("begin-continuation").await.unwrap().len(),
            1
        );
        assert_eq!(
            db.begin_continuation(
                "begin-continuation",
                &request.operation_id,
                &message,
                &ConvState::AwaitingContinuation {
                    request: request.clone(),
                },
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Duplicate
        );
        assert_eq!(
            db.get_messages("begin-continuation").await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn continuation_start_recovery_retains_threshold_response() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("recover-start", "recover", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state("recover-start", &ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        let request = phoenix_core::domain::sm_state::ContinuationSummaryRequest {
            operation_id: "recover-operation".to_string(),
            rejected_tool_calls: Vec::new(),
            attempt: 1,
        };
        let failure = ConvState::RecoverableContinuationFailure {
            failure: phoenix_core::domain::sm_state::RecoverableContinuationFailure {
                request: request.clone(),
                error_kind: ErrorKind::ServerError,
                message: "start failed".to_string(),
            },
        };
        let content =
            MessageContent::agent(vec![phoenix_core::domain::llm_types::ContentBlock::text(
                "threshold response",
            )]);
        let message = Message {
            message_id: request.operation_id.clone(),
            conversation_id: "recover-start".to_string(),
            sequence_id: 1,
            message_type: content.message_type(),
            content: content.clone(),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };

        assert_eq!(
            db.recover_continuation_start(
                "recover-start",
                &request.operation_id,
                &message,
                &failure,
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Applied
        );
        assert_eq!(
            db.get_conversation("recover-start").await.unwrap().state,
            failure
        );
        let messages = db.get_messages("recover-start").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, content);
        assert_eq!(
            db.recover_continuation_start(
                "recover-start",
                &request.operation_id,
                &message,
                &ConvState::RecoverableContinuationFailure {
                    failure: phoenix_core::domain::sm_state::RecoverableContinuationFailure {
                        request: request.clone(),
                        error_kind: ErrorKind::ServerError,
                        message: "start failed".to_string(),
                    },
                },
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Duplicate
        );
        assert_eq!(db.get_messages("recover-start").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn continuation_commit_is_atomic_idempotent_and_rejects_stale_operations() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "continuation-commit",
            "continuation",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let operation_id = "operation-1";
        db.update_conversation_state(
            "continuation-commit",
            &ConvState::AwaitingContinuation {
                request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                    operation_id: operation_id.to_string(),
                    rejected_tool_calls: Vec::new(),
                    attempt: 1,
                },
            },
        )
        .await
        .unwrap();
        let completed = ConvState::ContextExhausted {
            summary: "durable summary".to_string(),
        };
        let content = MessageContent::continuation("durable summary");
        let message = Message {
            message_id: format!("continuation-{operation_id}"),
            conversation_id: "continuation-commit".to_string(),
            sequence_id: 1,
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };

        assert_eq!(
            db.commit_continuation(
                "continuation-commit",
                operation_id,
                &message,
                &completed,
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Applied
        );
        assert_eq!(
            db.commit_continuation(
                "continuation-commit",
                operation_id,
                &message,
                &completed,
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Duplicate
        );
        assert_eq!(
            db.get_messages("continuation-commit").await.unwrap().len(),
            1
        );

        let stale_content = MessageContent::continuation("stale summary");
        let stale = Message {
            message_id: "continuation-operation-2".to_string(),
            conversation_id: "continuation-commit".to_string(),
            sequence_id: 2,
            message_type: stale_content.message_type(),
            content: stale_content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        assert_eq!(
            db.commit_continuation(
                "continuation-commit",
                "operation-2",
                &stale,
                &ConvState::ContextExhausted {
                    summary: "stale summary".to_string(),
                },
                Utc::now(),
            )
            .await
            .unwrap(),
            ContinuationCommitOutcome::Stale
        );
        assert_eq!(
            db.get_messages("continuation-commit").await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn coordinator_relation_is_singleton_and_keeps_conversation_shape_ordinary() {
        let db = Database::open_in_memory().await.unwrap();

        let first = db
            .get_or_create_coordinator(
                Some("test-model"),
                phoenix_core::llm_language::LlmLanguage::Caveman,
            )
            .await
            .unwrap();
        let second = db
            .get_or_create_coordinator(
                Some("other-model"),
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(
            first.llm_language,
            phoenix_core::llm_language::LlmLanguage::Caveman
        );
        assert_eq!(
            db.coordinator_conversation_id().await.unwrap().as_deref(),
            Some(first.id.as_str())
        );
        assert!(db.is_coordinator_conversation(&first.id).await.unwrap());

        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('conversations')")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert!(!columns.iter().any(|column| column == "conversation_kind"));
    }

    #[tokio::test]
    async fn conversations_schema_exposes_state_kind_discriminator() {
        let db = Database::open_in_memory().await.unwrap();

        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('conversations')")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert!(columns.iter().any(|column| column == "state_kind"));

        let indexed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_conversations_state_kind'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(indexed, 1);
    }

    #[tokio::test]
    async fn coordinator_creation_handles_slug_collision_and_concurrent_first_access() {
        let path = std::env::temp_dir().join(format!(
            "phoenix-coordinator-race-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        migrations::run_pending_migrations(db.pool()).await.unwrap();
        db.create_conversation("ordinary", "coordinator", "/tmp", true, None, None)
            .await
            .unwrap();

        let (left, right) = tokio::join!(
            db.get_or_create_coordinator(
                Some("test-model"),
                phoenix_core::llm_language::LlmLanguage::default()
            ),
            db.get_or_create_coordinator(
                Some("test-model"),
                phoenix_core::llm_language::LlmLanguage::default()
            ),
        );
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.id, right.id);
        assert_ne!(left.slug.as_deref(), Some("coordinator"));

        let coordinator_conversation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations WHERE runtime_role = 'coordinator'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(coordinator_conversation_count, 1);
        db.pool().close().await;
        let _ = std::fs::remove_file(path);
    }

    /// The clear watermark write is structurally monotonic: a value below the
    /// persisted watermark is ignored, never regressing it (REQ-STR-007). A
    /// transient stale-low write (e.g. after a failed read re-planning from 0)
    /// therefore cannot re-expose already-cleared results.
    #[tokio::test]
    async fn update_clear_watermark_never_regresses() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("wm-1", "wm-slug", "/tmp", true, None, None)
            .await
            .unwrap();

        assert_eq!(db.get_clear_watermark("wm-1").await.unwrap(), 0);

        db.update_clear_watermark("wm-1", 500).await.unwrap();
        assert_eq!(db.get_clear_watermark("wm-1").await.unwrap(), 500);

        // A lower value is ignored — the watermark holds at 500.
        db.update_clear_watermark("wm-1", 300).await.unwrap();
        assert_eq!(db.get_clear_watermark("wm-1").await.unwrap(), 500);

        // A higher value advances it.
        db.update_clear_watermark("wm-1", 900).await.unwrap();
        assert_eq!(db.get_clear_watermark("wm-1").await.unwrap(), 900);

        // A write to a missing conversation still reports not-found.
        assert!(matches!(
            db.update_clear_watermark("nope", 10).await,
            Err(DbError::ConversationNotFound(_))
        ));
    }

    #[tokio::test]
    async fn state_kind_tracks_inserted_and_updated_conversation_state() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("kind-conv", "kind-slug", "/tmp", true, None, None)
            .await
            .unwrap();

        let initial: (String, String) =
            sqlx::query_as("SELECT state_kind, state FROM conversations WHERE id = 'kind-conv'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(initial.0, "idle");
        assert!(initial.1.contains("\"type\":\"idle\""));

        let updated_state = ConvState::AwaitingContinuation {
            request: phoenix_core::domain::sm_state::ContinuationSummaryRequest {
                operation_id: "op-1".to_string(),
                rejected_tool_calls: Vec::new(),
                attempt: 1,
            },
        };
        db.update_conversation_state("kind-conv", &updated_state)
            .await
            .unwrap();

        let updated: (String, String) =
            sqlx::query_as("SELECT state_kind, state FROM conversations WHERE id = 'kind-conv'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(updated.0, "awaiting_continuation");
        assert!(updated.1.contains("\"type\":\"awaiting_continuation\""));
    }

    #[tokio::test]
    async fn list_usage_limit_errors_returns_only_due_candidate_rows() {
        use phoenix_core::domain::db_schema::ErrorKind;
        let db = Database::open_in_memory().await.unwrap();
        let reset = Utc::now();

        let usage_limit_err = |resets_at| ConvState::Error {
            message: "You've hit your usage limit.".to_string(),
            error_kind: ErrorKind::UsageLimitReached,
            resets_at,
        };

        // Match: usage-limit error with a reset time.
        db.create_conversation("ul", "s-ul", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state("ul", &usage_limit_err(Some(reset)))
            .await
            .unwrap();

        // Excluded: usage-limit error WITHOUT a reset time (no window to wait).
        db.create_conversation("ul-nores", "s-ulnr", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state("ul-nores", &usage_limit_err(None))
            .await
            .unwrap();

        // Excluded: a different error kind, even with a reset time.
        db.create_conversation("net", "s-net", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            "net",
            &ConvState::Error {
                message: "network".to_string(),
                error_kind: ErrorKind::Network,
                resets_at: Some(reset),
            },
        )
        .await
        .unwrap();

        // Excluded: a sub-agent (user_initiated = false) in a usage-limit error.
        db.create_conversation("sub", "s-sub", "/tmp", false, None, None)
            .await
            .unwrap();
        db.update_conversation_state("sub", &usage_limit_err(Some(reset)))
            .await
            .unwrap();

        // Excluded: not an error at all.
        db.create_conversation("idle", "s-idle", "/tmp", true, None, None)
            .await
            .unwrap();

        let coordinator = db
            .get_or_create_coordinator(
                Some("test-model"),
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        db.update_conversation_state(&coordinator.id, &usage_limit_err(Some(reset)))
            .await
            .unwrap();

        let got = db.list_usage_limit_errors().await.unwrap();
        let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["ul", coordinator.id.as_str()],
            "user-facing conversations include the singleton Coordinator"
        );
        assert!(matches!(
            got[0].1,
            ConvState::Error {
                error_kind: ErrorKind::UsageLimitReached,
                resets_at: Some(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn sub_agent_persona_roundtrips_and_upserts() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("sub-1", "sub-slug", "/tmp", false, None, None)
            .await
            .unwrap();

        // Absent until written.
        assert_eq!(db.get_sub_agent_persona("sub-1").await.unwrap(), None);

        db.set_sub_agent_persona("sub-1", "You are a reviewer.")
            .await
            .unwrap();
        assert_eq!(
            db.get_sub_agent_persona("sub-1").await.unwrap(),
            Some("You are a reviewer.".to_string())
        );

        // Upsert replaces.
        db.set_sub_agent_persona("sub-1", "You are a docs writer.")
            .await
            .unwrap();
        assert_eq!(
            db.get_sub_agent_persona("sub-1").await.unwrap(),
            Some("You are a docs writer.".to_string())
        );
    }

    #[tokio::test]
    async fn sub_agent_persona_cascade_deletes_with_conversation() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("sub-2", "sub-slug-2", "/tmp", false, None, None)
            .await
            .unwrap();
        db.set_sub_agent_persona("sub-2", "persona").await.unwrap();

        db.delete_conversation("sub-2").await.unwrap();
        assert_eq!(db.get_sub_agent_persona("sub-2").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_add_and_get_messages() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let db = Database::open_in_memory().await.unwrap();

        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        let msg1 = db
            .add_message(
                "msg-1",
                "conv-1",
                &MessageContent::user("Hello"),
                None,
                None,
            )
            .await
            .unwrap();

        let msg2 = db
            .add_message(
                "msg-2",
                "conv-1",
                &MessageContent::agent(vec![ContentBlock::text("Hi there!")]),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(msg1.sequence_id, 1);
        assert_eq!(msg2.sequence_id, 2);
        assert_eq!(msg1.message_type, MessageType::User);
        assert_eq!(msg2.message_type, MessageType::Agent);

        let messages = db.get_messages("conv-1").await.unwrap();
        assert_eq!(messages.len(), 2);

        // Verify content is properly typed
        match &messages[0].content {
            MessageContent::User(u) => assert_eq!(u.text, "Hello"),
            MessageContent::Agent(_)
            | MessageContent::Tool(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_)
            | MessageContent::Skill(_) => panic!("Expected User content"),
        }

        let after = db.get_messages_after("conv-1", 1).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message_id, "msg-2");
    }

    #[tokio::test]
    async fn transcript_generation_does_not_change_on_append() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-append", "slug-append", "/tmp", true, None, None)
            .await
            .unwrap();

        let before = db.get_conversation("conv-append").await.unwrap();
        db.add_message(
            "append-1",
            "conv-append",
            &MessageContent::user("hello"),
            None,
            None,
        )
        .await
        .unwrap();
        let after = db.get_conversation("conv-append").await.unwrap();

        assert_eq!(before.transcript_generation, 1);
        assert_eq!(after.transcript_generation, before.transcript_generation);
    }

    #[tokio::test]
    async fn update_message_display_data_increments_transcript_generation_once() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-display", "slug-display", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "display-1",
            "conv-display",
            &MessageContent::tool("tool-1", "result", false),
            None,
            None,
        )
        .await
        .unwrap();

        let before = db.get_conversation("conv-display").await.unwrap();
        let generation = db
            .update_message_display_data("display-1", &serde_json::json!({ "hidden": true }))
            .await
            .unwrap();
        let after = db.get_conversation("conv-display").await.unwrap();

        assert_eq!(generation, before.transcript_generation + 1);
        assert_eq!(
            after.transcript_generation,
            before.transcript_generation + 1
        );
    }

    #[tokio::test]
    async fn hiding_message_removes_retrieval_row_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("hidden-index", "hidden-index", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "hidden-index-message",
            "hidden-index",
            &MessageContent::user("searchable before hidden"),
            None,
            None,
        )
        .await
        .unwrap();

        db.update_message_display_data(
            "hidden-index-message",
            &serde_json::json!({ "hidden": true }),
        )
        .await
        .unwrap();

        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_fts_rows WHERE message_id = ?")
                .bind("hidden-index-message")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(rows, 1);
        let indexed_text: String = sqlx::query_scalar(
            "SELECT f.text FROM message_fts f \
             JOIN message_fts_rows r ON r.fts_rowid = f.rowid \
             WHERE r.message_id = ?",
        )
        .bind("hidden-index-message")
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(indexed_text.is_empty());

        db.update_message_display_data(
            "hidden-index-message",
            &serde_json::json!({ "hidden": false }),
        )
        .await
        .unwrap();
        let restored_text: String = sqlx::query_scalar(
            "SELECT f.text FROM message_fts f \
             JOIN message_fts_rows r ON r.fts_rowid = f.rowid \
             WHERE r.message_id = ?",
        )
        .bind("hidden-index-message")
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(restored_text.contains("searchable before hidden"));
    }

    #[tokio::test]
    async fn update_tool_message_content_increments_transcript_generation_once() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-tool", "slug-tool", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "tool-1",
            "conv-tool",
            &MessageContent::tool("tool-use-1", "alpha", false),
            None,
            None,
        )
        .await
        .unwrap();

        let before = db.get_conversation("conv-tool").await.unwrap();
        let generation = db
            .update_tool_message_content("tool-1", "omega")
            .await
            .unwrap();
        let after = db.get_conversation("conv-tool").await.unwrap();

        assert_eq!(generation, before.transcript_generation + 1);
        assert_eq!(
            after.transcript_generation,
            before.transcript_generation + 1
        );
    }

    #[tokio::test]
    async fn missing_message_edit_leaves_transcript_generation_unchanged() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-miss", "slug-miss", "/tmp", true, None, None)
            .await
            .unwrap();

        let before = db.get_conversation("conv-miss").await.unwrap();
        let err = db
            .update_message_display_data("missing", &serde_json::json!({ "hidden": true }))
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::MessageNotFound(id) if id == "missing"));
        let mid = db.get_conversation("conv-miss").await.unwrap();
        assert_eq!(mid.transcript_generation, before.transcript_generation);

        let err = db
            .update_tool_message_content("missing", "omega")
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::MessageNotFound(id) if id == "missing"));
        let after = db.get_conversation("conv-miss").await.unwrap();
        assert_eq!(after.transcript_generation, before.transcript_generation);
    }

    #[tokio::test]
    async fn message_slice_helpers_return_expected_windows() {
        let db = Database::open_in_memory().await.unwrap();

        db.create_conversation("conv-slices", "slug-slices", "/tmp", true, None, None)
            .await
            .unwrap();

        for idx in 1..=5 {
            db.add_message(
                &format!("msg-{idx}"),
                "conv-slices",
                &MessageContent::user(format!("m{idx}")),
                None,
                None,
            )
            .await
            .unwrap();
        }

        let latest = db.get_latest_messages("conv-slices", 2).await.unwrap();
        assert_eq!(
            latest.iter().map(|m| m.sequence_id).collect::<Vec<_>>(),
            vec![4, 5]
        );

        let before = db.get_messages_before("conv-slices", 4, 2).await.unwrap();
        assert_eq!(
            before.iter().map(|m| m.sequence_id).collect::<Vec<_>>(),
            vec![2, 3]
        );

        let after = db
            .get_messages_after_limited("conv-slices", 2, 2)
            .await
            .unwrap();
        assert_eq!(
            after.iter().map(|m| m.sequence_id).collect::<Vec<_>>(),
            vec![3, 4]
        );

        let range = db.get_message_range("conv-slices", 2, 4).await.unwrap();
        assert_eq!(
            range.iter().map(|m| m.sequence_id).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );

        let (around_before, around_after) = db
            .get_messages_around("conv-slices", 3, 2, 2)
            .await
            .unwrap();
        assert_eq!(
            around_before
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            around_after
                .iter()
                .map(|m| m.sequence_id)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
    }

    /// A freshly-migrated DB (base SCHEMA + all versioned migrations, the path
    /// `open_in_memory` exercises) ends with the normalized `cm_*` columns and
    /// no `conv_mode` blob. This locks the migration-029 end state and proves
    /// the `conv_mode`-referencing migrations resolve during fresh-DB replay
    /// (`conv_mode` lives in the base schema until 029 drops it).
    #[tokio::test]
    async fn conversations_schema_is_normalized_after_migrations() {
        let db = Database::open_in_memory().await.unwrap();
        let cols: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .map(|r: SqliteRow| r.get::<String, _>("name"))
            .fetch_all(db.pool())
            .await
            .unwrap();
        assert!(
            cols.iter().any(|c| c == "cm_kind"),
            "cm_kind present: {cols:?}"
        );
        for removed in ["cm_branch_name", "cm_worktree_path", "cm_base_branch"] {
            assert!(
                !cols.iter().any(|column| column == removed),
                "{removed} must be owned only by work_scopes: {cols:?}"
            );
        }
        let environment_projection_kind: String = sqlx::query_scalar(
            "SELECT type FROM sqlite_master WHERE name = 'work_scope_environments'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(environment_projection_kind, "view");
        let writable_projection_triggers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'trigger' AND name LIKE 'work_scope_environments_%'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(writable_projection_triggers, 0);
        assert!(
            !cols.iter().any(|c| c == "conv_mode"),
            "conv_mode blob must be dropped after migration 029: {cols:?}"
        );
    }

    /// User/skill attachments round-trip through the normalized child tables:
    /// they are written to `message_files`/`message_images`, the `content` blob
    /// stays attachment-free, and `get_messages` rehydrates them in order.
    #[tokio::test]
    async fn attachments_persist_in_child_tables_and_rehydrate() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-a", "slug-a", "/tmp", true, None, None)
            .await
            .unwrap();

        let images = vec![
            ImageData {
                data: "AAA".into(),
                media_type: "image/png".into(),
            },
            ImageData {
                data: "BBB".into(),
                media_type: "image/jpeg".into(),
            },
        ];
        let files = vec![FileAttachment {
            original_name: "notes.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: 42,
            stored_path: "/store/notes.txt".into(),
        }];
        db.add_message(
            "m-user",
            "conv-a",
            &MessageContent::user_with_attachments("hi", images, files),
            None,
            None,
        )
        .await
        .unwrap();

        db.add_message(
            "m-skill",
            "conv-a",
            &MessageContent::Skill(SkillContent {
                name: "build".into(),
                body: "body".into(),
                trigger: "/build".into(),
                files: vec![FileAttachment {
                    original_name: "s.txt".into(),
                    media_type: "text/plain".into(),
                    size_bytes: 7,
                    stored_path: "/store/s.txt".into(),
                }],
            }),
            None,
            None,
        )
        .await
        .unwrap();

        // The persisted content blob carries no attachment keys.
        for id in ["m-user", "m-skill"] {
            let raw: String =
                sqlx::query_scalar("SELECT content FROM messages WHERE message_id = ?1")
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert!(
                v.get("files").is_none() && v.get("images").is_none(),
                "blob for {id} must be attachment-free, got {raw}"
            );
        }

        // get_messages rehydrates attachments (in order) from the child tables.
        let messages = db.get_messages("conv-a").await.unwrap();
        let user = messages.iter().find(|m| m.message_id == "m-user").unwrap();
        match &user.content {
            MessageContent::User(u) => {
                assert_eq!(u.text, "hi");
                assert_eq!(u.images.len(), 2);
                assert_eq!(u.images[0].data, "AAA");
                assert_eq!(u.images[1].media_type, "image/jpeg");
                assert_eq!(u.files.len(), 1);
                assert_eq!(u.files[0].original_name, "notes.txt");
                assert_eq!(u.files[0].size_bytes, 42);
            }
            MessageContent::Agent(_)
            | MessageContent::Tool(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_)
            | MessageContent::Skill(_) => panic!("expected user content"),
        }
        let skill = messages.iter().find(|m| m.message_id == "m-skill").unwrap();
        match &skill.content {
            MessageContent::Skill(s) => {
                assert_eq!(s.files.len(), 1);
                assert_eq!(s.files[0].stored_path, "/store/s.txt");
            }
            MessageContent::User(_)
            | MessageContent::Agent(_)
            | MessageContent::Tool(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_) => panic!("expected skill content"),
        }
    }

    /// Steering queue round-trips through the normalized tables: replace-all
    /// writes entries + attachments + skill trio, `get_steering_queue` rehydrates
    /// them in FIFO order, and `remove_steering_entries` deletes an entry and
    /// cascades its attachments.
    #[tokio::test]
    async fn steering_queue_round_trips_and_removes_via_child_tables() {
        use phoenix_core::domain::db_schema::{FileAttachment, ImageData};
        use phoenix_core::domain::skill_invocation::SkillInvocation;
        use phoenix_core::domain::sm_event::SteerEntry;

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-s", "slug-s", "/tmp", true, None, None)
            .await
            .unwrap();

        let entry_a = SteerEntry {
            text: "first".into(),
            llm_text: Some("first-expanded".into()),
            images: vec![ImageData {
                data: "IMG".into(),
                media_type: "image/png".into(),
            }],
            files: vec![FileAttachment {
                original_name: "a.txt".into(),
                media_type: "text/plain".into(),
                size_bytes: 5,
                stored_path: "/store/a".into(),
            }],
            message_id: "sa".into(),
            user_agent: Some("UA".into()),
            skill_invocation: None,
        };
        let entry_b = SteerEntry {
            text: "second".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: "sb".into(),
            user_agent: None,
            skill_invocation: Some(SkillInvocation {
                name: "build".into(),
                body: "BODY".into(),
                skill_dir: "/skills/build".into(),
            }),
        };
        db.update_steering_queue("conv-s", &[entry_a, entry_b])
            .await
            .unwrap();

        let queue = db.get_steering_queue("conv-s").await.unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(
            db.steering_conversation_id_for_message("sa")
                .await
                .unwrap()
                .as_deref(),
            Some("conv-s")
        );
        assert_eq!(
            db.steering_conversation_id_for_message("missing")
                .await
                .unwrap(),
            None
        );
        assert_eq!(queue[0].message_id, "sa");
        assert_eq!(queue[0].llm_text.as_deref(), Some("first-expanded"));
        assert_eq!(queue[0].images.len(), 1);
        assert_eq!(queue[0].files[0].original_name, "a.txt");
        assert!(queue[0].skill_invocation.is_none());
        assert_eq!(queue[1].message_id, "sb");
        let skill = queue[1].skill_invocation.as_ref().unwrap();
        assert_eq!(skill.name, "build");
        assert_eq!(skill.skill_dir, "/skills/build");

        // The legacy blob column carries no data (defaulted to '[]').
        let blob: String =
            sqlx::query_scalar("SELECT steering_queue FROM conversations WHERE id = 'conv-s'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(blob, "[]");

        // Remove entry A; its grandchild rows cascade away, B survives.
        db.remove_steering_entries("conv-s", &["sa".to_string()])
            .await
            .unwrap();
        assert_eq!(
            db.steering_conversation_id_for_message("sa").await.unwrap(),
            None
        );
        let queue = db.get_steering_queue("conv-s").await.unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].message_id, "sb");
        let orphan_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_message_files WHERE message_id = 'sa'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(orphan_files, 0);

        // Replace-all with an empty queue clears everything.
        db.update_steering_queue("conv-s", &[]).await.unwrap();
        assert!(db.get_steering_queue("conv-s").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn close_admission_fence_serializes_concurrent_steering_admission() {
        use phoenix_core::domain::sm_event::SteerEntry;

        let (_dir, mut close_db, mut steering_db) = open_test_db_pair().await;
        let close_latch = std::sync::Arc::new(CloseFoundationTestLatch::new());
        let steering_latch = std::sync::Arc::new(SteeringBeginTestLatch::new());
        close_db.close_foundation_test_latch = Some(close_latch.clone());
        steering_db.steering_begin_test_latch = Some(steering_latch.clone());
        close_db
            .create_conversation("close-steering", "close steering", "/tmp", true, None, None)
            .await
            .unwrap();
        let product_conversation_id = close_db
            .get_conversation("close-steering")
            .await
            .unwrap()
            .product_conversation_id;
        let entry = SteerEntry {
            text: "refused".to_string(),
            llm_text: None,
            images: Vec::new(),
            files: Vec::new(),
            message_id: "refused-steering".to_string(),
            user_agent: None,
            skill_invocation: None,
        };

        let transaction_entered = close_latch.transaction_entered.notified();
        let close_writer = close_db.clone();
        let close = tokio::spawn(async move {
            close_writer
                .begin_close_foundation(&product_conversation_id, "close-vs-steering")
                .await
        });
        transaction_entered.await;
        let steering_before_begin = steering_latch.before_begin.notified();
        let steering_begin_called = steering_latch.begin_called.notified();
        let steering = tokio::spawn(async move {
            steering_db
                .append_steering_entry("close-steering", &entry, "refused-fingerprint")
                .await
        });
        steering_before_begin.await;
        steering_latch.allow_begin.notify_waiters();
        steering_begin_called.await;
        close_latch.release_transaction.notify_waiters();
        close.await.unwrap().unwrap();
        assert!(matches!(
            steering.await.unwrap(),
            Err(DbError::CloseAdmissionFenced(_))
        ));
        assert!(close_db
            .get_steering_queue("close-steering")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn steering_append_returns_committed_fifo_position_without_resurrecting_drains() {
        use phoenix_core::domain::sm_event::SteerEntry;

        fn entry(message_id: &str) -> SteerEntry {
            SteerEntry {
                text: message_id.to_string(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                message_id: message_id.to_string(),
                user_agent: None,
                skill_invocation: None,
            }
        }

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-append", "append", "/tmp", true, None, None)
            .await
            .unwrap();

        assert_eq!(
            db.append_steering_entry("conv-append", &entry("a"), "fp-a")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            db.append_steering_entry("conv-append", &entry("b"), "fp-b")
                .await
                .unwrap(),
            1
        );
        assert!(db.remove_steering_entry("conv-append", "a").await.unwrap());
        assert!(!db.remove_steering_entry("conv-append", "a").await.unwrap());
        assert_eq!(
            db.get_steering_acceptance_fingerprint("conv-append", "a")
                .await
                .unwrap(),
            Some(SteeringAcceptanceFingerprint::Exact("fp-a".to_string()))
        );
        assert_eq!(
            db.append_steering_entry("conv-append", &entry("c"), "fp-c")
                .await
                .unwrap(),
            1
        );

        let queue = db.get_steering_queue("conv-append").await.unwrap();
        assert_eq!(
            queue
                .iter()
                .map(|entry| entry.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
    }

    #[tokio::test]
    async fn steering_append_refuses_entry_beyond_capacity_in_its_write_transaction() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("capacity", "capacity", "/tmp", true, None, None)
            .await
            .unwrap();
        for index in 0..MAX_STEERING_QUEUE_DEPTH {
            db.append_steering_entry(
                "capacity",
                &steering_entry(&format!("entry-{index}")),
                &format!("fingerprint-{index}"),
            )
            .await
            .unwrap();
        }

        assert!(matches!(
            db.append_steering_entry("capacity", &steering_entry("overflow"), "overflow")
                .await
                .unwrap_err(),
            DbError::SteeringQueueFull
        ));
        assert_eq!(
            db.get_steering_queue("capacity").await.unwrap().len(),
            MAX_STEERING_QUEUE_DEPTH
        );
    }
    #[tokio::test]
    async fn steering_queue_mutators_share_exported_capacity() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "capacity-update",
            "capacity-update",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let queue = (0..MAX_STEERING_QUEUE_DEPTH)
            .map(|index| steering_entry(&format!("entry-{index}")))
            .collect::<Vec<_>>();
        db.update_steering_queue("capacity-update", &queue)
            .await
            .unwrap();

        assert_eq!(
            db.steering_queue_depth("capacity-update").await.unwrap(),
            MAX_STEERING_QUEUE_DEPTH
        );
        assert!(matches!(
            db.update_steering_queue(
                "capacity-update",
                &[queue.clone(), vec![steering_entry("overflow")]].concat(),
            )
            .await
            .unwrap_err(),
            DbError::SteeringQueueFull
        ));
    }

    #[tokio::test]
    async fn steering_append_rolls_back_receipt_when_queue_insert_fails() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("receipt-a", "receipt-a", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("receipt-b", "receipt-b", "/tmp", true, None, None)
            .await
            .unwrap();
        db.append_steering_entry("receipt-a", &steering_entry("shared"), "fp-a")
            .await
            .unwrap();

        db.append_steering_entry("receipt-b", &steering_entry("shared"), "fp-b")
            .await
            .expect_err("global queue identity conflict must abort append");

        assert_eq!(
            db.get_steering_acceptance_fingerprint("receipt-b", "shared")
                .await
                .unwrap(),
            None
        );
        assert!(db.get_steering_queue("receipt-b").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn legacy_queue_replace_records_unknown_receipt_without_overwriting_exact_receipt() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("receipt-legacy", "receipt-legacy", "/tmp", true, None, None)
            .await
            .unwrap();
        db.append_steering_entry(
            "receipt-legacy",
            &steering_entry("exact"),
            "exact-fingerprint",
        )
        .await
        .unwrap();

        db.update_steering_queue(
            "receipt-legacy",
            &[steering_entry("exact"), steering_entry("legacy")],
        )
        .await
        .unwrap();

        assert_eq!(
            db.get_steering_acceptance_fingerprint("receipt-legacy", "exact")
                .await
                .unwrap(),
            Some(SteeringAcceptanceFingerprint::Exact(
                "exact-fingerprint".to_string()
            ))
        );
        assert_eq!(
            db.get_steering_acceptance_fingerprint("receipt-legacy", "legacy")
                .await
                .unwrap(),
            Some(SteeringAcceptanceFingerprint::LegacyUnknown)
        );
    }

    fn steering_drain_message(
        conversation_id: &str,
        message_id: &str,
        sequence_id: i64,
    ) -> Message {
        let content = MessageContent::User(UserContent::new(message_id));
        Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    fn steering_entry(message_id: &str) -> phoenix_core::domain::sm_event::SteerEntry {
        phoenix_core::domain::sm_event::SteerEntry {
            text: message_id.to_string(),
            llm_text: None,
            images: Vec::new(),
            files: Vec::new(),
            message_id: message_id.to_string(),
            user_agent: None,
            skill_invocation: None,
        }
    }

    #[tokio::test]
    async fn steering_drain_commits_fifo_messages_state_and_exact_queue_ids() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("drain-ok", "drain-ok", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_steering_queue(
            "drain-ok",
            &[
                steering_entry("a"),
                steering_entry("b"),
                steering_entry("concurrent"),
            ],
        )
        .await
        .unwrap();
        assert!(!db.has_committed_steering_turn("drain-ok").await.unwrap());
        let next_state = ConvState::LlmRequesting { attempt: 1 };
        let state_updated_at = Utc::now();

        let statuses = db
            .commit_steering_drain(
                "drain-ok",
                &[
                    steering_drain_message("drain-ok", "a", 10),
                    steering_drain_message("drain-ok", "b", 11),
                ],
                &next_state,
                state_updated_at,
            )
            .await
            .unwrap();

        assert_eq!(
            statuses,
            vec![
                SteeringDrainMessageStatus::Inserted,
                SteeringDrainMessageStatus::Inserted,
            ]
        );
        assert_eq!(
            db.get_messages("drain-ok")
                .await
                .unwrap()
                .iter()
                .map(|message| message.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        let queue = db.get_steering_queue("drain-ok").await.unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].message_id, "concurrent");
        let conversation = db.get_conversation("drain-ok").await.unwrap();
        assert_eq!(conversation.state, next_state);
        assert_eq!(conversation.state_updated_at, state_updated_at);
        assert_eq!(
            db.get_steering_acceptance_fingerprint("drain-ok", "a")
                .await
                .unwrap(),
            Some(SteeringAcceptanceFingerprint::LegacyUnknown)
        );
        assert!(db.has_committed_steering_turn("drain-ok").await.unwrap());

        db.add_message(
            "first-response",
            "drain-ok",
            &MessageContent::agent(vec![phoenix_core::domain::llm_types::ContentBlock::text(
                "settled",
            )]),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(!db.has_committed_steering_turn("drain-ok").await.unwrap());
    }

    #[tokio::test]
    async fn steering_drain_missing_supplied_queue_id_rolls_back_every_write() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("drain-rollback", "drain-rollback", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_steering_queue("drain-rollback", &[steering_entry("a")])
            .await
            .unwrap();

        let error = db
            .commit_steering_drain(
                "drain-rollback",
                &[
                    steering_drain_message("drain-rollback", "a", 10),
                    steering_drain_message("drain-rollback", "missing", 11),
                ],
                &ConvState::LlmRequesting { attempt: 1 },
                Utc::now(),
            )
            .await
            .expect_err("stale reducer batch must fail");
        assert!(error.to_string().contains("was no longer pending"));
        assert!(db.get_messages("drain-rollback").await.unwrap().is_empty());
        assert_eq!(
            db.get_steering_queue("drain-rollback").await.unwrap().len(),
            1
        );
        assert_eq!(
            db.get_conversation("drain-rollback").await.unwrap().state,
            ConvState::Idle
        );
    }

    #[tokio::test]
    async fn steering_drain_recovers_matching_legacy_materialized_message_once() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("drain-legacy", "drain-legacy", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_steering_queue("drain-legacy", &[steering_entry("a")])
            .await
            .unwrap();
        db.add_message_with_seq(
            "a",
            "drain-legacy",
            4,
            &MessageContent::User(UserContent::new("a")),
            None,
            None,
        )
        .await
        .unwrap();

        let statuses = db
            .commit_steering_drain(
                "drain-legacy",
                &[steering_drain_message("drain-legacy", "a", 10)],
                &ConvState::LlmRequesting { attempt: 1 },
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(
            statuses,
            vec![SteeringDrainMessageStatus::LegacyAlreadyMaterialized]
        );
        let messages = db.get_messages("drain-legacy").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].sequence_id, 4);
        assert!(db
            .get_steering_queue("drain-legacy")
            .await
            .unwrap()
            .is_empty());
    }

    /// Regression for task 02679: messages must persist with the seq their
    /// broadcaster pre-allocated, not with a `DB-MAX+1` seq.
    /// `add_message_with_seq` writes the caller-supplied seq verbatim; the
    /// broadcaster's seq is strictly greater than any ephemeral event
    /// (token / `state_change` / error) emitted earlier, so the client's
    /// `applyIfNewer` guard does not drop the message as stale. See
    /// `PersistBeforeBroadcast` in `specs/sse_wire/sse_wire.allium`.
    #[tokio::test]
    async fn test_add_message_with_seq_writes_caller_seq() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-seq", "slug-seq", "/tmp", true, None, None)
            .await
            .unwrap();

        // Simulate: broadcaster has emitted several ephemeral events,
        // advancing its counter well past the DB message count.
        let pre_allocated_seq = 42;

        let msg = db
            .add_message_with_seq(
                "msg-seq",
                "conv-seq",
                pre_allocated_seq,
                &MessageContent::user("message after many tokens"),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            msg.sequence_id, pre_allocated_seq,
            "add_message_with_seq must use the caller-supplied seq verbatim"
        );

        // A subsequent add_message falls back to DB-MAX+1, which picks up
        // the pre-allocated seq. This is the glue that keeps the
        // non-broadcasting paths (sub-agent bootstrap, crash recovery)
        // compatible with broadcasting paths: DB's MAX is the running
        // watermark no matter which API wrote the last message.
        let next = db
            .add_message(
                "msg-next",
                "conv-seq",
                &MessageContent::user("next message via MAX+1"),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            next.sequence_id,
            pre_allocated_seq + 1,
            "DB-MAX+1 allocation must observe seqs planted by add_message_with_seq"
        );
    }

    #[tokio::test]
    async fn reset_preserves_llm_requesting_for_unfinished_creation() {
        let db = Database::open_in_memory().await.unwrap();
        insert_test_creation_job(&db, "job-bootstrap", "conv-bootstrap").await;
        let requesting = ConvState::LlmRequesting { attempt: 1 };
        db.update_conversation_state("conv-bootstrap", &requesting)
            .await
            .unwrap();

        db.reset_all_to_idle().await.unwrap();

        assert!(matches!(
            db.get_conversation("conv-bootstrap").await.unwrap().state,
            ConvState::LlmRequesting { .. }
        ));
    }

    #[tokio::test]
    async fn reset_preserves_creation_cancelled_state() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("cancelled", "slug", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            "cancelled",
            &ConvState::CreationCancelled {
                job_id: "job".to_string(),
            },
        )
        .await
        .unwrap();

        db.reset_all_to_idle().await.unwrap();

        assert!(matches!(
            db.get_conversation("cancelled").await.unwrap().state,
            ConvState::CreationCancelled { ref job_id } if job_id == "job"
        ));
    }

    #[tokio::test]
    async fn test_reset_preserves_context_exhausted_state() {
        let db = Database::open_in_memory().await.unwrap();

        // Create a conversation with context_exhausted state
        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        // Manually set state to context_exhausted
        let exhausted_state = ConvState::ContextExhausted {
            summary: "Test summary".to_string(),
        };
        db.update_conversation_state("conv-1", &exhausted_state)
            .await
            .unwrap();

        // Verify state is set
        let conv_before = db.get_conversation("conv-1").await.unwrap();
        assert!(
            matches!(conv_before.state, ConvState::ContextExhausted { .. }),
            "State should be ContextExhausted before reset"
        );

        // Run reset
        db.reset_all_to_idle().await.unwrap();

        // Verify context_exhausted state is preserved (not reset to idle)
        let conv_after = db.get_conversation("conv-1").await.unwrap();
        assert!(
            matches!(conv_after.state, ConvState::ContextExhausted { .. }),
            "ContextExhausted state should be preserved after reset"
        );

        // Verify the summary is intact
        if let ConvState::ContextExhausted { summary } = conv_after.state {
            assert_eq!(summary, "Test summary");
        }
    }

    #[tokio::test]
    async fn test_reset_preserves_awaiting_task_approval_state() {
        let db = Database::open_in_memory().await.unwrap();

        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        let approval_state = ConvState::AwaitingTaskApproval {
            task_file: "tasks/12345-p1-ready--fix-the-widget.md".to_string(),
            title: "Fix the widget".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "Step 1: read code\nStep 2: fix bug".to_string(),
        };
        db.update_conversation_state("conv-1", &approval_state)
            .await
            .unwrap();

        db.reset_all_to_idle().await.unwrap();

        let conv_after = db.get_conversation("conv-1").await.unwrap();
        assert!(
            matches!(conv_after.state, ConvState::AwaitingTaskApproval { .. }),
            "AwaitingTaskApproval state should be preserved after reset"
        );

        if let ConvState::AwaitingTaskApproval {
            task_file,
            title,
            priority,
            plan,
        } = conv_after.state
        {
            assert_eq!(task_file, "tasks/12345-p1-ready--fix-the-widget.md");
            assert_eq!(title, "Fix the widget");
            assert_eq!(priority, phoenix_core::task_source::Priority::P1);
            assert_eq!(plan, "Step 1: read code\nStep 2: fix bug");
        }
    }

    #[tokio::test]
    async fn test_reset_repairs_orphaned_tool_use() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let db = Database::open_in_memory().await.unwrap();

        // Create a conversation
        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        // Add user message
        db.add_message(
            "msg-1",
            "conv-1",
            &MessageContent::user("Run a command"),
            None,
            None,
        )
        .await
        .unwrap();

        // Add agent message with tool_use (simulating LLM response)
        db.add_message(
            "msg-2",
            "conv-1",
            &MessageContent::agent(vec![
                ContentBlock::text("Let me run that for you."),
                ContentBlock::tool_use(
                    "tool-123",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "ls"}),
                ),
            ]),
            None,
            None,
        )
        .await
        .unwrap();

        // NO tool_result added - simulating crash during tool execution

        // Verify we have an orphaned tool_use
        let messages_before = db.get_messages("conv-1").await.unwrap();
        assert_eq!(messages_before.len(), 2);

        // Run reset (which should repair orphans)
        db.reset_all_to_idle().await.unwrap();

        // Verify synthetic tool_result was injected
        let messages_after = db.get_messages("conv-1").await.unwrap();
        assert_eq!(
            messages_after.len(),
            3,
            "Should have injected synthetic tool_result"
        );

        // Check the synthetic result
        let tool_msg = &messages_after[2];
        assert_eq!(tool_msg.message_type, MessageType::Tool);
        match &tool_msg.content {
            MessageContent::Tool(tc) => {
                assert_eq!(tc.tool_use_id, "tool-123");
                assert!(tc.is_error);
                assert!(tc.content.contains("interrupted"));
            }
            MessageContent::User(_)
            | MessageContent::Agent(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_)
            | MessageContent::Skill(_) => panic!("Expected Tool content"),
        }
    }

    #[tokio::test]
    async fn test_reset_does_not_duplicate_complete_exchanges() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let db = Database::open_in_memory().await.unwrap();

        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        // Add a complete exchange: user -> agent(tool_use) -> tool_result
        db.add_message(
            "msg-1",
            "conv-1",
            &MessageContent::user("Run a command"),
            None,
            None,
        )
        .await
        .unwrap();

        db.add_message(
            "msg-2",
            "conv-1",
            &MessageContent::agent(vec![ContentBlock::tool_use(
                "tool-123",
                "bash",
                serde_json::json!({"op": "run", "cmd": "ls"}),
            )]),
            None,
            None,
        )
        .await
        .unwrap();

        db.add_message(
            "msg-3",
            "conv-1",
            &MessageContent::tool("tool-123", "file1.txt\nfile2.txt", false),
            None,
            None,
        )
        .await
        .unwrap();

        // Run reset
        db.reset_all_to_idle().await.unwrap();

        // Should still have exactly 3 messages (no synthetic added)
        let messages = db.get_messages("conv-1").await.unwrap();
        assert_eq!(
            messages.len(),
            3,
            "Complete exchange should not be modified"
        );
    }

    #[tokio::test]
    async fn test_reset_repairs_multiple_orphaned_tools() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let db = Database::open_in_memory().await.unwrap();

        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        // Agent message with multiple tool_use blocks
        db.add_message(
            "msg-1",
            "conv-1",
            &MessageContent::agent(vec![
                ContentBlock::tool_use(
                    "tool-1",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "ls"}),
                ),
                ContentBlock::tool_use(
                    "tool-2",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "pwd"}),
                ),
                ContentBlock::tool_use(
                    "tool-3",
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "date"}),
                ),
            ]),
            None,
            None,
        )
        .await
        .unwrap();

        // Only tool-1 completed before crash
        db.add_message(
            "msg-2",
            "conv-1",
            &MessageContent::tool("tool-1", "output", false),
            None,
            None,
        )
        .await
        .unwrap();

        // Run reset
        db.reset_all_to_idle().await.unwrap();

        // Should have 2 synthetic results for tool-2 and tool-3
        let messages = db.get_messages("conv-1").await.unwrap();
        assert_eq!(
            messages.len(),
            4,
            "Should have 1 agent + 1 real tool + 2 synthetic"
        );

        // Check that tool-2 and tool-3 have synthetic results
        let tool_results: Vec<_> = messages
            .iter()
            .filter(|m| m.message_type == MessageType::Tool)
            .collect();
        assert_eq!(tool_results.len(), 3);

        let tool_ids: Vec<_> = tool_results
            .iter()
            .filter_map(|m| match &m.content {
                MessageContent::Tool(tc) => Some(tc.tool_use_id.clone()),
                MessageContent::User(_)
                | MessageContent::Agent(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_) => None,
            })
            .collect();
        assert!(tool_ids.contains(&"tool-1".to_string()));
        assert!(tool_ids.contains(&"tool-2".to_string()));
        assert!(tool_ids.contains(&"tool-3".to_string()));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // single end-to-end restart scenario; splitting hurts clarity
    async fn test_reset_materializes_in_flight_tool_round() {
        use phoenix_core::domain::db_schema::ToolResult;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, ConvState, ThinkInput, ToolCall, ToolInput,
        };

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        // The user message that prompted the turn is already persisted.
        db.add_message(
            "msg-user",
            "conv-1",
            &MessageContent::user("do three things"),
            None,
            None,
        )
        .await
        .unwrap();

        // Build an in-flight ToolExecuting round: assistant message holds three
        // tool_use blocks; tool-1 finished (real result), tool-2 is in flight
        // (current), tool-3 is queued (remaining). The assistant message + the
        // tool-1 result live ONLY in the state JSON — never persisted to
        // `messages` (that happens at end-of-round checkpoint).
        let think = |t: &str| ToolInput::Think(ThinkInput { thoughts: t.into() });
        let assistant = AssistantMessage::new(
            "asst-1".to_string(),
            vec![
                ContentBlock::text("Working on it."),
                ContentBlock::tool_use("tool-1", "think", serde_json::json!({"thoughts": "a"})),
                ContentBlock::tool_use("tool-2", "think", serde_json::json!({"thoughts": "b"})),
                ContentBlock::tool_use("tool-3", "think", serde_json::json!({"thoughts": "c"})),
            ],
            None,
            None,
        );
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new("tool-2", think("b")),
            remaining_tools: vec![ToolCall::new("tool-3", think("c"))],
            completed_results: vec![ToolResult::success(
                "tool-1".to_string(),
                "real output for tool-1".to_string(),
            )],
            pending_sub_agents: vec![],
            assistant_message: assistant,
        };
        db.update_conversation_state("conv-1", &state)
            .await
            .unwrap();

        // Only the user message is in `messages` so far.
        assert_eq!(db.get_messages("conv-1").await.unwrap().len(), 1);

        // Restart sweep.
        db.reset_all_to_idle().await.unwrap();

        // State reset to idle.
        let conv = db.get_conversation("conv-1").await.unwrap();
        assert!(
            matches!(conv.state, ConvState::Idle),
            "tool_executing must reset to idle after materialization"
        );

        let msgs = db.get_messages("conv-1").await.unwrap();
        // user + assistant + 3 tool results.
        assert_eq!(
            msgs.len(),
            5,
            "expected user + assistant + 3 paired tool results, got {:?}",
            msgs.iter().map(|m| &m.message_id).collect::<Vec<_>>()
        );

        // Assistant turn materialized with its three tool_use blocks intact.
        let agent = msgs
            .iter()
            .find(|m| m.message_id == "asst-1")
            .expect("assistant message must be materialized");
        match &agent.content {
            MessageContent::Agent(blocks) => {
                let n_tool_uses = blocks
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                    .count();
                assert_eq!(
                    n_tool_uses, 3,
                    "assistant must carry all three tool_use blocks"
                );
            }
            MessageContent::User(_)
            | MessageContent::Tool(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_)
            | MessageContent::Skill(_) => panic!("asst-1 must be agent content"),
        }

        // Exactly one tool_result per tool_use, paired by id.
        let by_id = |id: &str| {
            msgs.iter().find_map(|m| match &m.content {
                MessageContent::Tool(tc) if tc.tool_use_id == id => Some(tc.clone()),
                MessageContent::Tool(_)
                | MessageContent::User(_)
                | MessageContent::Agent(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_) => None,
            })
        };
        let r1 = by_id("tool-1").expect("tool-1 result present");
        let r2 = by_id("tool-2").expect("tool-2 result present");
        let r3 = by_id("tool-3").expect("tool-3 result present");

        // Completed tool keeps its real output, not a placeholder.
        assert_eq!(r1.content, "real output for tool-1");
        assert!(!r1.is_error, "completed tool result must not be an error");

        // In-flight + queued tools get synthetic interrupted errors.
        assert!(r2.is_error && r2.content.contains("interrupted"));
        assert!(r3.is_error && r3.content.contains("interrupted"));

        // Idempotent: re-running the sweep must not duplicate or 400-trip.
        db.reset_all_to_idle().await.unwrap();
        assert_eq!(
            db.get_messages("conv-1").await.unwrap().len(),
            5,
            "re-running reset must not duplicate materialized rows"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // single end-to-end restart scenario; splitting hurts clarity
    async fn test_reset_materializes_cancelling_tool_round() {
        use phoenix_core::domain::db_schema::ToolResult;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, ConvState, ThinkInput, ToolCall, ToolInput,
        };

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-c", "slug-c", "/tmp", true, None, None)
            .await
            .unwrap();

        // The user message that prompted the turn is already persisted.
        db.add_message(
            "msg-user",
            "conv-c",
            &MessageContent::user("do three things"),
            None,
            None,
        )
        .await
        .unwrap();

        // Build an in-flight CancellingTool round: the user hit cancel while
        // tool-2 was running. The assistant message holds three tool_use
        // blocks; tool-1 finished (real result, in `completed_results`), tool-2
        // is the tool being aborted (`tool_use_id`), and tool-3 was skipped.
        // The assistant message + the tool-1 result live ONLY in the state JSON
        // — never persisted to `messages` (the abort/complete checkpoint that
        // would persist them never ran because the process exited first).
        let think = |t: &str| ToolInput::Think(ThinkInput { thoughts: t.into() });
        let assistant = AssistantMessage::new(
            "asst-c".to_string(),
            vec![
                ContentBlock::text("Working on it."),
                ContentBlock::tool_use("tool-1", "think", serde_json::json!({"thoughts": "a"})),
                ContentBlock::tool_use("tool-2", "think", serde_json::json!({"thoughts": "b"})),
                ContentBlock::tool_use("tool-3", "think", serde_json::json!({"thoughts": "c"})),
            ],
            None,
            None,
        );
        let state = ConvState::CancellingTool {
            tool_use_id: "tool-2".to_string(),
            skipped_tools: vec![ToolCall::new("tool-3", think("c"))],
            completed_results: vec![ToolResult::success(
                "tool-1".to_string(),
                "real output for tool-1".to_string(),
            )],
            assistant_message: assistant,
            pending_sub_agents: vec![],
        };
        db.update_conversation_state("conv-c", &state)
            .await
            .unwrap();

        // Only the user message is in `messages` so far.
        assert_eq!(db.get_messages("conv-c").await.unwrap().len(), 1);

        // Restart sweep.
        db.reset_all_to_idle().await.unwrap();

        // State reset to idle.
        let conv = db.get_conversation("conv-c").await.unwrap();
        assert!(
            matches!(conv.state, ConvState::Idle),
            "cancelling_tool must reset to idle after materialization"
        );

        let msgs = db.get_messages("conv-c").await.unwrap();
        // user + assistant + 3 tool results.
        assert_eq!(
            msgs.len(),
            5,
            "expected user + assistant + 3 paired tool results, got {:?}",
            msgs.iter().map(|m| &m.message_id).collect::<Vec<_>>()
        );

        // Assistant turn materialized with its three tool_use blocks intact.
        let agent = msgs
            .iter()
            .find(|m| m.message_id == "asst-c")
            .expect("assistant message must be materialized");
        match &agent.content {
            MessageContent::Agent(blocks) => {
                let n_tool_uses = blocks
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                    .count();
                assert_eq!(
                    n_tool_uses, 3,
                    "assistant must carry all three tool_use blocks"
                );
            }
            MessageContent::User(_)
            | MessageContent::Tool(_)
            | MessageContent::System(_)
            | MessageContent::Error(_)
            | MessageContent::Continuation(_)
            | MessageContent::Skill(_) => panic!("asst-c must be agent content"),
        }

        // Exactly one tool_result per tool_use, paired by id.
        let by_id = |id: &str| {
            msgs.iter().find_map(|m| match &m.content {
                MessageContent::Tool(tc) if tc.tool_use_id == id => Some(tc.clone()),
                MessageContent::Tool(_)
                | MessageContent::User(_)
                | MessageContent::Agent(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_) => None,
            })
        };
        let r1 = by_id("tool-1").expect("tool-1 result present");
        let r2 = by_id("tool-2").expect("tool-2 result present");
        let r3 = by_id("tool-3").expect("tool-3 result present");

        // Completed tool keeps its real output, not a placeholder.
        assert_eq!(r1.content, "real output for tool-1");
        assert!(!r1.is_error, "completed tool result must not be an error");

        // Cancelling + skipped tools get synthetic interrupted errors.
        assert!(r2.is_error && r2.content.contains("interrupted"));
        assert!(r3.is_error && r3.content.contains("interrupted"));

        // Idempotent: re-running the sweep must not duplicate or 400-trip.
        db.reset_all_to_idle().await.unwrap();
        assert_eq!(
            db.get_messages("conv-c").await.unwrap().len(),
            5,
            "re-running reset must not duplicate materialized rows"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // single end-to-end restart scenario; splitting hurts clarity
    async fn test_reset_materializes_in_flight_round_with_pending_sub_agents() {
        use phoenix_core::domain::db_schema::ToolResult;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, ConvState, PendingSubAgent, SubAgentMode, ThinkInput, ToolCall,
            ToolInput,
        };

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-sa", "slug-sa", "/tmp", true, None, None)
            .await
            .unwrap();

        db.add_message(
            "msg-user",
            "conv-sa",
            &MessageContent::user("spawn two agents then think"),
            None,
            None,
        )
        .await
        .unwrap();

        // Round shape: [spawn_agents, think]. `spawn_agents` (tool-1) ran first
        // and is in `completed_results` as the raw "Spawning …" placeholder; it
        // launched two sub-agents that are still pending. `think` (tool-2) is the
        // in-flight tool. Neither the assistant turn nor any result is persisted
        // yet — that happens at the end-of-round checkpoint, which never ran.
        let think = |t: &str| ToolInput::Think(ThinkInput { thoughts: t.into() });
        let assistant = AssistantMessage::new(
            "asst-sa".to_string(),
            vec![
                ContentBlock::text("Spawning agents, then thinking."),
                ContentBlock::tool_use(
                    "tool-1",
                    "spawn_agents",
                    serde_json::json!({"tasks": [{"task": "a"}, {"task": "b"}]}),
                ),
                ContentBlock::tool_use("tool-2", "think", serde_json::json!({"thoughts": "t"})),
            ],
            None,
            None,
        );
        // The spawn placeholder names the agent UUIDs it launched, exactly as the
        // executor's `handle_spawn_agents_tool` would.
        let agent_a = "11111111-1111-4111-8111-111111111111";
        let agent_b = "22222222-2222-4222-8222-222222222222";
        for agent_id in [agent_a, agent_b] {
            db.create_conversation(agent_id, agent_id, "/tmp", false, Some("conv-sa"), None)
                .await
                .unwrap();
            db.update_conversation_state(agent_id, &ConvState::LlmRequesting { attempt: 0 })
                .await
                .unwrap();
        }
        db.update_conversation_state(
            agent_b,
            &ConvState::ContextExhausted {
                summary: "preserve this reason".to_string(),
            },
        )
        .await
        .unwrap();
        let placeholder = format!("Spawning 2 sub-agent(s): {agent_a}, {agent_b}");
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new("tool-2", think("t")),
            remaining_tools: vec![],
            completed_results: vec![ToolResult::success("tool-1".to_string(), placeholder)],
            pending_sub_agents: vec![
                PendingSubAgent {
                    agent_id: agent_a.to_string(),
                    task: "investigate the parser".to_string(),
                    mode: SubAgentMode::Explore,
                },
                PendingSubAgent {
                    agent_id: agent_b.to_string(),
                    task: "audit the lexer".to_string(),
                    mode: SubAgentMode::Explore,
                },
            ],
            assistant_message: assistant,
        };
        db.update_conversation_state("conv-sa", &state)
            .await
            .unwrap();

        assert_eq!(db.get_messages("conv-sa").await.unwrap().len(), 1);

        db.reset_all_to_idle().await.unwrap();

        let conv = db.get_conversation("conv-sa").await.unwrap();
        assert!(matches!(conv.state, ConvState::Idle));

        let msgs = db.get_messages("conv-sa").await.unwrap();
        // user + assistant + 2 paired tool results (one per tool_use).
        assert_eq!(
            msgs.len(),
            4,
            "expected user + assistant + 2 paired tool results, got {:?}",
            msgs.iter().map(|m| &m.message_id).collect::<Vec<_>>()
        );

        let by_id = |id: &str| {
            msgs.iter().find_map(|m| match &m.content {
                MessageContent::Tool(tc) if tc.tool_use_id == id => Some((tc.clone(), m.clone())),
                MessageContent::Tool(_)
                | MessageContent::User(_)
                | MessageContent::Agent(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_) => None,
            })
        };
        let (spawn_tc, spawn_msg) = by_id("tool-1").expect("spawn_agents result present");
        let (think_tc, _) = by_id("tool-2").expect("think result present");

        // The spawn_agents placeholder is rewritten into the interrupted fan-in:
        // an LLM-readable summary naming both sub-agents' tasks and their
        // interrupted outcome — NOT the raw "Spawning …" placeholder.
        assert!(
            !spawn_tc.is_error,
            "spawn_agents fan-in is not itself an error result"
        );
        assert!(
            spawn_tc.content.contains("Sub-agent results (2 completed)"),
            "spawn result must carry the fan-in summary, got: {}",
            spawn_tc.content
        );
        assert!(
            spawn_tc.content.contains("investigate the parser")
                && spawn_tc.content.contains("audit the lexer"),
            "fan-in must name both sub-agent tasks, got: {}",
            spawn_tc.content
        );
        assert!(
            spawn_tc.content.contains("interrupted by server restart"),
            "fan-in must reflect the interrupted outcome, got: {}",
            spawn_tc.content
        );
        assert!(
            !spawn_tc.content.starts_with("Spawning"),
            "raw placeholder must be replaced, got: {}",
            spawn_tc.content
        );

        // display_data carries the typed subagent_summary blob the UI renders.
        let display = spawn_msg
            .display_data
            .as_ref()
            .expect("fan-in display_data present");
        assert_eq!(display["type"], "subagent_summary");
        assert_eq!(
            display["results"].as_array().map(Vec::len),
            Some(2),
            "display_data must carry both typed sub-agent results"
        );

        // The in-flight `think` tool still gets its synthetic interrupted error.
        assert!(think_tc.is_error && think_tc.content.contains("interrupted"));

        // Idempotent: re-running the sweep must not duplicate or 400-trip.
        db.reset_all_to_idle().await.unwrap();
        assert_eq!(db.get_messages("conv-sa").await.unwrap().len(), 4);
        assert!(matches!(
            db.get_conversation(agent_a).await.unwrap().state,
            ConvState::Failed { .. }
        ));
        assert!(matches!(
            db.get_conversation(agent_b).await.unwrap().state,
            ConvState::ContextExhausted { .. }
        ));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn startup_parent_reconciliation_materializes_exact_child_result_once() {
        use phoenix_core::domain::db_schema::ToolResult;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, PendingSubAgent, SubAgentMode, ThinkInput, ToolCall, ToolInput,
        };

        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "startup-parent";
        let child_id = "startup-child";
        db.create_conversation(parent_id, "startup-parent", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            child_id,
            "startup-child",
            "/tmp",
            false,
            Some(parent_id),
            None,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            child_id,
            &ConvState::Completed {
                result: "exact recovered result".to_string(),
            },
        )
        .await
        .unwrap();
        let assistant = AssistantMessage::new(
            "startup-assistant".to_string(),
            vec![
                ContentBlock::tool_use(
                    "spawn-tool",
                    "spawn_agents",
                    serde_json::json!({"tasks": [{"task": "audit"}]}),
                ),
                ContentBlock::tool_use(
                    "think-tool",
                    "think",
                    serde_json::json!({"thoughts": "finish"}),
                ),
            ],
            None,
            None,
        );
        db.update_conversation_state(
            parent_id,
            &ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    "think-tool",
                    ToolInput::Think(ThinkInput {
                        thoughts: "finish".to_string(),
                    }),
                ),
                remaining_tools: Vec::new(),
                completed_results: vec![ToolResult::success(
                    "spawn-tool".to_string(),
                    format!("Spawning 1 sub-agent(s): {child_id}"),
                )],
                pending_sub_agents: vec![PendingSubAgent {
                    agent_id: child_id.to_string(),
                    task: "audit".to_string(),
                    mode: SubAgentMode::Explore,
                }],
                assistant_message: assistant,
            },
        )
        .await
        .unwrap();
        let ids = std::collections::HashSet::from([parent_id.to_string()]);

        let reconciled = db.reconcile_startup_obligated_parents(&ids).await.unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(reconciled[0].conversation_id, parent_id);
        let actions = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].conversation_id, parent_id);
        assert_eq!(actions[0].action, StartupParentAction::Resume);
        assert!(matches!(
            db.get_conversation(parent_id).await.unwrap().state,
            ConvState::Idle
        ));
        let messages = db.get_messages(parent_id).await.unwrap();
        let spawn_result = messages
            .iter()
            .find(|message| {
                matches!(
                    &message.content,
                    MessageContent::Tool(content) if content.tool_use_id == "spawn-tool"
                )
            })
            .expect("spawn result was materialized");
        let MessageContent::Tool(content) = &spawn_result.content else {
            unreachable!()
        };
        assert!(content.content.contains("exact recovered result"));
        assert!(!content.content.contains("interrupted by server restart"));

        let reconciled = db.reconcile_startup_obligated_parents(&ids).await.unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(reconciled[0].conversation_id, parent_id);
        let actions = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].conversation_id, parent_id);
        assert_eq!(actions[0].action, StartupParentAction::Resume);
        assert_eq!(db.get_messages(parent_id).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn startup_action_replacement_never_reuses_a_consumed_high_id() {
        let db = Database::open_in_memory().await.unwrap();
        for conversation_id in ["action-low", "action-high"] {
            db.create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
                .await
                .unwrap();
            db.establish_parent_reconcile_action(conversation_id)
                .await
                .unwrap();
        }
        let actions = db.list_startup_parent_actions().await.unwrap();
        let low_id = actions
            .iter()
            .find(|action| action.conversation_id == "action-low")
            .unwrap()
            .action_id;
        let consumed_high_id = actions
            .iter()
            .find(|action| action.conversation_id == "action-high")
            .unwrap()
            .action_id;
        assert!(consumed_high_id > low_id);
        db.delete_startup_parent_action("action-high", consumed_high_id)
            .await
            .unwrap();

        db.persist_startup_sub_agent_fan_in(
            "action-low",
            &[],
            None,
            &ConvState::Idle,
            &ConvState::LlmRequesting { attempt: 1 },
            StartupParentAction::Resume,
            Utc::now(),
        )
        .await
        .unwrap();

        let replacement = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(replacement.len(), 1);
        assert!(replacement[0].action_id > consumed_high_id);
    }

    #[tokio::test]
    async fn startup_reconcile_to_resume_rotates_action_id() {
        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "reconcile-to-resume-parent";
        db.create_conversation(parent_id, parent_id, "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation(
            "reconcile-to-resume-child",
            "child",
            "/tmp",
            false,
            Some(parent_id),
            None,
        )
        .await
        .unwrap();
        db.update_conversation_state(parent_id, &ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        db.establish_parent_reconcile_action(parent_id)
            .await
            .unwrap();
        let original = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(original.len(), 1);

        db.reconcile_startup_obligated_parents(&std::collections::HashSet::from([
            parent_id.to_string()
        ]))
        .await
        .unwrap();

        let replacement = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(replacement.len(), 1);
        assert_eq!(replacement[0].action, StartupParentAction::Resume);
        assert_ne!(replacement[0].action_id, original[0].action_id);
    }

    #[tokio::test]
    async fn restart_tool_round_materialization_replays_with_persisted_timestamp() {
        use phoenix_core::domain::db_schema::ToolResult;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{AssistantMessage, ThinkInput, ToolCall, ToolInput};

        let db = Database::open_in_memory().await.unwrap();
        let conversation_id = "stable-materialization-time";
        db.create_conversation(conversation_id, "stable", "/tmp", true, None, None)
            .await
            .unwrap();
        let entered_at = Utc::now() - chrono::Duration::hours(1);
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "think-stable",
                ToolInput::Think(ThinkInput {
                    thoughts: "stable".to_string(),
                }),
            ),
            remaining_tools: Vec::new(),
            completed_results: Vec::<ToolResult>::new(),
            pending_sub_agents: Vec::new(),
            assistant_message: AssistantMessage::new(
                "stable-assistant".to_string(),
                vec![ContentBlock::tool_use(
                    "think-stable",
                    "think",
                    serde_json::json!({"thoughts": "stable"}),
                )],
                None,
                None,
            ),
        };
        db.update_conversation_state(conversation_id, &state)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE conversations SET state_updated_at = ?1, updated_at = ?1 WHERE id = ?2",
        )
        .bind(entered_at.to_rfc3339())
        .bind(conversation_id)
        .execute(db.pool())
        .await
        .unwrap();
        let only = std::collections::HashSet::from([conversation_id.to_string()]);

        db.materialize_in_flight_tool_rounds(&Utc::now(), Some(&only))
            .await
            .unwrap();
        db.materialize_in_flight_tool_rounds(
            &(Utc::now() + chrono::Duration::minutes(5)),
            Some(&only),
        )
        .await
        .expect("replay uses the persisted state timestamp");
        let messages = db.get_messages(conversation_id).await.unwrap();
        assert_eq!(messages.len(), 2);
        let tool_result = messages
            .iter()
            .find(|message| message.message_id == tool_result_message_id("think-stable"))
            .unwrap();
        assert_eq!(tool_result.created_at, entered_at);
    }

    #[tokio::test]
    async fn startup_tool_round_resume_rotates_action_id() {
        use phoenix_core::domain::sm_state::{AssistantMessage, ThinkInput, ToolCall, ToolInput};

        let db = Database::open_in_memory().await.unwrap();
        let conversation_id = "tool-round-action-version";
        db.create_conversation(conversation_id, conversation_id, "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            conversation_id,
            &ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    "think-action-version",
                    ToolInput::Think(ThinkInput {
                        thoughts: "rotate".to_string(),
                    }),
                ),
                remaining_tools: Vec::new(),
                completed_results: Vec::new(),
                pending_sub_agents: Vec::new(),
                assistant_message: AssistantMessage::new(
                    "action-version-assistant".to_string(),
                    vec![phoenix_core::domain::llm_types::ContentBlock::tool_use(
                        "think-action-version",
                        "think",
                        serde_json::json!({"thoughts": "rotate"}),
                    )],
                    None,
                    None,
                ),
            },
        )
        .await
        .unwrap();
        db.establish_parent_reconcile_action(conversation_id)
            .await
            .unwrap();
        let original = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(original.len(), 1);

        db.reconcile_startup_obligated_parents(&std::collections::HashSet::from([
            conversation_id.to_string()
        ]))
        .await
        .unwrap();

        let replacement = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(replacement.len(), 1);
        assert_eq!(replacement[0].action, StartupParentAction::Resume);
        assert_ne!(replacement[0].action_id, original[0].action_id);
    }

    #[tokio::test]
    async fn startup_fan_in_preserves_terminal_child_and_interrupts_live_sibling() {
        use phoenix_core::domain::sm_state::{PendingSubAgent, SubAgentMode};

        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "startup-fan-in-parent";
        let done_id = "startup-fan-in-done";
        let live_id = "startup-fan-in-live";
        db.create_conversation(parent_id, "parent", "/tmp", true, None, None)
            .await
            .unwrap();
        for child_id in [done_id, live_id] {
            db.create_conversation(child_id, child_id, "/tmp", false, Some(parent_id), None)
                .await
                .unwrap();
        }
        db.update_conversation_state(
            done_id,
            &ConvState::Completed {
                result: "exact sibling result".to_string(),
            },
        )
        .await
        .unwrap();
        db.update_conversation_state(live_id, &ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        db.add_message_with_seq(
            &tool_result_message_id("spawn-fan-in"),
            parent_id,
            1,
            &MessageContent::tool("spawn-fan-in", "Spawning 2 sub-agents", false),
            None,
            None,
        )
        .await
        .unwrap();
        db.update_conversation_state(
            parent_id,
            &ConvState::AwaitingSubAgents {
                pending: vec![
                    PendingSubAgent {
                        agent_id: done_id.to_string(),
                        task: "done".to_string(),
                        mode: SubAgentMode::Explore,
                    },
                    PendingSubAgent {
                        agent_id: live_id.to_string(),
                        task: "live".to_string(),
                        mode: SubAgentMode::Explore,
                    },
                ],
                completed_results: Vec::new(),
                spawn_tool_id: Some("spawn-fan-in".to_string()),
            },
        )
        .await
        .unwrap();
        let ids = std::collections::HashSet::from([parent_id.to_string()]);

        db.reconcile_startup_obligated_parents(&ids).await.unwrap();
        assert!(matches!(
            db.get_conversation(parent_id).await.unwrap().state,
            ConvState::LlmRequesting { attempt: 1 }
        ));
        let message = db
            .get_messages(parent_id)
            .await
            .unwrap()
            .into_iter()
            .find(|message| message.message_id == tool_result_message_id("spawn-fan-in"))
            .unwrap();
        let MessageContent::Tool(content) = message.content else {
            unreachable!()
        };
        assert!(content.content.contains("exact sibling result"));
        assert!(content.content.contains("interrupted by server restart"));
        assert!(matches!(
            db.get_conversation(live_id).await.unwrap().state,
            ConvState::Failed {
                error_kind: ErrorKind::SubAgentError,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn startup_summary_identity_is_stable_per_round_and_unique_across_rounds() {
        use phoenix_core::domain::sm_state::{SubAgentOutcome, SubAgentResult};

        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "multi-round-summary-parent";
        db.create_conversation(parent_id, parent_id, "/tmp", true, None, None)
            .await
            .unwrap();
        let destination = ConvState::LlmRequesting { attempt: 1 };
        let mut action_ids = Vec::new();
        for (agent_id, result) in [("round-one", "one"), ("round-two", "two")] {
            let expected_state = db.get_conversation(parent_id).await.unwrap().state;
            db.persist_startup_sub_agent_fan_in(
                parent_id,
                &[SubAgentResult {
                    agent_id: agent_id.to_string(),
                    task: agent_id.to_string(),
                    outcome: SubAgentOutcome::Success {
                        result: result.to_string(),
                    },
                }],
                None,
                &expected_state,
                &destination,
                StartupParentAction::Resume,
                Utc::now(),
            )
            .await
            .unwrap();
            let actions = db.list_startup_parent_actions().await.unwrap();
            assert_eq!(actions.len(), 1);
            action_ids.push(actions[0].action_id);
        }
        assert_ne!(action_ids[0], action_ids[1]);

        let summaries: Vec<_> = db
            .get_messages(parent_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|message| {
                message
                    .message_id
                    .starts_with(&format!("startup-sub-agent-summary:{parent_id}:"))
            })
            .collect();
        assert_eq!(summaries.len(), 2);
        assert_ne!(summaries[0].message_id, summaries[1].message_id);
    }

    #[tokio::test]
    async fn startup_fan_in_retries_unreadable_child_state_without_overwrite() {
        use phoenix_core::domain::sm_state::{PendingSubAgent, SubAgentMode};

        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "unreadable-parent";
        let child_id = "unreadable-child";
        db.create_conversation(parent_id, parent_id, "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation(child_id, child_id, "/tmp", false, Some(parent_id), None)
            .await
            .unwrap();
        db.update_conversation_state(
            parent_id,
            &ConvState::AwaitingSubAgents {
                pending: vec![PendingSubAgent {
                    agent_id: child_id.to_string(),
                    task: "unreadable".to_string(),
                    mode: SubAgentMode::Explore,
                }],
                completed_results: Vec::new(),
                spawn_tool_id: None,
            },
        )
        .await
        .unwrap();
        db.update_conversation_state(
            child_id,
            &ConvState::Completed {
                result: "exact result".to_string(),
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE conversations SET state = json_remove(state, '$.result') WHERE id = ?1",
        )
        .bind(child_id)
        .execute(db.pool())
        .await
        .unwrap();
        let ids = std::collections::HashSet::from([parent_id.to_string()]);

        let error = db
            .reconcile_startup_obligated_parents(&ids)
            .await
            .expect_err("unreadable terminal evidence must remain retryable");
        assert!(error.to_string().contains("decode pending sub-agent"));
        let raw: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
            .bind(child_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(!raw.contains("result"));
        assert!(db.list_startup_parent_actions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn startup_cancelling_sub_agents_reaches_cause_destination() {
        use phoenix_core::domain::sm_event::CancelCause;
        use phoenix_core::domain::sm_state::{PendingSubAgent, SubAgentMode};

        for (suffix, cause, expects_request) in [
            ("user", CancelCause::UserRequested, false),
            ("timeout", CancelCause::Timeout, true),
        ] {
            let db = Database::open_in_memory().await.unwrap();
            let parent_id = format!("cancelling-parent-{suffix}");
            let child_id = format!("cancelling-child-{suffix}");
            db.create_conversation(&parent_id, &parent_id, "/tmp", true, None, None)
                .await
                .unwrap();
            db.create_conversation(&child_id, &child_id, "/tmp", false, Some(&parent_id), None)
                .await
                .unwrap();
            if expects_request {
                db.update_conversation_state(
                    &child_id,
                    &ConvState::Completed {
                        result: "late success after timeout".to_string(),
                    },
                )
                .await
                .unwrap();
            }
            db.update_conversation_state(
                &parent_id,
                &ConvState::CancellingSubAgents {
                    pending: vec![PendingSubAgent {
                        agent_id: child_id,
                        task: "cancel".to_string(),
                        mode: SubAgentMode::Explore,
                    }],
                    completed_results: Vec::new(),
                    cause,
                    spawn_tool_id: None,
                },
            )
            .await
            .unwrap();
            let ids = std::collections::HashSet::from([parent_id.clone()]);

            let reconciled = db.reconcile_startup_obligated_parents(&ids).await.unwrap();
            assert_eq!(reconciled.len(), 1);
            let actions = db.list_startup_parent_actions().await.unwrap();
            assert_eq!(actions.len(), 1);
            assert_eq!(actions[0].conversation_id, parent_id);
            assert_eq!(
                actions[0].action,
                if expects_request {
                    StartupParentAction::Resume
                } else {
                    StartupParentAction::Cancel
                }
            );
            let state = db.get_conversation(&parent_id).await.unwrap().state;
            assert_eq!(
                matches!(state, ConvState::LlmRequesting { attempt: 1 }),
                expects_request
            );
            if !expects_request {
                assert!(matches!(state, ConvState::Idle));
            }
            let summary = db
                .get_messages(&parent_id)
                .await
                .unwrap()
                .into_iter()
                .find(|message| {
                    message
                        .message_id
                        .starts_with(&format!("startup-sub-agent-summary:{parent_id}:"))
                })
                .unwrap();
            let MessageContent::User(content) = summary.content else {
                unreachable!()
            };
            if expects_request {
                assert!(content.text.to_ascii_lowercase().contains("timed out"));
            } else {
                assert!(content.text.to_ascii_lowercase().contains("interrupted"));
            }
        }
    }

    #[tokio::test]
    async fn reset_all_to_idle_preserves_completed_and_failed_states() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("completed", "completed", "/tmp", false, None, None)
            .await
            .unwrap();
        db.create_conversation("failed", "failed", "/tmp", false, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            "completed",
            &ConvState::Completed {
                result: "done".to_string(),
            },
        )
        .await
        .unwrap();
        db.update_conversation_state(
            "failed",
            &ConvState::Failed {
                error: "boom".to_string(),
                error_kind: phoenix_core::domain::db_schema::ErrorKind::SubAgentError,
            },
        )
        .await
        .unwrap();

        db.reset_all_to_idle().await.unwrap();

        assert!(matches!(
            db.get_conversation("completed").await.unwrap().state,
            ConvState::Completed { .. }
        ));
        assert!(matches!(
            db.get_conversation("failed").await.unwrap().state,
            ConvState::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn reset_preserves_continuation_auth_recovery_but_resets_ordinary_recovery() {
        let db = Database::open_in_memory().await.unwrap();
        for id in ["continuation-auth", "ordinary-auth"] {
            db.create_conversation(id, id, "/tmp", true, None, None)
                .await
                .unwrap();
        }
        let request = phoenix_core::domain::sm_state::ContinuationSummaryRequest {
            operation_id: "auth-operation".to_string(),
            rejected_tool_calls: Vec::new(),
            attempt: 1,
        };
        db.update_conversation_state(
            "continuation-auth",
            &ConvState::AwaitingRecovery {
                message: "authenticate".to_string(),
                error_kind: ErrorKind::Auth,
                recovery_kind: phoenix_core::domain::sm_state::RecoveryKind::Credential,
                resume: phoenix_core::domain::sm_state::RecoveryResumeTarget::ContinuationSummary {
                    request: request.clone(),
                },
            },
        )
        .await
        .unwrap();
        db.update_conversation_state(
            "ordinary-auth",
            &ConvState::AwaitingRecovery {
                message: "authenticate".to_string(),
                error_kind: ErrorKind::Auth,
                recovery_kind: phoenix_core::domain::sm_state::RecoveryKind::Credential,
                resume: phoenix_core::domain::sm_state::RecoveryResumeTarget::ConversationTurn,
            },
        )
        .await
        .unwrap();

        db.reset_all_to_idle().await.unwrap();

        assert!(matches!(
            db.get_conversation("continuation-auth").await.unwrap().state,
            ConvState::AwaitingRecovery {
                resume: phoenix_core::domain::sm_state::RecoveryResumeTarget::ContinuationSummary {
                    request: persisted,
                },
                ..
            } if persisted == request
        ));
        assert_eq!(
            db.get_conversation("ordinary-auth").await.unwrap().state,
            ConvState::Idle
        );
    }

    /// A pending sub-agent that ALREADY reached its terminal state in its child
    /// conversation before the restart must be fanned in with its REAL outcome
    /// (success/failure), not rewritten as "interrupted by server restart". A
    /// sibling still running keeps the interrupted fallback.
    #[tokio::test]
    async fn test_materialize_uses_real_outcome_for_completed_sub_agent() {
        use phoenix_core::domain::db_schema::ToolResult;
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, ConvState, PendingSubAgent, SubAgentMode, ThinkInput, ToolCall,
            ToolInput,
        };

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-p", "slug-p", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message("m-u", "conv-p", &MessageContent::user("go"), None, None)
            .await
            .unwrap();

        let done_agent = "33333333-3333-4333-8333-333333333333";
        let running_agent = "44444444-4444-4444-8444-444444444444";

        // The done agent's child conversation (created with id == agent_id)
        // reached terminal `Completed`.
        db.create_conversation(done_agent, "sub-done", "/tmp", false, Some("conv-p"), None)
            .await
            .unwrap();
        db.update_conversation_state(
            done_agent,
            &ConvState::Completed {
                result: "found the bug in the parser".to_string(),
            },
        )
        .await
        .unwrap();
        // The running agent has a child row that is NOT terminal.
        db.create_conversation(
            running_agent,
            "sub-run",
            "/tmp",
            false,
            Some("conv-p"),
            None,
        )
        .await
        .unwrap();

        let think = |t: &str| ToolInput::Think(ThinkInput { thoughts: t.into() });
        let assistant = AssistantMessage::new(
            "asst-p".to_string(),
            vec![
                ContentBlock::tool_use(
                    "sp",
                    "spawn_agents",
                    serde_json::json!({"tasks": [{"task": "a"}, {"task": "b"}]}),
                ),
                ContentBlock::tool_use("th", "think", serde_json::json!({"thoughts": "t"})),
            ],
            None,
            None,
        );
        let placeholder = format!("Spawning 2 sub-agent(s): {done_agent}, {running_agent}");
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new("th", think("t")),
            remaining_tools: vec![],
            completed_results: vec![ToolResult::success("sp".to_string(), placeholder)],
            pending_sub_agents: vec![
                PendingSubAgent {
                    agent_id: done_agent.to_string(),
                    task: "investigate the parser".to_string(),
                    mode: SubAgentMode::Explore,
                },
                PendingSubAgent {
                    agent_id: running_agent.to_string(),
                    task: "audit the lexer".to_string(),
                    mode: SubAgentMode::Explore,
                },
            ],
            assistant_message: assistant,
        };
        db.update_conversation_state("conv-p", &state)
            .await
            .unwrap();

        db.reset_all_to_idle().await.unwrap();

        let msgs = db.get_messages("conv-p").await.unwrap();
        let spawn = msgs
            .iter()
            .find_map(|m| match &m.content {
                MessageContent::Tool(tc) if tc.tool_use_id == "sp" => Some(tc.clone()),
                MessageContent::Tool(_)
                | MessageContent::User(_)
                | MessageContent::Agent(_)
                | MessageContent::System(_)
                | MessageContent::Error(_)
                | MessageContent::Continuation(_)
                | MessageContent::Skill(_) => None,
            })
            .expect("spawn fan-in present");

        // The done agent's real result is fanned in; it is NOT "interrupted".
        assert!(
            spawn.content.contains("found the bug in the parser"),
            "completed sub-agent must show its real result, got: {}",
            spawn.content
        );
        // The still-running agent keeps the interrupted fallback.
        assert!(
            spawn.content.contains("interrupted by server restart"),
            "still-running sub-agent must show interrupted, got: {}",
            spawn.content
        );
    }

    #[tokio::test]
    async fn test_reset_skips_repair_for_preserved_state_conversations() {
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::ConvState;

        let db = Database::open_in_memory().await.unwrap();

        for (id, state) in [
            (
                "ctx-exhausted",
                ConvState::ContextExhausted {
                    summary: "summary".into(),
                },
            ),
            ("terminal", ConvState::Terminal),
        ] {
            db.create_conversation(id, &format!("slug-{id}"), "/tmp", true, None, None)
                .await
                .unwrap();

            // Agent message with an orphaned tool_use (no matching result).
            db.add_message(
                &format!("{id}-msg-1"),
                id,
                &MessageContent::agent(vec![ContentBlock::tool_use(
                    format!("{id}-tool"),
                    "bash",
                    serde_json::json!({"op": "run", "cmd": "ls"}),
                )]),
                None,
                None,
            )
            .await
            .unwrap();

            db.update_conversation_state(id, &state).await.unwrap();
        }

        db.reset_all_to_idle().await.unwrap();

        for id in ["ctx-exhausted", "terminal"] {
            let msgs = db.get_messages(id).await.unwrap();
            assert_eq!(
                msgs.len(),
                1,
                "frozen conversation {id} should not get a synthetic tool_result, \
                 got {} messages",
                msgs.len()
            );
            assert_eq!(msgs[0].message_type, MessageType::Agent);
        }
    }

    #[tokio::test]
    async fn conversation_effort_round_trips_and_sub_agents_inherit_explicit_override() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation(
            "effort-parent",
            "effort-parent",
            "/tmp",
            true,
            None,
            Some("gpt-5.4"),
        )
        .await
        .unwrap();
        db.update_conversation_model_and_effort(
            "effort-parent",
            "gpt-5.4",
            Some(ModelEffort::High),
            ServiceTier::Standard,
        )
        .await
        .unwrap();

        let parent = db.get_conversation("effort-parent").await.unwrap();
        assert_eq!(parent.effort, Some(ModelEffort::High));

        db.create_conversation(
            "effort-child",
            "effort-child",
            "/tmp",
            false,
            Some("effort-parent"),
            Some("gpt-5.4"),
        )
        .await
        .unwrap();
        let child = db.get_conversation("effort-child").await.unwrap();
        assert_eq!(child.effort, Some(ModelEffort::High));
    }

    #[tokio::test]
    async fn unattached_sub_agent_inherits_parent_effort_without_a_work_scope() {
        let db = Database::open_in_memory().await.unwrap();
        let parent = db
            .get_or_create_coordinator(
                Some("gpt-5.4"),
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        db.update_conversation_model_and_effort(
            &parent.id,
            "gpt-5.4",
            Some(ModelEffort::High),
            ServiceTier::Standard,
        )
        .await
        .unwrap();

        let child = db
            .create_subagent_conversation(
                "unattached-effort-child",
                "unattached-effort-child",
                "/tmp",
                &parent.id,
                "gpt-5.4",
                &ConvMode::Explore {
                    worktree_path: None,
                    next_taskmd_id_hint: None,
                },
                phoenix_core::llm_language::LlmLanguage::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(child.attached_work_scope_id, None);
        assert_eq!(child.effort, Some(ModelEffort::High));
        assert_eq!(
            child.product_conversation_id,
            parent.product_conversation_id
        );
    }

    async fn open_file_backed_test_db(name: &str) -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        migrations::run_pending_migrations(db.pool()).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn sub_agent_creation_owns_write_intent_before_parent_snapshot() {
        let (_dir, mut db) = open_file_backed_test_db("write-intent-before-snapshot.sqlite").await;
        let parent = db
            .create_conversation_with_project(
                "write-intent-parent",
                "write-intent-parent",
                "/tmp",
                true,
                None,
                Some("gpt-5.4"),
                None,
                &work_mode_fixture(),
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        let expected_scope = parent.attached_work_scope_id.unwrap();
        let latch = std::sync::Arc::new(SubAgentCreationTestLatch::new());
        db.sub_agent_creation_test_latch = Some(latch.clone());
        let child_db = db.clone();
        let child = tokio::spawn(async move {
            child_db
                .create_subagent_conversation(
                    "write-intent-child",
                    "write-intent-child",
                    "/tmp",
                    "write-intent-parent",
                    "gpt-5.4",
                    &ConvMode::Explore {
                        worktree_path: None,
                        next_taskmd_id_hint: None,
                    },
                    phoenix_core::llm_language::LlmLanguage::default(),
                    Some(&expected_scope),
                )
                .await
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            latch.parent_read.notified(),
        )
        .await
        .expect("child creation must reach its parent snapshot");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db.path))
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::ZERO);
        let mut competing_writer = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let write_error = sqlx::query(
            "UPDATE conversations SET updated_at = updated_at WHERE id = 'write-intent-parent'",
        )
        .execute(&mut competing_writer)
        .await
        .unwrap_err();
        assert_eq!(
            write_error.as_database_error().unwrap().code().as_deref(),
            Some("5")
        );
        latch.competing_write_observed.notify_one();
        let created = child.await.unwrap().unwrap();

        assert_eq!(created.id, "write-intent-child");
    }

    #[tokio::test]
    async fn parallel_sub_agent_creation_serializes_scope_snapshot_and_insert() {
        let (_dir, db) = open_file_backed_test_db("parallel-sub-agents.sqlite").await;
        let parent = db
            .create_conversation_with_project(
                "parallel-parent",
                "parallel-parent",
                "/tmp",
                true,
                None,
                Some("gpt-5.4"),
                None,
                &work_mode_fixture(),
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        db.update_conversation_model_and_effort(
            &parent.id,
            "gpt-5.4",
            Some(ModelEffort::High),
            ServiceTier::Standard,
        )
        .await
        .unwrap();
        let expected_scope = parent.attached_work_scope_id.unwrap();
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(10));
        let mut children = tokio::task::JoinSet::new();

        for index in 0..10 {
            let db = db.clone();
            let start = start.clone();
            let expected_scope = expected_scope.clone();
            children.spawn(async move {
                start.wait().await;
                db.create_subagent_conversation(
                    &format!("parallel-child-{index}"),
                    &format!("parallel-child-{index}"),
                    "/tmp",
                    "parallel-parent",
                    "gpt-5.4",
                    &ConvMode::Explore {
                        worktree_path: None,
                        next_taskmd_id_hint: None,
                    },
                    phoenix_core::llm_language::LlmLanguage::default(),
                    Some(&expected_scope),
                )
                .await
            });
        }

        let mut created = Vec::new();
        while let Some(result) = children.join_next().await {
            created.push(result.unwrap().unwrap());
        }
        assert_eq!(created.len(), 10);
        assert!(created.iter().all(|child| {
            child.attached_work_scope_id.as_ref() == Some(&expected_scope)
                && child.effort == Some(ModelEffort::High)
        }));
        let persisted_children: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations WHERE parent_conversation_id = 'parallel-parent'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(persisted_children, 10);
    }

    #[tokio::test]
    async fn sub_agent_creation_rejects_changed_parent_scope_without_partial_child() {
        let (_dir, db) = open_file_backed_test_db("changed-parent-scope.sqlite").await;
        let parent = db
            .create_conversation_with_project(
                "scope-parent",
                "scope-parent",
                "/tmp",
                true,
                None,
                Some("gpt-5.4"),
                None,
                &work_mode_fixture(),
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        let captured_scope = parent.attached_work_scope_id.unwrap();
        let replacement_parent = db
            .create_conversation_with_project(
                "replacement-scope-parent",
                "replacement-scope-parent",
                "/tmp/replacement",
                true,
                None,
                Some("gpt-5.4"),
                None,
                &work_mode_fixture(),
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'scope-parent'")
            .bind(
                replacement_parent
                    .attached_work_scope_id
                    .as_ref()
                    .unwrap()
                    .as_str(),
            )
            .execute(db.pool())
            .await
            .unwrap();

        let error = db
            .create_subagent_conversation(
                "rejected-child",
                "rejected-child",
                "/tmp",
                "scope-parent",
                "gpt-5.4",
                &ConvMode::Explore {
                    worktree_path: None,
                    next_taskmd_id_hint: None,
                },
                phoenix_core::llm_language::LlmLanguage::default(),
                Some(&captured_scope),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, DbError::CloseFoundationConflict(_)));
        let child_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE id = 'rejected-child'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(child_count, 0);
    }

    #[tokio::test]
    async fn test_slug_collision_gets_suffix() {
        let db = Database::open_in_memory().await.unwrap();

        // First conversation gets the exact slug
        let first = db
            .create_conversation("id-1", "my-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        assert_eq!(first.slug, Some("my-slug".to_string()));

        // Second conversation with the same slug gets a suffix
        let second = db
            .create_conversation("id-2", "my-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        let second_slug = second.slug.unwrap();
        assert!(
            second_slug.starts_with("my-slug-"),
            "Expected suffix, got: {second_slug}"
        );
        assert_ne!(second_slug, "my-slug");

        // Both are retrievable by ID
        assert_eq!(
            db.get_conversation("id-1").await.unwrap().slug,
            Some("my-slug".to_string())
        );
        assert_eq!(
            db.get_conversation("id-2").await.unwrap().slug,
            Some(second_slug)
        );
    }

    /// REQ-BED-030 Phase 1 (task 24696): the `continued_in_conv_id` column
    /// round-trips through the sqlx read/write path. Fresh rows read back as
    /// `None`; rows with the column populated (via direct SQL here, since
    /// the public handoff API arrives in Phase 2) read back as `Some`.
    #[tokio::test]
    async fn test_continued_in_conv_id_db_round_trip() {
        let db = Database::open_in_memory().await.unwrap();

        // Fresh conversation: the column is NULL, so the struct field is None.
        let fresh = db
            .create_conversation("conv-parent", "parent-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        assert_eq!(fresh.continued_in_conv_id, None);

        let fetched = db.get_conversation("conv-parent").await.unwrap();
        assert_eq!(fetched.continued_in_conv_id, None);

        // Simulate a continuation: create a second conversation, then point
        // parent -> child via direct SQL. Phase 2 will expose a typed API;
        // Phase 1 just needs the read path to surface the column.
        db.create_conversation("conv-child", "child-slug", "/tmp", true, None, None)
            .await
            .unwrap();
        let parent_product_id = fresh.product_conversation_id;
        recreate_test_conversation_in_product(&db, "conv-child", parent_product_id, "conv-parent")
            .await;

        let parent = db.get_conversation("conv-parent").await.unwrap();
        assert_eq!(parent.continued_in_conv_id, Some("conv-child".to_string()));

        // List paths surface the same field.
        let list = db.list_conversations().await.unwrap();
        let from_list = list.iter().find(|c| c.id == "conv-parent").unwrap();
        assert_eq!(
            from_list.continued_in_conv_id,
            Some("conv-child".to_string())
        );
    }

    // FTUX-08: Conversation names are auto-generated slugs
    //
    // The Conversation struct only has a `slug` field (kebab-case) and no
    // `title` field. The UI displays slugs like "add-hello-file-task" as
    // conversation names. The serialized JSON sent to the API should include
    // a human-readable `title` field (e.g., "Add Hello File Task") in
    // addition to the machine-friendly `slug`.
    #[tokio::test]
    async fn test_ftux08_conversation_json_includes_title_field() {
        let db = Database::open_in_memory().await.unwrap();

        let conv = db
            .create_conversation(
                "conv-ftux08",
                "my-test-conversation",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();

        // Serialize to JSON (same path as conversation_to_json in handlers.rs)
        let json_val = serde_json::to_value(&conv).unwrap();
        let obj = json_val
            .as_object()
            .expect("Conversation should serialize to JSON object");

        // The JSON should have a "title" field with a human-readable name.
        // Currently it only has "slug" (kebab-case), so this test FAILS.
        assert!(
            obj.contains_key("title"),
            "Conversation JSON must include a 'title' field for human-readable display. \
             Found keys: {:?}",
            obj.keys().collect::<Vec<_>>()
        );

        let title = obj["title"].as_str().expect("title should be a string");
        // The title should be human-readable, not kebab-case
        assert!(
            !title.contains('-'),
            "title should be human-readable, not kebab-case. Got: {title}"
        );
    }

    // ============================================================
    // REQ-BED-030 Phase 2 (task 24696): continue_conversation
    // transaction — inheritance table, single-continuation policy,
    // precondition gates.
    //
    // These tests force-set parent state to ContextExhausted via
    // `update_conversation_state` (public API on Database). As of
    // task 24696 Phase 3 the executor no longer auto-cleans
    // worktrees on context exhaustion, so the force-set path
    // matches production behaviour: the parent's worktree fields
    // are preserved for inheritance by the continuation.
    // ============================================================

    /// Helper: create a parent conversation with the given `ConvMode`, force-set its
    /// state to `ContextExhausted`, and return the refreshed record.
    async fn setup_exhausted_parent(
        db: &Database,
        id: &str,
        slug: &str,
        cwd: &str,
        conv_mode: &ConvMode,
    ) -> Conversation {
        db.create_conversation_with_project(
            id,
            slug,
            cwd,
            true,
            None,
            Some("claude-opus-test"),
            None,
            conv_mode,
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();

        let exhausted = ConvState::ContextExhausted {
            summary: "parent's summary of what happened".to_string(),
        };
        db.update_conversation_state(id, &exhausted).await.unwrap();
        db.get_conversation(id).await.unwrap()
    }

    fn work_mode_fixture() -> ConvMode {
        ConvMode::Work {
            branch_name: NonEmptyString::new("task-24696-continue").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt/parent-work").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("TK24696").unwrap(),
            task_title: NonEmptyString::new("Test continuation transfer").unwrap(),
        }
    }

    fn branch_mode_fixture() -> ConvMode {
        ConvMode::Branch {
            branch_name: NonEmptyString::new("feature-login").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/wt/parent-branch").unwrap(),
            base_branch: NonEmptyString::new("feature-login").unwrap(),
        }
    }

    #[tokio::test]
    async fn continuation_creation_persists_dispatch_intent_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        setup_exhausted_parent(
            &db,
            "parent-intent",
            "parent-intent",
            "/tmp",
            &ConvMode::Direct,
        )
        .await;

        let requested = NewContinuationDispatchIntent {
            message_id: "opening-message".to_string(),
            handoff: "Exact edited handoff".to_string(),
            user_agent: Some("test-agent".to_string()),
        };
        let (outcome, intent) = db
            .continue_conversation_with_intent("parent-intent", requested)
            .await
            .unwrap();
        let successor_id = match outcome {
            ContinueOutcome::Created(conversation) => conversation.id,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };
        let intent = intent.expect("created successor must have an intent");
        assert_eq!(intent.successor_conversation_id, successor_id);
        assert_eq!(intent.message_id, "opening-message");
        assert_eq!(intent.handoff, "Exact edited handoff");

        let content = MessageContent::User(UserContent::new("Exact edited handoff"));
        db.add_message("opening-message", &successor_id, &content, None, None)
            .await
            .unwrap();
        assert!(db
            .continuation_dispatch_intent("parent-intent")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn continuation_retry_returns_original_pending_intent() {
        let db = Database::open_in_memory().await.unwrap();
        setup_exhausted_parent(
            &db,
            "parent-retry-intent",
            "parent-retry-intent",
            "/tmp",
            &ConvMode::Direct,
        )
        .await;
        db.continue_conversation_with_intent(
            "parent-retry-intent",
            NewContinuationDispatchIntent {
                message_id: "original-message".to_string(),
                handoff: "Original handoff".to_string(),
                user_agent: None,
            },
        )
        .await
        .unwrap();

        let (outcome, intent) = db
            .continue_conversation_with_intent(
                "parent-retry-intent",
                NewContinuationDispatchIntent {
                    message_id: "different-message".to_string(),
                    handoff: "Must not replace original".to_string(),
                    user_agent: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ContinueOutcome::AlreadyContinued(_)));
        let intent = intent.unwrap();
        assert_eq!(intent.message_id, "original-message");
        assert_eq!(intent.handoff, "Original handoff");
    }

    /// Work -> Work: worktree fields and `task_id` all transfer; parent's
    /// `continued_in_conv_id` points at the new conv.
    #[tokio::test]
    async fn test_continue_conversation_work_to_work() {
        let db = Database::open_in_memory().await.unwrap();
        let parent_mode = work_mode_fixture();
        let parent =
            setup_exhausted_parent(&db, "parent-work", "parent-work", "/tmp", &parent_mode).await;
        db.update_conversation_model_and_effort(
            "parent-work",
            "claude-opus-test",
            Some(ModelEffort::High),
            ServiceTier::Standard,
        )
        .await
        .unwrap();

        let outcome = db.continue_conversation("parent-work").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };
        let persisted_new_conv = db.get_conversation(&new_conv.id).await.unwrap();
        assert_eq!(persisted_new_conv.effort, Some(ModelEffort::High));
        assert_eq!(
            new_conv.product_conversation_id,
            parent.product_conversation_id
        );
        assert_ne!(new_conv.id, new_conv.product_conversation_id.as_str());

        // Inheritance: every ConvMode::Work field copied verbatim.
        match (&parent.conv_mode, &new_conv.conv_mode) {
            (
                ConvMode::Work {
                    branch_name: pb,
                    worktree_path: pw,
                    base_branch: pbb,
                    task_id: pt,
                    task_title: ptt,
                },
                ConvMode::Work {
                    branch_name: nb,
                    worktree_path: nw,
                    base_branch: nbb,
                    task_id: nt,
                    task_title: ntt,
                },
            ) => {
                assert_eq!(pb, nb, "branch_name must be inherited");
                assert_eq!(pw, nw, "worktree_path must be inherited");
                assert_eq!(pbb, nbb, "base_branch must be inherited");
                assert_eq!(pt, nt, "task_id must be inherited (REQ-BED-030 Work-only)");
                assert_eq!(ptt, ntt, "task_title must be inherited");
            }
            _ => panic!("both parent and new conv must be Work mode"),
        }
        assert_eq!(new_conv.cwd, parent.cwd);
        assert_eq!(new_conv.model, parent.model);
        assert!(matches!(new_conv.state, ConvState::Idle));
        assert_eq!(new_conv.continued_in_conv_id, None);
        assert_eq!(new_conv.parent_conversation_id, None);

        // Parent's continued_in_conv_id now points at the continuation.
        let refreshed_parent = db.get_conversation("parent-work").await.unwrap();
        assert_eq!(refreshed_parent.continued_in_conv_id, Some(new_conv.id));
    }

    /// Branch -> Branch: `branch_name/worktree_path/base_branch` transfer; no `task_id`.
    #[tokio::test]
    async fn test_continue_conversation_branch_to_branch() {
        let db = Database::open_in_memory().await.unwrap();
        let parent_mode = branch_mode_fixture();
        let parent = setup_exhausted_parent(
            &db,
            "parent-branch",
            "parent-branch",
            "/tmp/branch-cwd",
            &parent_mode,
        )
        .await;

        let outcome = db.continue_conversation("parent-branch").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };

        match (&parent.conv_mode, &new_conv.conv_mode) {
            (
                ConvMode::Branch {
                    branch_name: pb,
                    worktree_path: pw,
                    base_branch: pbb,
                },
                ConvMode::Branch {
                    branch_name: nb,
                    worktree_path: nw,
                    base_branch: nbb,
                },
            ) => {
                assert_eq!(pb, nb);
                assert_eq!(pw, nw);
                assert_eq!(pbb, nbb);
            }
            _ => panic!("both must be Branch mode"),
        }
        assert_eq!(new_conv.cwd, parent.cwd);
        // task_id is Work-only — there's no field on Branch ConvMode, so this
        // is enforced structurally rather than via an assertion.

        let refreshed_parent = db.get_conversation("parent-branch").await.unwrap();
        assert_eq!(refreshed_parent.continued_in_conv_id, Some(new_conv.id));
    }

    /// Explore -> Explore: mode is cloned (Explore has no worktree fields on
    /// the `ConvMode` variant — REQ-PROJ-028's on-first-message worktree isn't
    /// encoded in `ConvMode::Explore`, so this is just cwd + mode inheritance).
    #[tokio::test]
    async fn test_continue_conversation_explore_to_explore() {
        let db = Database::open_in_memory().await.unwrap();
        let parent = setup_exhausted_parent(
            &db,
            "parent-explore",
            "parent-explore",
            "/tmp/explore-cwd",
            &ConvMode::Explore {
                worktree_path: None,
                next_taskmd_id_hint: None,
            },
        )
        .await;

        let outcome = db.continue_conversation("parent-explore").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };

        assert!(matches!(new_conv.conv_mode, ConvMode::Explore { .. }));
        assert_eq!(new_conv.cwd, parent.cwd);
        assert_eq!(new_conv.model, parent.model);
        let refreshed_parent = db.get_conversation("parent-explore").await.unwrap();
        assert_eq!(refreshed_parent.continued_in_conv_id, Some(new_conv.id));
    }

    /// Direct -> Direct: no worktree, only cwd and model inheritance.
    #[tokio::test]
    async fn test_continue_conversation_direct_to_direct() {
        let db = Database::open_in_memory().await.unwrap();
        let parent = setup_exhausted_parent(
            &db,
            "parent-direct",
            "parent-direct",
            "/tmp/direct-cwd",
            &ConvMode::Direct,
        )
        .await;

        let outcome = db.continue_conversation("parent-direct").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };

        assert!(matches!(new_conv.conv_mode, ConvMode::Direct));
        assert_eq!(new_conv.cwd, parent.cwd);
        assert_eq!(new_conv.model, parent.model);
        let refreshed_parent = db.get_conversation("parent-direct").await.unwrap();
        assert_eq!(refreshed_parent.continued_in_conv_id, Some(new_conv.id));
    }

    /// Double-continue: the second call returns the same continuation id as
    /// the first (idempotent return) and does NOT create a second new conv.
    /// The parent's `continued_in_conv_id` is unchanged by the second call.
    #[tokio::test]
    async fn test_continue_conversation_idempotent_double_continue() {
        let db = Database::open_in_memory().await.unwrap();
        setup_exhausted_parent(
            &db,
            "parent-double",
            "parent-double",
            "/tmp",
            &work_mode_fixture(),
        )
        .await;

        let first = match db.continue_conversation("parent-double").await.unwrap() {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("first call should create, got {other:?}")
            }
        };

        let second = match db.continue_conversation("parent-double").await.unwrap() {
            ContinueOutcome::AlreadyContinued(c) => c,
            other @ (ContinueOutcome::Created(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("second call should return AlreadyContinued, got {other:?}")
            }
        };

        assert_eq!(
            first.id, second.id,
            "idempotent return must yield the same continuation id"
        );

        // Parent pointer unchanged.
        let refreshed_parent = db.get_conversation("parent-double").await.unwrap();
        assert_eq!(refreshed_parent.continued_in_conv_id, Some(first.id));

        // No phantom third conversation exists.
        let all = db.list_conversations().await.unwrap();
        assert_eq!(
            all.len(),
            2,
            "only parent + single continuation should be listed; got: {:?}",
            all.iter().map(|c| &c.id).collect::<Vec<_>>(),
        );
    }

    /// Parent not in `ContextExhausted` state: transaction does not run;
    /// parent state is unchanged.
    #[tokio::test]
    async fn test_continue_conversation_rejects_idle_parent() {
        let db = Database::open_in_memory().await.unwrap();
        // Create a Work-mode parent but leave it in Idle.
        db.create_conversation_with_project(
            "parent-idle",
            "parent-idle",
            "/tmp",
            true,
            None,
            Some("claude-opus-test"),
            None,
            &work_mode_fixture(),
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();

        let outcome = db.continue_conversation("parent-idle").await.unwrap();
        match outcome {
            ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                assert_eq!(state_variant, "Idle");
            }
            other @ (ContinueOutcome::Created(_) | ContinueOutcome::AlreadyContinued(_)) => {
                panic!("expected ParentNotContextExhausted, got {other:?}")
            }
        }

        // Parent unchanged.
        let refreshed = db.get_conversation("parent-idle").await.unwrap();
        assert!(matches!(refreshed.state, ConvState::Idle));
        assert_eq!(refreshed.continued_in_conv_id, None);

        // No new conversation created.
        let all = db.list_conversations().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    /// Parent id does not exist: returns `DbError::ConversationNotFound` so the
    /// HTTP handler can map to 404.
    #[tokio::test]
    async fn test_continue_conversation_parent_not_found() {
        let db = Database::open_in_memory().await.unwrap();
        let result = db.continue_conversation("no-such-conv").await;
        match result {
            Err(DbError::ConversationNotFound(id)) => assert_eq!(id, "no-such-conv"),
            other => panic!("expected ConversationNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn direct_continuation_gets_fresh_work_scope() {
        let db = Database::open_in_memory().await.unwrap();
        let parent =
            setup_exhausted_parent(&db, "direct-root", "direct", "/tmp", &ConvMode::Direct).await;

        let child = match db.continue_conversation(&parent.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };

        assert_ne!(child.attached_work_scope_id, parent.attached_work_scope_id);
        assert_eq!(child.cwd, parent.cwd);
    }

    /// Sequential slugs: first continuation is `{root}-2`, multi-level chains
    /// use the root slug (not the parent slug) so names don't compound.
    #[tokio::test]
    async fn test_continue_conversation_sequential_slugs() {
        let db = Database::open_in_memory().await.unwrap();

        // Root conversation: slug = "my-task"
        setup_exhausted_parent(&db, "root", "my-task", "/tmp", &ConvMode::Direct).await;

        // First continuation: should be "my-task-2"
        let first = match db.continue_conversation("root").await.unwrap() {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };
        assert_eq!(
            first.slug.as_deref(),
            Some("my-task-2"),
            "first continuation slug must be {{root_slug}}-2"
        );

        // Exhaust and continue from first continuation.
        let exhausted = ConvState::ContextExhausted {
            summary: "summary".to_string(),
        };
        db.update_conversation_state(&first.id, &exhausted)
            .await
            .unwrap();

        // Second continuation: should be "my-task-3" (root slug, not parent slug)
        let second = match db.continue_conversation(&first.id).await.unwrap() {
            ContinueOutcome::Created(c) => c,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };
        assert_eq!(
            second.slug.as_deref(),
            Some("my-task-3"),
            "second continuation slug must be {{root_slug}}-3, not the parent slug appended"
        );
    }

    #[tokio::test]
    async fn coordinator_relation_moves_to_continuation() {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(
                Some("test-model"),
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        db.update_conversation_state(
            &coordinator.id,
            &ConvState::ContextExhausted {
                summary: "summary".to_string(),
            },
        )
        .await
        .unwrap();

        let continuation = match db.continue_conversation(&coordinator.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected created continuation, got {other:?}")
            }
        };
        assert_eq!(
            db.coordinator_conversation_id().await.unwrap().as_deref(),
            Some(continuation.id.as_str())
        );
        assert!(db
            .is_coordinator_conversation(&coordinator.id)
            .await
            .unwrap());
        assert!(db
            .is_coordinator_conversation(&continuation.id)
            .await
            .unwrap());
        assert!(!continuation.user_initiated);
        assert!(!db
            .list_conversations()
            .await
            .unwrap()
            .iter()
            .any(|conversation| conversation.id == continuation.id));
        db.update_conversation_state(
            &continuation.id,
            &ConvState::ContextExhausted {
                summary: "second summary".to_string(),
            },
        )
        .await
        .unwrap();
        let second = match db.continue_conversation(&continuation.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected second continuation, got {other:?}")
            }
        };
        assert!(!second.user_initiated);
        let listed_ids: Vec<_> = db
            .list_conversations()
            .await
            .unwrap()
            .into_iter()
            .map(|conversation| conversation.id)
            .collect();
        assert!(!listed_ids.contains(&coordinator.id));
        assert!(!listed_ids.contains(&continuation.id));
        assert!(!listed_ids.contains(&second.id));
        let preview_roots = db.preview_roots().await.unwrap();
        assert!(!preview_roots.contains(&coordinator.cwd));
        assert!(!preview_roots.contains(&continuation.cwd));
        assert!(!preview_roots.contains(&second.cwd));
    }

    // ------------------------------------------------------------------
    // Phoenix Chains v1 (task 02686): chain_name + chain walk methods
    // ------------------------------------------------------------------

    async fn recreate_test_conversation_in_product(
        db: &Database,
        id: &str,
        product_conversation_id: phoenix_core::domain::product_conversation::ProductConversationId,
        predecessor_id: &str,
    ) {
        let mut conversation = db.get_conversation(id).await.unwrap();
        let old_product_conversation_id = conversation.product_conversation_id.clone();
        sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(id)
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM product_conversations WHERE id = ?1")
            .bind(old_product_conversation_id.as_str())
            .execute(&db.pool)
            .await
            .unwrap();
        conversation.product_conversation_id = product_conversation_id;
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO product_continuation_reservations (
                 predecessor_conversation_id, successor_conversation_id,
                 product_conversation_id
             ) VALUES (?1, ?2, ?3)",
        )
        .bind(predecessor_id)
        .bind(id)
        .bind(conversation.product_conversation_id.as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(id)
            .bind(predecessor_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        insert_conversation_tx(&mut tx, &conversation)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM product_continuation_reservations
             WHERE predecessor_conversation_id = ?1",
        )
        .bind(predecessor_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// Build a 3-member linear continuation chain `a -> b -> c` and return
    /// the ids in chain order. Uses raw SQL to bypass `continue_conversation`'s
    /// gating on `ContextExhausted` parents — the walk methods are invariant
    /// to how the edges were written.
    async fn build_linear_chain(db: &Database, ids: &[&str]) {
        for id in ids {
            db.create_conversation(id, &format!("slug-{id}"), "/tmp", true, None, None)
                .await
                .unwrap();
        }
        let root_product_conversation_id = db
            .get_conversation(ids[0])
            .await
            .unwrap()
            .product_conversation_id;
        for (index, id) in ids[1..].iter().enumerate() {
            let predecessor_id = ids[index];
            let conversation = db.get_conversation(id).await.unwrap();
            sqlx::query("DELETE FROM conversations WHERE id = ?1")
                .bind(id)
                .execute(&db.pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM product_conversations WHERE id = ?1")
                .bind(conversation.product_conversation_id.as_str())
                .execute(&db.pool)
                .await
                .unwrap();
            let mut tx = db.pool.begin().await.unwrap();
            sqlx::query("PRAGMA defer_foreign_keys = ON")
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO product_continuation_reservations (
                     predecessor_conversation_id, successor_conversation_id,
                     product_conversation_id
                 ) VALUES (?1, ?2, ?3)",
            )
            .bind(predecessor_id)
            .bind(id)
            .bind(root_product_conversation_id.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(id)
                .bind(predecessor_id)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO conversations (
                     id, product_conversation_id, slug, user_initiated, runtime_role,
                     state_updated_at, created_at, updated_at, work_scope_id
                 ) VALUES (?1, ?2, ?3, 1, 'user', ?4, ?4, ?4, ?5)",
            )
            .bind(id)
            .bind(root_product_conversation_id.as_str())
            .bind(format!("slug-{id}"))
            .bind(Utc::now().to_rfc3339())
            .bind(
                conversation
                    .attached_work_scope_id
                    .as_ref()
                    .map(WorkScopeId::as_str),
            )
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query(
                "DELETE FROM product_continuation_reservations
                 WHERE predecessor_conversation_id = ?1",
            )
            .bind(predecessor_id)
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
    }

    #[tokio::test]
    async fn task_handoff_creation_job_owns_detached_scope_and_immutable_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("handoff-parent", "handoff-parent", "/tmp", true, None, None)
            .await
            .unwrap();
        let parent = db.get_conversation("handoff-parent").await.unwrap();
        let approval = phoenix_core::task_handoff::TaskApprovalHandoffData {
            task_id: "27002".to_string(),
            task_title: "Approve Fresh".to_string(),
            branch_name: "task-27002-approve-fresh".to_string(),
            approved_commit_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            worktree_path: "/source-worktree-must-not-transfer".to_string(),
            base_branch: "main".to_string(),
            title: "Approve Fresh".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "immutable approved plan".to_string(),
            task_file: "tasks/27002-p1-ready--approve-fresh.md".to_string(),
        };
        let successor = db
            .create_task_approval_handoff_creation_job("handoff-parent", &approval)
            .await
            .unwrap();
        assert!(matches!(successor.state, ConvState::Provisioning { .. }));
        assert_eq!(successor.conv_mode, ConvMode::Direct);
        assert_eq!(successor.attached_work_scope_id, None);
        assert!(parent.attached_work_scope_id.is_some());
        let job = db
            .get_conversation_creation_job_for_conversation(&successor.id)
            .await
            .unwrap()
            .unwrap();
        let snapshot = job.intent.approved_task.expect("durable approved snapshot");
        assert_eq!(snapshot.task_id, approval.task_id);
        assert_eq!(snapshot.plan, approval.plan);
        assert_eq!(snapshot.branch_name, approval.branch_name);
        assert!(!job.intent.text.contains(&approval.worktree_path));
        let replayed = db
            .create_task_approval_handoff_creation_job("handoff-parent", &approval)
            .await
            .unwrap();
        assert_eq!(replayed.id, successor.id);
        let mut divergent_retry = approval.clone();
        divergent_retry.plan = "unreviewed replacement plan".to_string();
        assert!(matches!(
            db.create_task_approval_handoff_creation_job("handoff-parent", &divergent_retry)
                .await,
            Err(DbError::ContinuationPrecondition(message))
                if message.contains("conflicts with committed reviewed snapshot")
        ));
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversation_creation_jobs WHERE conversation_id = ?1",
        )
        .bind(&successor.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        assert!(matches!(
            db.get_conversation("handoff-parent").await.unwrap().state,
            ConvState::Idle
        ));
    }

    #[tokio::test]
    async fn task_handoff_creation_failure_rolls_back_source_settlement_and_target() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("handoff-parent", "handoff-parent", "/tmp", true, None, None)
            .await
            .unwrap();
        let awaiting = ConvState::AwaitingTaskApproval {
            task_file: "tasks/27004-p1-ready--rollback.md".to_string(),
            title: "Rollback".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "reviewed".to_string(),
        };
        sqlx::query(
            "UPDATE conversations SET state = ?1, state_kind = ?2 WHERE id = 'handoff-parent'",
        )
        .bind(serde_json::to_string(&awaiting).unwrap())
        .bind(conv_state_kind(&awaiting))
        .execute(&db.pool)
        .await
        .unwrap();
        let approval = phoenix_core::task_handoff::TaskApprovalHandoffData {
            task_id: "27004".to_string(),
            task_title: "Rollback".to_string(),
            branch_name: "task-27004-rollback".to_string(),
            approved_commit_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            worktree_path: "/ignored".to_string(),
            base_branch: "main".to_string(),
            title: "Rollback".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "reviewed".to_string(),
            task_file: "tasks/27004-p1-ready--rollback.md".to_string(),
        };
        sqlx::query("DROP TABLE conversation_creation_jobs")
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(db
            .create_task_approval_handoff_creation_job("handoff-parent", &approval)
            .await
            .is_err());
        assert!(matches!(
            db.get_conversation("handoff-parent").await.unwrap().state,
            ConvState::AwaitingTaskApproval { .. }
        ));
        let successors: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE id <> 'handoff-parent'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(successors, 0);
    }

    #[tokio::test]
    async fn concurrent_task_handoff_creation_jobs_converge_on_one_snapshot() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("handoff-parent", "handoff-parent", "/tmp", true, None, None)
            .await
            .unwrap();
        let approval = phoenix_core::task_handoff::TaskApprovalHandoffData {
            task_id: "27003".to_string(),
            task_title: "Concurrent".to_string(),
            branch_name: "task-27003-concurrent".to_string(),
            approved_commit_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
            worktree_path: "/ignored".to_string(),
            base_branch: "main".to_string(),
            title: "Concurrent".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "reviewed immutable plan".to_string(),
            task_file: "tasks/27003-p1-ready--concurrent.md".to_string(),
        };
        let (first, second) = tokio::join!(
            db.create_task_approval_handoff_creation_job("handoff-parent", &approval),
            db.create_task_approval_handoff_creation_job("handoff-parent", &approval),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.id, second.id);
        let job = db
            .get_conversation_creation_job_for_conversation(&first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.intent.approved_task.unwrap().plan, approval.plan);
    }

    /// REQ-CHN-007: a `chain_name` set on the root round-trips through
    /// INSERT (raw UPDATE) and SELECT, and the unset case stays NULL.
    #[tokio::test]
    async fn test_chain_name_round_trips() {
        let db = Database::open_in_memory().await.unwrap();

        let unset = db
            .create_conversation("conv-unset", "slug-unset", "/tmp", true, None, None)
            .await
            .unwrap();
        assert_eq!(unset.chain_name, None);

        let fetched_unset = db.get_conversation("conv-unset").await.unwrap();
        assert_eq!(fetched_unset.chain_name, None);

        db.create_conversation("conv-named", "slug-named", "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET chain_name = ?1 WHERE id = ?2")
            .bind("auth refactor")
            .bind("conv-named")
            .execute(&db.pool)
            .await
            .unwrap();

        let fetched_named = db.get_conversation("conv-named").await.unwrap();
        assert_eq!(fetched_named.chain_name, Some("auth refactor".to_string()));

        // List queries also project the column.
        let listed = db.list_conversations().await.unwrap();
        let named = listed.iter().find(|c| c.id == "conv-named").unwrap();
        assert_eq!(named.chain_name, Some("auth refactor".to_string()));
    }

    /// REQ-CHN-002: `chain_members_forward` returns members in chain order
    /// for a 3-member linear chain.
    #[tokio::test]
    async fn test_chain_members_forward_three_member_linear() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["a", "b", "c"]).await;

        let members = db.chain_members_forward("a").await.unwrap();
        assert_eq!(
            members,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    /// `chain_members_forward_full` returns the same membership and order as the
    /// id-only walk, fully hydrated, with an accurate `message_count` per member.
    /// This is the single-query equivalent of looping `get_conversation` over
    /// `chain_members_forward`, so the two must not diverge.
    #[tokio::test]
    async fn test_chain_members_forward_full_matches_per_member_fetch() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["a", "b", "c"]).await;

        // Give members distinct message counts so the COUNT subquery is exercised.
        db.add_message_with_seq("a-1", "a", 1, &MessageContent::user("a1"), None, None)
            .await
            .unwrap();
        db.add_message_with_seq("c-1", "c", 1, &MessageContent::user("c1"), None, None)
            .await
            .unwrap();
        db.add_message_with_seq("c-2", "c", 2, &MessageContent::user("c2"), None, None)
            .await
            .unwrap();

        let full = db.chain_members_forward_full("a").await.unwrap();

        // Same ids, same root-first order as the id-only walk.
        let ids: Vec<&str> = full.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);

        // Per-member parity with the loop it replaces.
        for member in &full {
            let direct = db.get_conversation(&member.id).await.unwrap();
            assert_eq!(member.id, direct.id);
            assert_eq!(member.message_count, direct.message_count);
        }

        let counts: Vec<i64> = full.iter().map(|c| c.message_count).collect();
        assert_eq!(counts, vec![1, 0, 2]);
    }

    /// A non-existent root yields an empty vec, mirroring `chain_members_forward`.
    #[tokio::test]
    async fn test_chain_members_forward_full_nonexistent_root() {
        let db = Database::open_in_memory().await.unwrap();

        let members = db.chain_members_forward_full("ghost").await.unwrap();
        assert!(members.is_empty());
    }

    /// REQ-CHN-002: a single conversation with no continuation returns just itself.
    #[tokio::test]
    async fn test_chain_members_forward_single_member() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("solo", "slug-solo", "/tmp", true, None, None)
            .await
            .unwrap();

        let members = db.chain_members_forward("solo").await.unwrap();
        assert_eq!(members, vec!["solo".to_string()]);
    }

    /// REQ-CHN-002: a non-existent root yields an empty vec, not an error —
    /// callers (Phase 2 Q&A) use this to short-circuit when the chain root
    /// has been hard-deleted.
    #[tokio::test]
    async fn test_chain_members_forward_nonexistent_root() {
        let db = Database::open_in_memory().await.unwrap();

        let members = db.chain_members_forward("ghost").await.unwrap();
        assert!(
            members.is_empty(),
            "nonexistent root should yield empty vec, got: {members:?}"
        );
    }

    /// REQ-CHN-002: `chain_root_of` walks back from the leaf to the root.
    #[tokio::test]
    async fn test_chain_root_of_leaf_returns_root() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["root-x", "mid-x", "leaf-x"]).await;

        let root = db.chain_root_of("leaf-x").await.unwrap();
        assert_eq!(root, Some("root-x".to_string()));

        // Mid-chain walks back to the same root.
        let from_mid = db.chain_root_of("mid-x").await.unwrap();
        assert_eq!(from_mid, Some("root-x".to_string()));
    }

    /// REQ-CHN-002: `chain_root_of` on a root returns the same id.
    #[tokio::test]
    async fn test_chain_root_of_root_returns_self() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("only-root", "slug-only-root", "/tmp", true, None, None)
            .await
            .unwrap();

        let root = db.chain_root_of("only-root").await.unwrap();
        assert_eq!(root, Some("only-root".to_string()));
    }

    /// REQ-CHN-002: `chain_root_of` on a nonexistent id yields None.
    #[tokio::test]
    async fn test_chain_root_of_nonexistent_returns_none() {
        let db = Database::open_in_memory().await.unwrap();

        let root = db.chain_root_of("ghost").await.unwrap();
        assert_eq!(root, None);
    }

    /// `chain_root_if_member` returns Some(root) for any chain member
    /// (root, mid, leaf) and None for solo conversations.
    #[tokio::test]
    async fn test_chain_root_if_member() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["m-a", "m-b", "m-c"]).await;
        db.create_conversation("solo-x", "slug-solo-x", "/tmp", true, None, None)
            .await
            .unwrap();

        for id in ["m-a", "m-b", "m-c"] {
            assert_eq!(
                db.chain_root_if_member(id).await.unwrap(),
                Some("m-a".to_string()),
                "{id} should report root m-a",
            );
        }
        assert_eq!(db.chain_root_if_member("solo-x").await.unwrap(), None);
        assert_eq!(db.chain_root_if_member("nonexistent").await.unwrap(), None);
    }

    // ------------------------------------------------------------------
    // Phoenix Chains v1 (task 02687): chain_qa CRUD + startup sweep
    // ------------------------------------------------------------------

    use chrono::TimeZone;

    fn fresh_new_chain_qa(id: &str, root: &str) -> NewChainQa {
        NewChainQa {
            id: id.to_string(),
            root_conv_id: root.to_string(),
            question: "what happened in this chain?".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            chain_members_at_answer: 3,
            chain_messages_at_answer: 17,
            created_at: Utc::now(),
        }
    }

    /// REQ-CHN-005: `insert_chain_qa` round-trips all columns and starts `in_flight`.
    #[tokio::test]
    async fn test_insert_chain_qa_round_trips() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["qa-a", "qa-b"]).await;
        db.insert_chain_qa(fresh_new_chain_qa("qa-1", "qa-a"))
            .await
            .unwrap();

        let history = db.list_chain_qa("qa-a").await.unwrap();
        assert_eq!(history.len(), 1);
        let row = &history[0];
        assert_eq!(row.id, "qa-1");
        assert_eq!(row.root_conv_id, "qa-a");
        assert_eq!(row.question, "what happened in this chain?");
        assert_eq!(row.answer, None);
        assert_eq!(row.model, "claude-sonnet-4-6");
        assert_eq!(row.status, ChainQaStatus::InFlight);
        assert_eq!(row.chain_members_at_answer, 3);
        assert_eq!(row.chain_messages_at_answer, 17);
        assert!(row.completed_at.is_none());
    }

    /// REQ-CHN-005: `complete_chain_qa` sets answer + `completed_at` + status,
    /// and rewrites the snapshot counters to the completion-time chain shape.
    #[tokio::test]
    async fn test_complete_chain_qa_transitions_row() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["qac-a", "qac-b"]).await;
        db.insert_chain_qa(fresh_new_chain_qa("qac-1", "qac-a"))
            .await
            .unwrap();

        // Complete with a snapshot larger than the inserted one (a continuation
        // landed mid-run): the completion-time counters must overwrite.
        let now = Utc::now();
        db.complete_chain_qa("qac-1", "the final answer", 4, 21, now)
            .await
            .unwrap();

        let row = &db.list_chain_qa("qac-a").await.unwrap()[0];
        assert_eq!(row.status, ChainQaStatus::Completed);
        assert_eq!(row.answer.as_deref(), Some("the final answer"));
        assert!(row.completed_at.is_some());
        assert_eq!(row.chain_members_at_answer, 4);
        assert_eq!(row.chain_messages_at_answer, 21);
    }

    /// REQ-CHN-005: `fail_chain_qa` preserves the question and an optional partial.
    #[tokio::test]
    async fn test_fail_chain_qa_preserves_question_and_partial() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["qaf-a", "qaf-b"]).await;
        db.insert_chain_qa(fresh_new_chain_qa("qaf-1", "qaf-a"))
            .await
            .unwrap();

        db.fail_chain_qa("qaf-1", Some("partial token stream"))
            .await
            .unwrap();
        let row = &db.list_chain_qa("qaf-a").await.unwrap()[0];
        assert_eq!(row.status, ChainQaStatus::Failed);
        assert_eq!(row.answer.as_deref(), Some("partial token stream"));
        assert!(row.completed_at.is_none());

        // None partial works too — no partial answer to preserve.
        db.insert_chain_qa(fresh_new_chain_qa("qaf-2", "qaf-a"))
            .await
            .unwrap();
        db.fail_chain_qa("qaf-2", None).await.unwrap();
        let history = db.list_chain_qa("qaf-a").await.unwrap();
        let row2 = history.iter().find(|r| r.id == "qaf-2").unwrap();
        assert_eq!(row2.status, ChainQaStatus::Failed);
        assert_eq!(row2.answer, None);
    }

    /// REQ-CHN-005: `list_chain_qa` returns rows in chronological order.
    #[tokio::test]
    async fn test_list_chain_qa_orders_chronologically() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["qal-a", "qal-b"]).await;

        let mut row1 = fresh_new_chain_qa("qal-1", "qal-a");
        row1.created_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut row2 = fresh_new_chain_qa("qal-2", "qal-a");
        row2.created_at = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let mut row3 = fresh_new_chain_qa("qal-3", "qal-a");
        row3.created_at = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();

        // Insert out-of-order on purpose.
        db.insert_chain_qa(row3).await.unwrap();
        db.insert_chain_qa(row1).await.unwrap();
        db.insert_chain_qa(row2).await.unwrap();

        let ids: Vec<String> = db
            .list_chain_qa("qal-a")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec!["qal-1", "qal-2", "qal-3"]);
    }

    /// REQ-CHN-005: `sweep_in_flight_chain_qa` flips ONLY `in_flight` rows.
    #[tokio::test]
    async fn test_sweep_in_flight_chain_qa_targets_in_flight_only() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["qas-a", "qas-b"]).await;

        // Three rows: completed, failed, and in_flight.
        db.insert_chain_qa(fresh_new_chain_qa("qas-c", "qas-a"))
            .await
            .unwrap();
        db.complete_chain_qa("qas-c", "done", 2, 8, Utc::now())
            .await
            .unwrap();

        db.insert_chain_qa(fresh_new_chain_qa("qas-f", "qas-a"))
            .await
            .unwrap();
        db.fail_chain_qa("qas-f", None).await.unwrap();

        db.insert_chain_qa(fresh_new_chain_qa("qas-i", "qas-a"))
            .await
            .unwrap();

        let n = db.sweep_in_flight_chain_qa().await.unwrap();
        assert_eq!(n, 1, "only the in_flight row should be touched");

        let rows = db.list_chain_qa("qas-a").await.unwrap();
        let by_id = |id: &str| rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(by_id("qas-c").status, ChainQaStatus::Completed);
        assert_eq!(by_id("qas-f").status, ChainQaStatus::Failed);
        assert_eq!(by_id("qas-i").status, ChainQaStatus::Abandoned);
    }

    /// REQ-CHN-007: `set_chain_name` round-trips (set, change, clear).
    #[tokio::test]
    async fn test_set_chain_name_round_trips() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("scn-root", "slug-scn", "/tmp", true, None, None)
            .await
            .unwrap();

        db.set_chain_name("scn-root", Some("auth refactor"))
            .await
            .unwrap();
        let conv = db.get_conversation("scn-root").await.unwrap();
        assert_eq!(conv.chain_name, Some("auth refactor".to_string()));

        db.set_chain_name("scn-root", Some("auth-refactor-v2"))
            .await
            .unwrap();
        let conv = db.get_conversation("scn-root").await.unwrap();
        assert_eq!(conv.chain_name, Some("auth-refactor-v2".to_string()));

        db.set_chain_name("scn-root", None).await.unwrap();
        let conv = db.get_conversation("scn-root").await.unwrap();
        assert_eq!(conv.chain_name, None);

        // Missing conversation surfaces as a typed error, not silent no-op.
        let err = db.set_chain_name("ghost", Some("x")).await.unwrap_err();
        matches!(err, DbError::ConversationNotFound(_));
    }

    /// REQ-CHN-010: `first_opening_message_text` returns the earliest opening
    /// message — a `user` message OR a `skill` invocation (its trigger text) —
    /// and skips agent/tool/continuation messages.
    #[tokio::test]
    async fn test_first_opening_message_text() {
        use phoenix_core::domain::llm_types::ContentBlock;

        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("fum", "slug-fum", "/tmp", true, None, None)
            .await
            .unwrap();

        // No opening yet → None.
        assert_eq!(db.first_opening_message_text("fum").await.unwrap(), None);

        // Add an agent message first (lower sequence) — must be skipped.
        db.add_message(
            "fum-agent",
            "fum",
            &MessageContent::agent(vec![ContentBlock::text("agent says hi")]),
            None,
            None,
        )
        .await
        .unwrap();
        // Still no opening.
        assert_eq!(db.first_opening_message_text("fum").await.unwrap(), None);

        // First user message.
        db.add_message(
            "fum-u1",
            "fum",
            &MessageContent::user("Refactor the auth module"),
            None,
            None,
        )
        .await
        .unwrap();
        // A later user message must not win.
        db.add_message(
            "fum-u2",
            "fum",
            &MessageContent::user("Now add tests"),
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            db.first_opening_message_text("fum").await.unwrap(),
            Some("Refactor the auth module".to_string())
        );

        // Unknown conversation → None (no rows match), not an error.
        assert_eq!(db.first_opening_message_text("ghost").await.unwrap(), None);

        // A member whose opening is a SKILL invocation: the user's trigger text
        // is the opening intent — not the expanded skill body.
        db.create_conversation("skillconv", "slug-skill", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "skill-open",
            "skillconv",
            &MessageContent::Skill(phoenix_core::domain::db_schema::SkillContent {
                name: "build".to_string(),
                body: "EXPANDED SKILL BODY — must not be the chain name".to_string(),
                trigger: "Ship the release build".to_string(),
                files: vec![],
            }),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            db.first_opening_message_text("skillconv").await.unwrap(),
            Some("Ship the release build".to_string())
        );
    }

    // ==================== rename_conversation Tests ====================

    /// `rename_conversation` updates the slug and advances `updated_at`.
    #[tokio::test]
    async fn test_rename_conversation_success() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("rc", "slug-rc-old", "/tmp", true, None, None)
            .await
            .unwrap();

        let before = db.get_conversation("rc").await.unwrap();
        assert_eq!(before.slug.as_deref(), Some("slug-rc-old"));

        db.rename_conversation("rc", "slug-rc-new").await.unwrap();

        let after = db.get_conversation("rc").await.unwrap();
        assert_eq!(after.slug.as_deref(), Some("slug-rc-new"));
        assert!(
            after.updated_at >= before.updated_at,
            "updated_at must not regress on rename (before={:?}, after={:?})",
            before.updated_at,
            after.updated_at,
        );
    }

    /// Renaming to a slug already used by another conversation returns
    /// `DbError::SlugExists` and leaves the original row untouched.
    #[tokio::test]
    async fn test_rename_conversation_duplicate_slug() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("rc-a", "slug-taken", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("rc-b", "slug-b", "/tmp", true, None, None)
            .await
            .unwrap();

        match db.rename_conversation("rc-b", "slug-taken").await {
            Err(DbError::SlugExists(slug)) => assert_eq!(slug, "slug-taken"),
            other => panic!("expected SlugExists, got {other:?}"),
        }

        // rc-b's slug is unchanged after the rejected rename.
        let b = db.get_conversation("rc-b").await.unwrap();
        assert_eq!(b.slug.as_deref(), Some("slug-b"));
    }

    /// Renaming a conversation whose slug already equals the target is a
    /// no-op success (the existence check excludes the row's own id), so a
    /// caller re-applying the current slug is not spuriously rejected.
    #[tokio::test]
    async fn test_rename_conversation_same_slug_is_ok() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("rc-same", "slug-same", "/tmp", true, None, None)
            .await
            .unwrap();

        db.rename_conversation("rc-same", "slug-same")
            .await
            .unwrap();

        let conv = db.get_conversation("rc-same").await.unwrap();
        assert_eq!(conv.slug.as_deref(), Some("slug-same"));
    }

    /// Renaming a nonexistent conversation returns
    /// `DbError::ConversationNotFound`, not a silent no-op.
    #[tokio::test]
    async fn test_rename_conversation_not_found() {
        let db = Database::open_in_memory().await.unwrap();
        match db.rename_conversation("ghost", "slug-anything").await {
            Err(DbError::ConversationNotFound(id)) => assert_eq!(id, "ghost"),
            other => panic!("expected ConversationNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn test_delete_conversation_removes_wake_owned_workflows_and_messages() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation_with_project(
                "conv-del",
                "conv-del",
                "/tmp",
                true,
                None,
                Some("claude-opus-test"),
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        let work_scope_id = conversation
            .attached_work_scope_id
            .expect("ordinary conversation scope");
        let wake_repo = crate::workflow::wake::WakeRepository::new(db.pool.clone());
        let workflow_id = phoenix_workflow::WorkflowId(9_101);
        let intent = phoenix_workflow::wake_profile::WakeRegistrationIntent {
            contract_id: "contract-del".into(),
            conversation_id: "conv-del".into(),
            root_conversation_id: "conv-del".into(),
            registration_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(
                work_scope_id.as_str().into(),
            ),
            resource: phoenix_workflow::wake_profile::WakeResourceIdentity::Bash(
                phoenix_workflow::wake_profile::BashResourceIdentity {
                    work_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(
                        work_scope_id.as_str().into(),
                    ),
                    handle_id: "b-del".into(),
                },
            ),
            registering_tool_use_id: "tool-del".into(),
            registered_at: phoenix_workflow::Timestamp(10),
            expires_at: phoenix_workflow::Timestamp(100),
        };
        wake_repo
            .register_allocated(
                workflow_id,
                &intent,
                "fp-del",
                phoenix_workflow::Timestamp(10),
            )
            .await
            .unwrap();
        let started = wake_repo
            .claim_observation_if_eligible(
                workflow_id,
                phoenix_workflow::ProcessIncarnation(1),
                phoenix_workflow::Timestamp(20),
                phoenix_workflow::LeaseExpiry(30),
            )
            .await
            .unwrap();
        let authority = match started {
            crate::workflow::wake::WakeObservationOutcome::Started { canonical } => {
                canonical.authority.expect("authority")
            }
            other @ (crate::workflow::wake::WakeObservationOutcome::Busy { .. }
            | crate::workflow::wake::WakeObservationOutcome::Ineligible) => {
                panic!("expected started, got {other:?}")
            }
        };
        let pending = match wake_repo
            .record_terminal_evidence(
                workflow_id,
                &authority,
                1,
                phoenix_workflow::ReceiptId(1),
                phoenix_workflow::DeliveryId(1),
                phoenix_workflow::Timestamp(20),
                &phoenix_workflow::wake_profile::WakeTerminalEvidence::Bash(
                    phoenix_workflow::wake_profile::BashTerminalEvidence {
                        identity: phoenix_workflow::wake_profile::BashResourceIdentity {
                            work_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(
                                work_scope_id.as_str().into(),
                            ),
                            handle_id: "b-del".into(),
                        },
                        status: phoenix_workflow::wake_profile::BashTerminalStatus::Exited,
                        occurred_at: phoenix_workflow::Timestamp(19),
                        exit_code: Some(0),
                        duration_ms: Some(12),
                        signal_number: None,
                        kill_signal_sent: None,
                        final_tail: vec!["done".into()],
                    },
                ),
            )
            .await
            .unwrap()
        {
            crate::workflow::wake::WakeTerminalEvidenceOutcome::Recorded { delivery, .. }
            | crate::workflow::wake::WakeTerminalEvidenceOutcome::Replayed { delivery, .. } => {
                delivery
            }
            other @ (crate::workflow::wake::WakeTerminalEvidenceOutcome::StaleAttempt
            | crate::workflow::wake::WakeTerminalEvidenceOutcome::WrongResource
            | crate::workflow::wake::WakeTerminalEvidenceOutcome::EvidenceAfterObservation
            | crate::workflow::wake::WakeTerminalEvidenceOutcome::EvidenceAfterExpiry) => {
                panic!("expected pending delivery, got {other:?}")
            }
        };
        let _ = wake_repo
            .materialize_pending_delivery_message(
                &crate::workflow::wake::MaterializePendingDeliveryMessageInput {
                    workflow_id,
                    delivery_id: pending.canonical_delivery.delivery_id,
                    conversation_id: pending.conversation_id.clone(),
                    rendered_content: "wake complete".to_string(),
                    display_data: None,
                    auto_resume: true,
                    created_at: phoenix_workflow::Timestamp(50),
                    sequence_id: None,
                },
            )
            .await
            .unwrap();

        db.delete_conversation("conv-del").await.unwrap();

        assert!(db.get_conversation("conv-del").await.is_err());
        let workflow_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE workflow_id = ?1")
                .bind(9_101_i64)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let binding_exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM wake_bindings WHERE workflow_id = ?1")
                .bind(9_101_i64)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let receipt_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wake_terminal_receipts WHERE workflow_id = ?1",
        )
        .bind(9_101_i64)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let link_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wake_delivery_messages WHERE workflow_id = ?1",
        )
        .bind(9_101_i64)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(workflow_exists, 0);
        assert_eq!(binding_exists, 0);
        assert_eq!(receipt_exists, 0);
        assert_eq!(link_exists, 0);
    }

    #[tokio::test]
    async fn delete_conversation_removes_only_an_empty_product_owner() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("delete-root", "delete-root", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(
            &root.id,
            &ConvState::ContextExhausted {
                summary: "continue".to_string(),
            },
        )
        .await
        .unwrap();
        let continuation = match db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected continuation creation, got {other:?}")
            }
        };
        assert_eq!(
            continuation.product_conversation_id,
            root.product_conversation_id
        );

        db.delete_conversation(&root.id).await.unwrap();
        let owner_after_first_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations WHERE id = ?1")
                .bind(root.product_conversation_id.as_str())
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(owner_after_first_delete, 1);

        db.delete_conversation(&continuation.id).await.unwrap();
        let owner_after_last_delete: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations WHERE id = ?1")
                .bind(root.product_conversation_id.as_str())
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(owner_after_last_delete, 0);
        assert!(matches!(
            db.delete_conversation(&continuation.id).await,
            Err(DbError::ConversationNotFound(id)) if id == continuation.id
        ));
    }

    #[tokio::test]
    async fn deleting_last_parent_removes_subordinates_and_product_owner() {
        let db = Database::open_in_memory().await.unwrap();
        let root = db
            .create_conversation("delete-parent", "delete-parent", "/tmp", true, None, None)
            .await
            .unwrap();
        let child = db
            .create_conversation_with_project(
                "delete-child",
                "delete-child",
                "/tmp",
                false,
                Some(&root.id),
                None,
                None,
                &ConvMode::Explore {
                    worktree_path: None,
                    next_taskmd_id_hint: None,
                },
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();

        db.delete_conversation(&root.id).await.unwrap();

        assert!(matches!(
            db.get_conversation(&child.id).await,
            Err(DbError::ConversationNotFound(id)) if id == child.id
        ));
        let owner_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM product_conversations WHERE id = ?1")
                .bind(root.product_conversation_id.as_str())
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(owner_count, 0);
    }

    // ==================== Fork Proposal Tests ====================

    fn fork_proposal_fixture(id: &str, origin: &str) -> ForkProposal {
        ForkProposal {
            id: id.to_string(),
            origin_conversation_id: origin.to_string(),
            task_file: "tasks/00042-p1-ready--fix-thing.md".to_string(),
            title: "Fix Thing".to_string(),
            priority: "p1".to_string(),
            body: "# Fix Thing\n\nDo the thing.".to_string(),
            status: ForkProposalStatus::Pending,
            fork_conversation_id: None,
            refinement_conversation_id: None,
            created_at: Utc::now(),
            resolved_at: None,
        }
    }

    /// Build a child fork/refinement `Conversation` carrying the breadcrumb.
    async fn child_conv_fixture(db: &Database, id: &str, spawned_from: &str) -> Conversation {
        // Persist + read a base row, then mutate into the child shape.
        let base = db
            .create_conversation(id, &format!("slug-{id}"), "/tmp/fork", true, None, None)
            .await
            .unwrap();
        // Remove it so the resolve path inserts it fresh inside its own tx.
        db.delete_conversation(id).await.unwrap();
        Conversation {
            spawned_from_conversation_id: Some(spawned_from.to_string()),
            ..base
        }
    }

    fn seed_msg(conv_id: &str) -> Message {
        Message {
            message_id: format!("seed-{conv_id}"),
            conversation_id: conv_id.to_string(),
            sequence_id: 1,
            message_type: MessageType::User,
            content: MessageContent::user("seed brief body"),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn fork_proposal_insert_get_roundtrip() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-1", "o1", "/tmp", true, None, None)
            .await
            .unwrap();

        let p = fork_proposal_fixture("fp-1", "origin-1");
        db.insert_fork_proposal(&p).await.unwrap();

        let got = db.get_fork_proposal("fp-1").await.unwrap().unwrap();
        assert_eq!(got.id, "fp-1");
        assert_eq!(got.origin_conversation_id, "origin-1");
        assert_eq!(got.status, ForkProposalStatus::Pending);
        assert_eq!(got.task_file, p.task_file);
        assert_eq!(got.body, p.body);
        assert_eq!(got.fork_conversation_id, None);
        assert!(got.resolved_at.is_none());

        assert!(db.get_fork_proposal("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn persist_fork_proposal_with_tool_round_commits_both() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-tr", "otr", "/tmp", true, None, None)
            .await
            .unwrap();

        let assistant = Message {
            message_id: "asst-1".to_string(),
            conversation_id: "origin-tr".to_string(),
            sequence_id: 10,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let tool_result = Message {
            message_id: "tool-1-result".to_string(),
            conversation_id: "origin-tr".to_string(),
            sequence_id: 11,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-1", "Fork proposal recorded", false),
            display_data: Some(serde_json::json!({ "fork_proposal_id": "fp-tr" })),
            usage_data: None,
            created_at: Utc::now(),
        };

        let proposal = fork_proposal_fixture("fp-tr", "origin-tr");
        db.persist_fork_proposal_with_tool_round(
            "origin-tr",
            &assistant,
            &[tool_result],
            &proposal,
        )
        .await
        .unwrap();

        // The control-plane row committed.
        let got = db.get_fork_proposal("fp-tr").await.unwrap().unwrap();
        assert_eq!(got.status, ForkProposalStatus::Pending);

        // The tool round committed in the same transaction.
        let msgs = db.get_messages("origin-tr").await.unwrap();
        assert!(
            msgs.iter().any(|m| m.message_id == "asst-1"),
            "assistant message must be durable"
        );
        let ack = msgs
            .iter()
            .find(|m| m.message_id == "tool-1-result")
            .expect("synthetic ack must be durable");
        assert_eq!(
            ack.display_data
                .as_ref()
                .and_then(|d| d.get("fork_proposal_id"))
                .and_then(|v| v.as_str()),
            Some("fp-tr"),
            "ack must carry the fork_proposal_id handle"
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn usage_recent_llm_metrics_returns_all_window_rows() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-ttft", "slug-ttft", "/tmp", true, None, None)
            .await
            .unwrap();
        db.upsert_llm_request_metrics(&LlmAttemptMetrics {
            conversation_id: "conv-ttft".to_string(),
            root_conversation_id: "conv-ttft".to_string(),
            request_id: "req-1".to_string(),
            retry_attempt: 1,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-5".to_string(),
            transport: LlmTransport::HttpSse,
            total_duration_ms: 4_000,
            stream: ProviderStreamTelemetry {
                dispatch_to_first_provider_event_ms: Some(100),
                dispatch_to_first_generation_event_ms: Some(900),
                dispatch_to_first_visible_text_ms: Some(950),
                provider_event_count: 1,
                generation_event_count: 1,
                visible_text_event_count: 1,
                max_provider_gap_ms: Some(100),
                max_generation_gap_ms: Some(100),
                output_kind: StreamTelemetryOutputKind::Text,
                completed: true,
            },
            outcome: LlmAttemptOutcome::Success,
        })
        .await
        .unwrap();
        db.upsert_llm_request_metrics(&LlmAttemptMetrics {
            conversation_id: "conv-ttft".to_string(),
            root_conversation_id: "conv-ttft".to_string(),
            request_id: "req-2".to_string(),
            retry_attempt: 2,
            provider: "openai".to_string(),
            model: "gpt-5.6-sol".to_string(),
            transport: LlmTransport::HttpJson,
            total_duration_ms: 8_000,
            stream: ProviderStreamTelemetry {
                dispatch_to_first_provider_event_ms: Some(150),
                dispatch_to_first_generation_event_ms: None,
                dispatch_to_first_visible_text_ms: None,
                provider_event_count: 1,
                generation_event_count: 0,
                visible_text_event_count: 0,
                max_provider_gap_ms: Some(150),
                max_generation_gap_ms: None,
                output_kind: StreamTelemetryOutputKind::None,
                completed: false,
            },
            outcome: LlmAttemptOutcome::ServerError,
        })
        .await
        .unwrap();

        let replacement = LlmAttemptMetrics {
            conversation_id: "conv-ttft".to_string(),
            root_conversation_id: "conv-ttft".to_string(),
            request_id: "req-1".to_string(),
            retry_attempt: 1,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-5".to_string(),
            transport: LlmTransport::HttpSse,
            total_duration_ms: 4_100,
            stream: ProviderStreamTelemetry {
                dispatch_to_first_provider_event_ms: Some(110),
                dispatch_to_first_generation_event_ms: Some(910),
                dispatch_to_first_visible_text_ms: Some(960),
                provider_event_count: 2,
                generation_event_count: 1,
                visible_text_event_count: 1,
                max_provider_gap_ms: Some(800),
                max_generation_gap_ms: None,
                output_kind: StreamTelemetryOutputKind::Text,
                completed: true,
            },
            outcome: LlmAttemptOutcome::Success,
        };
        db.upsert_llm_request_metrics(&replacement).await.unwrap();

        let rows = db
            .usage_recent_llm_metrics("1970-01-01T00:00:00+00:00")
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "all rows in the analytics window are returned"
        );
        let updated = rows
            .iter()
            .find(|row| row.request_id == "req-1")
            .expect("updated request row");
        assert_eq!(updated.retry_attempt, 1);
        assert_eq!(updated.provider, "anthropic");
        assert_eq!(updated.model, "claude-sonnet-5");
        assert_eq!(updated.transport, LlmTransport::HttpSse);
        assert_eq!(updated.dispatch_to_first_generation_event_ms, Some(910));
        assert_eq!(updated.outcome, LlmAttemptOutcome::Success);

        let all_rows = db
            .usage_recent_llm_metrics("1970-01-01T00:00:00+00:00")
            .await
            .unwrap();
        assert_eq!(all_rows.len(), 2);
        let failed = all_rows
            .iter()
            .find(|row| row.request_id == "req-2")
            .expect("failed retry row");
        assert_eq!(failed.retry_attempt, 2);
        assert_eq!(failed.transport, LlmTransport::HttpJson);
        assert_eq!(failed.dispatch_to_first_generation_event_ms, None);
        assert_eq!(failed.outcome, LlmAttemptOutcome::ServerError);
    }

    #[tokio::test]
    async fn persist_tool_round_commits_assistant_and_all_results() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-tr", "ctr", "/tmp", true, None, None)
            .await
            .unwrap();

        let assistant = Message {
            message_id: "asst-tr".to_string(),
            conversation_id: "conv-tr".to_string(),
            sequence_id: 10,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let result_a = Message {
            message_id: "tool-a-result".to_string(),
            conversation_id: "conv-tr".to_string(),
            sequence_id: 11,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-a", "output a", false),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let result_b = Message {
            message_id: "tool-b-result".to_string(),
            conversation_id: "conv-tr".to_string(),
            sequence_id: 12,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-b", "output b", false),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };

        db.persist_tool_round("conv-tr", &assistant, &[result_a, result_b])
            .await
            .unwrap();

        let msgs = db.get_messages("conv-tr").await.unwrap();
        let ids: Vec<&str> = msgs.iter().map(|m| m.message_id.as_str()).collect();
        assert!(
            ids.contains(&"asst-tr"),
            "assistant message must be durable"
        );
        assert!(
            ids.contains(&"tool-a-result"),
            "first tool result must be durable"
        );
        assert!(
            ids.contains(&"tool-b-result"),
            "second tool result must be durable"
        );
    }

    async fn create_runtime_turn_for_terminal_test(
        repo: &workflow::WorkflowRepository,
        conversation_id: &str,
        key: &str,
    ) -> phoenix_workflow::TurnAuthorityId {
        let payload = phoenix_core::domain::sm_event::PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: key.to_string(),
                images: Vec::new(),
                files: Vec::new(),
                message_id: key.to_string(),
                user_agent: None,
                skill_invocation: None,
                expansion_policy:
                    phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::LiteralText,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: key.to_string(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                skill_invocation: None,
            },
        );
        let accepted = repo
            .accept_authoritative_turn(&workflow::AcceptAuthoritativeTurn {
                client_key: phoenix_workflow::ClientTurnKey::new(key).unwrap(),
                prepared: phoenix_workflow::PreparedTurn::from_exact_payload(
                    &phoenix_workflow::ConversationAuthority(conversation_id.to_string()),
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: phoenix_workflow::AcceptedDisposition::Runtime,
                accepted_at: phoenix_workflow::Timestamp(1),
            })
            .await
            .unwrap();
        let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("expected created turn")
        };
        turn_id
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn terminal_checkpoint_probe_classifies_commit_acknowledgement_cuts() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-checkpoint-cuts", "ccc", "/tmp", true, None, None)
            .await
            .unwrap();
        let repo = workflow::WorkflowRepository::new(db.pool().clone());
        let payload = phoenix_core::domain::sm_event::PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: "checkpoint".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                message_id: "checkpoint-cuts".to_string(),
                user_agent: None,
                skill_invocation: None,
                expansion_policy:
                    phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::LiteralText,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: "checkpoint".to_string(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                skill_invocation: None,
            },
        );
        let accepted = repo
            .accept_authoritative_turn(&workflow::AcceptAuthoritativeTurn {
                client_key: phoenix_workflow::ClientTurnKey::new("checkpoint-cuts").unwrap(),
                prepared: phoenix_workflow::PreparedTurn::from_exact_payload(
                    &phoenix_workflow::ConversationAuthority("conv-checkpoint-cuts".to_string()),
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: phoenix_workflow::AcceptedDisposition::Runtime,
                accepted_at: phoenix_workflow::Timestamp(1),
            })
            .await
            .unwrap();
        let phoenix_workflow::TurnOutcome::Created { turn_id, .. } = accepted.outcome else {
            panic!("expected created turn")
        };
        let assistant = Message {
            message_id: "checkpoint-assistant".to_string(),
            conversation_id: "conv-checkpoint-cuts".to_string(),
            sequence_id: 20,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![
                phoenix_core::domain::llm_types::ContentBlock::text("checkpoint"),
            ]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let tool = Message {
            message_id: "checkpoint-tool".to_string(),
            conversation_id: "conv-checkpoint-cuts".to_string(),
            sequence_id: 21,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool", "cancelled", true),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let obligation = workflow::DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Cancelled,
            projection: workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            },
            response_message_id: None,
        };
        let evidence =
            workflow::TerminalEvidenceExpectation::Messages(vec![assistant.clone(), tool.clone()]);

        assert!(db
            .persist_tool_round_with_terminal_obligation_at_cut(
                "conv-checkpoint-cuts",
                &assistant,
                std::slice::from_ref(&tool),
                &obligation,
                TerminalEvidenceTransactionCut::BeforeCommit,
            )
            .await
            .is_err());
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::KnownNotCommitted
        );

        assert!(db
            .persist_tool_round_with_terminal_obligation_at_cut(
                "conv-checkpoint-cuts",
                &assistant,
                std::slice::from_ref(&tool),
                &obligation,
                TerminalEvidenceTransactionCut::AfterCommit,
            )
            .await
            .is_err());
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::Established {
                transcript_generation: None
            }
        );

        sqlx::query("DELETE FROM direct_turn_terminal_obligations WHERE turn_id = ?1")
            .bind(i64::try_from(turn_id.0).unwrap())
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::Incomplete
        );
    }

    #[tokio::test]
    async fn terminal_subagent_update_crash_cuts_never_split_evidence() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-subagent-update", "csu", "/tmp", true, None, None)
            .await
            .unwrap();
        let original = MessageContent::tool("spawn", "running", false);
        db.add_message_with_seq(
            "tool-spawn",
            "conv-subagent-update",
            30,
            &original,
            None,
            None,
        )
        .await
        .unwrap();
        let repo = workflow::WorkflowRepository::new(db.pool().clone());
        let turn_id = create_runtime_turn_for_terminal_test(
            &repo,
            "conv-subagent-update",
            "subagent-update-cuts",
        )
        .await;
        let evidence = workflow::TerminalEvidenceExpectation::MessageMutation {
            conversation_id: "conv-subagent-update".to_string(),
            message_id: "tool-spawn".to_string(),
            content: MessageContent::tool("spawn", "cancelled result", false),
            display_data: serde_json::json!({"type":"subagent_results","results":[]}),
        };
        let obligation = workflow::DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Cancelled,
            projection: workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            },
            response_message_id: None,
        };

        assert!(db
            .persist_sub_agent_terminal_evidence_at_cut(
                &evidence,
                &obligation,
                TerminalEvidenceTransactionCut::BeforeCommit,
            )
            .await
            .is_err());
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::KnownNotCommitted
        );
        assert_eq!(
            db.get_message_by_id("tool-spawn").await.unwrap().content,
            original
        );

        assert!(db
            .persist_sub_agent_terminal_evidence_at_cut(
                &evidence,
                &obligation,
                TerminalEvidenceTransactionCut::AfterCommit,
            )
            .await
            .is_err());
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::Established {
                transcript_generation: Some(2)
            }
        );
    }

    #[tokio::test]
    async fn terminal_subagent_insert_crash_cuts_never_split_evidence() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-subagent-cuts", "csc", "/tmp", true, None, None)
            .await
            .unwrap();
        let repo = workflow::WorkflowRepository::new(db.pool().clone());
        let turn_id =
            create_runtime_turn_for_terminal_test(&repo, "conv-subagent-cuts", "subagent-cuts")
                .await;
        let content = MessageContent::User(UserContent::meta("sub-agent result"));
        let message = Message {
            message_id: "subagent-summary-cut".to_string(),
            conversation_id: "conv-subagent-cuts".to_string(),
            sequence_id: 30,
            message_type: content.message_type(),
            content,
            display_data: Some(serde_json::json!({"type":"subagent_summary","results":[]})),
            usage_data: None,
            created_at: Utc::now(),
        };
        let evidence = workflow::TerminalEvidenceExpectation::Messages(vec![message]);
        let obligation = workflow::DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Cancelled,
            projection: workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            },
            response_message_id: None,
        };

        assert!(db
            .persist_sub_agent_terminal_evidence_at_cut(
                &evidence,
                &obligation,
                TerminalEvidenceTransactionCut::BeforeCommit,
            )
            .await
            .is_err());
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::KnownNotCommitted
        );
        assert!(db
            .persist_sub_agent_terminal_evidence_at_cut(
                &evidence,
                &obligation,
                TerminalEvidenceTransactionCut::AfterCommit,
            )
            .await
            .is_err());
        assert_eq!(
            repo.probe_exact_terminal_evidence(&evidence, &obligation)
                .await
                .unwrap(),
            workflow::TerminalEvidenceProbe::Established {
                transcript_generation: None
            }
        );
    }

    #[tokio::test]
    async fn terminal_system_message_rolls_back_when_obligation_fails() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-terminal-system", "cts", "/tmp", true, None, None)
            .await
            .unwrap();
        let obligation = workflow::DirectTurnTerminalObligationInput {
            turn_id: phoenix_workflow::TurnAuthorityId(u64::MAX),
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Completed,
            projection: workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            },
            response_message_id: Some("terminal-system".to_string()),
        };

        assert!(db
            .add_message_with_seq_and_terminal_obligation(
                "terminal-system",
                "conv-terminal-system",
                20,
                &MessageContent::system("Task rejected."),
                None,
                None,
                &obligation,
            )
            .await
            .is_err());
        assert!(db
            .get_messages("conv-terminal-system")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn terminal_tool_round_accepts_only_exact_message_replays() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation_id = "conv-terminal-exact-replay";
        db.create_conversation(conversation_id, "cter", "/tmp", true, None, None)
            .await
            .unwrap();
        let repo = workflow::WorkflowRepository::new(db.pool().clone());
        let turn_id =
            create_runtime_turn_for_terminal_test(&repo, conversation_id, "terminal-exact-replay")
                .await;
        let created_at = Utc::now();
        let assistant = Message {
            message_id: "assistant-terminal-exact".to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id: 20,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![]),
            display_data: None,
            usage_data: None,
            created_at,
        };
        let result = Message {
            message_id: "tool-terminal-exact".to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id: 21,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-terminal", "first durable result", false),
            display_data: Some(serde_json::json!({"kind": "first"})),
            usage_data: None,
            created_at,
        };
        let obligation = workflow::DirectTurnTerminalObligationInput {
            turn_id,
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Completed,
            projection: workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: created_at,
            },
            response_message_id: None,
        };

        db.persist_tool_round_with_terminal_obligation(
            conversation_id,
            &assistant,
            std::slice::from_ref(&result),
            &obligation,
        )
        .await
        .unwrap();
        db.persist_tool_round_with_terminal_obligation(
            conversation_id,
            &assistant,
            std::slice::from_ref(&result),
            &obligation,
        )
        .await
        .expect("an exact replay is idempotent");

        let mut conflicting = result.clone();
        conflicting.content = MessageContent::tool("tool-terminal", "conflicting replay", true);
        let error = db
            .persist_tool_round_with_terminal_obligation(
                conversation_id,
                &assistant,
                &[conflicting],
                &obligation,
            )
            .await
            .expect_err("a conflicting replay must not consume the obligation");
        assert!(error
            .to_string()
            .contains("conflicts with first durable payload"));
        let persisted = db.get_messages(conversation_id).await.unwrap();
        assert_eq!(persisted.len(), 2);
        assert_eq!(persisted[1].content, result.content);
        assert!(repo
            .load_active_terminal_obligation(&phoenix_workflow::ConversationAuthority(
                conversation_id.to_string(),
            ))
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn terminal_tool_round_rolls_back_messages_when_obligation_fails() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-terminal-round", "ctr", "/tmp", true, None, None)
            .await
            .unwrap();
        let assistant = Message {
            message_id: "asst-terminal-round".to_string(),
            conversation_id: "conv-terminal-round".to_string(),
            sequence_id: 20,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let result = Message {
            message_id: "tool-terminal-result".to_string(),
            conversation_id: "conv-terminal-round".to_string(),
            sequence_id: 21,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-terminal", "cancelled", true),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let obligation = workflow::DirectTurnTerminalObligationInput {
            turn_id: phoenix_workflow::TurnAuthorityId(u64::MAX),
            expected_generation: 0,
            terminal: phoenix_workflow::TurnTerminal::Cancelled,
            projection: workflow::PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: Utc::now(),
            },
            response_message_id: None,
        };

        assert!(db
            .persist_tool_round_with_terminal_obligation(
                "conv-terminal-round",
                &assistant,
                &[result],
                &obligation,
            )
            .await
            .is_err());
        assert!(db
            .get_messages("conv-terminal-round")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn persist_tool_round_rolls_back_assistant_when_a_result_insert_fails() {
        // A `tool_use` with no paired `tool_result` 400s every later LLM
        // request. `persist_tool_round` must commit the whole round or none of
        // it. Here the second result names a non-existent conversation, so its
        // INSERT trips the `messages.conversation_id` foreign key (FKs are on)
        // and the transaction rolls back — the assistant message must NOT be
        // left behind unpaired.
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-tr", "ctr", "/tmp", true, None, None)
            .await
            .unwrap();

        let assistant = Message {
            message_id: "asst-tr".to_string(),
            conversation_id: "conv-tr".to_string(),
            sequence_id: 10,
            message_type: MessageType::Agent,
            content: MessageContent::agent(vec![]),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let good_result = Message {
            message_id: "tool-a-result".to_string(),
            conversation_id: "conv-tr".to_string(),
            sequence_id: 11,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-a", "output a", false),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };
        let orphan_fk_result = Message {
            message_id: "tool-b-result".to_string(),
            // No such conversation: FK violation on insert.
            conversation_id: "conv-does-not-exist".to_string(),
            sequence_id: 12,
            message_type: MessageType::Tool,
            content: MessageContent::tool("tool-b", "output b", false),
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
        };

        let err = db
            .persist_tool_round("conv-tr", &assistant, &[good_result, orphan_fk_result])
            .await;
        assert!(err.is_err(), "FK violation must surface as an error");

        // Nothing from the round committed: not the assistant, not the first
        // (otherwise-valid) result. All-or-nothing.
        let msgs = db.get_messages("conv-tr").await.unwrap();
        assert!(
            msgs.is_empty(),
            "rolled-back round must leave no rows; found {:?}",
            msgs.iter().map(|m| &m.message_id).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn fork_proposal_list_for_origin_ordered() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-2", "o2", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("origin-3", "o3", "/tmp", true, None, None)
            .await
            .unwrap();

        let mut a = fork_proposal_fixture("fp-a", "origin-2");
        a.created_at = Utc::now() - chrono::Duration::seconds(10);
        let b = fork_proposal_fixture("fp-b", "origin-2");
        let other = fork_proposal_fixture("fp-c", "origin-3");
        db.insert_fork_proposal(&a).await.unwrap();
        db.insert_fork_proposal(&b).await.unwrap();
        db.insert_fork_proposal(&other).await.unwrap();

        let list = db.list_fork_proposals_for_origin("origin-2").await.unwrap();
        assert_eq!(
            list.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["fp-a", "fp-b"]
        );
    }

    #[tokio::test]
    async fn fork_proposal_dismiss_is_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-4", "o4", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-d", "origin-4"))
            .await
            .unwrap();

        assert!(db.dismiss_fork_proposal("fp-d").await.unwrap());
        let got = db.get_fork_proposal("fp-d").await.unwrap().unwrap();
        assert_eq!(got.status, ForkProposalStatus::Dismissed);
        assert!(got.resolved_at.is_some());

        // Second dismiss updates nothing.
        assert!(!db.dismiss_fork_proposal("fp-d").await.unwrap());
        // Dismissing an unknown id is also false (no row updated).
        assert!(!db.dismiss_fork_proposal("ghost").await.unwrap());
    }

    #[tokio::test]
    async fn fork_proposal_retire_pending_only() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-5", "o5", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-p1", "origin-5"))
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-p2", "origin-5"))
            .await
            .unwrap();
        // Resolve one to spawned so retire must leave it alone.
        let child = child_conv_fixture(&db, "fork-x", "origin-5").await;
        db.resolve_fork_proposal_spawned("fp-p2", &child, &[seed_msg("fork-x")])
            .await
            .unwrap();

        db.retire_pending_fork_proposals_for_origin("origin-5")
            .await
            .unwrap();

        let p1 = db.get_fork_proposal("fp-p1").await.unwrap().unwrap();
        let p2 = db.get_fork_proposal("fp-p2").await.unwrap().unwrap();
        assert_eq!(p1.status, ForkProposalStatus::Dismissed);
        assert_eq!(p2.status, ForkProposalStatus::Spawned);
    }

    #[tokio::test]
    async fn fork_proposal_cascade_deletes_with_origin() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-6", "o6", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-cas", "origin-6"))
            .await
            .unwrap();

        db.delete_conversation("origin-6").await.unwrap();
        assert!(db.get_fork_proposal("fp-cas").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn sub_agent_cwd_override_survives_hydration() {
        let db = Database::open_in_memory().await.unwrap();
        let parent = db
            .create_conversation(
                "parent-override",
                "parent",
                "/tmp/worktree",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let child = db
            .create_conversation_with_project(
                "child-override",
                "child",
                "/tmp/worktree/subdir",
                false,
                Some(&parent.id),
                None,
                None,
                &ConvMode::Explore {
                    worktree_path: None,
                    next_taskmd_id_hint: None,
                },
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();

        assert_eq!(child.cwd, "/tmp/worktree/subdir");
        assert_eq!(
            db.get_conversation(&child.id).await.unwrap().cwd,
            "/tmp/worktree/subdir"
        );

        db.update_conversation_state(
            &child.id,
            &ConvState::ContextExhausted {
                summary: "continue child".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            db.continue_conversation(&child.id).await,
            Err(DbError::ContinuationPrecondition(message))
                if message.contains("subordinate executions")
        ));
        let hydrated = db.get_conversation(&child.id).await.unwrap();
        assert_eq!(hydrated.cwd, "/tmp/worktree/subdir");
        assert!(hydrated.continued_in_conv_id.is_none());
    }

    #[tokio::test]
    async fn resolve_spawned_happy_path_and_idempotent_retry() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-7", "o7", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-s", "origin-7"))
            .await
            .unwrap();
        let child = child_conv_fixture(&db, "fork-7", "origin-7").await;
        let seeds = vec![seed_msg("fork-7")];

        db.resolve_fork_proposal_spawned("fp-s", &child, &seeds)
            .await
            .unwrap();

        let p = db.get_fork_proposal("fp-s").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Spawned);
        assert_eq!(p.fork_conversation_id.as_deref(), Some("fork-7"));
        assert!(p.refinement_conversation_id.is_none());
        assert!(p.resolved_at.is_some());

        // Child + breadcrumb + seed message present.
        let fork = db.get_conversation("fork-7").await.unwrap();
        assert_eq!(
            fork.spawned_from_conversation_id.as_deref(),
            Some("origin-7")
        );
        let msgs = db.get_messages("fork-7").await.unwrap();
        assert_eq!(msgs.len(), 1);

        // Idempotent re-run: same id, no duplicate rows, still one spawned.
        db.resolve_fork_proposal_spawned("fp-s", &child, &seeds)
            .await
            .unwrap();
        let p2 = db.get_fork_proposal("fp-s").await.unwrap().unwrap();
        assert_eq!(p2.status, ForkProposalStatus::Spawned);
        assert_eq!(db.get_messages("fork-7").await.unwrap().len(), 1);
        let orphan_scopes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scopes ws
             WHERE NOT EXISTS (
                 SELECT 1 FROM conversations c WHERE c.work_scope_id = ws.id
             )",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(orphan_scopes, 0);
    }

    #[tokio::test]
    async fn resolve_promoted_happy_path_and_idempotent_retry() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-8", "o8", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-pr", "origin-8"))
            .await
            .unwrap();
        let child = child_conv_fixture(&db, "refine-8", "origin-8").await;
        let seeds = vec![seed_msg("refine-8")];

        db.resolve_fork_proposal_promoted("fp-pr", &child, &seeds)
            .await
            .unwrap();

        let p = db.get_fork_proposal("fp-pr").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Promoted);
        assert_eq!(p.refinement_conversation_id.as_deref(), Some("refine-8"));
        assert!(p.fork_conversation_id.is_none());

        // Idempotent re-run.
        db.resolve_fork_proposal_promoted("fp-pr", &child, &seeds)
            .await
            .unwrap();
        assert_eq!(db.get_messages("refine-8").await.unwrap().len(), 1);
    }

    /// N7: `insert_conversation_tx` is idempotent on the PRIMARY KEY ONLY
    /// (`ON CONFLICT(id) DO NOTHING`). A same-id crash-retry is a no-op (one row),
    /// but a UNIQUE `slug` collision with a DIFFERENT conversation is NOT silently
    /// swallowed — it surfaces as an error rather than skipping the insert (which
    /// would FK-fail the following seed-message insert and roll the whole resolve
    /// back into a permanently-stuck retry loop).
    #[tokio::test]
    async fn insert_conversation_tx_is_pk_only_idempotent_not_slug_swallowing() {
        let db = Database::open_in_memory().await.unwrap();

        // An existing distinct conversation owns a slug.
        db.create_conversation("conv-existing", "fork-collide", "/tmp", true, None, None)
            .await
            .unwrap();

        // Same-id retry of an already-present row is a no-op (PK idempotency).
        db.create_conversation("conv-a", "slug-a", "/tmp", true, None, None)
            .await
            .unwrap();
        let conv_a = db.get_conversation("conv-a").await.unwrap();
        let colliding_product_conversation_id =
            phoenix_core::domain::product_conversation::ProductConversationId::new();
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES (?1, 'ordinary', 'open')",
        )
        .bind(colliding_product_conversation_id.as_str())
        .execute(&db.pool)
        .await
        .unwrap();
        {
            let mut tx = db.pool.begin().await.unwrap();
            insert_conversation_tx(&mut tx, &conv_a).await.unwrap();
            tx.commit().await.unwrap();
        }
        // Still exactly one row, slug unchanged.
        let again = db.get_conversation("conv-a").await.unwrap();
        assert_eq!(again.slug.as_deref(), Some("slug-a"));

        // A DIFFERENT-id conversation reusing an existing slug must NOT be silently
        // skipped: the insert raises rather than swallowing the UNIQUE violation.
        let colliding = Conversation {
            id: "conv-b".to_string(),
            product_conversation_id: colliding_product_conversation_id,
            slug: Some("fork-collide".to_string()),
            ..conv_a
        };
        let mut tx = db.pool.begin().await.unwrap();
        let err = insert_conversation_tx(&mut tx, &colliding)
            .await
            .unwrap_err();
        assert!(
            matches!(err, DbError::Sqlx(_)),
            "a distinct-conversation slug collision must surface as an error, got {err:?}"
        );
        drop(tx);
        // The colliding insert did not create a row.
        assert!(db.get_conversation("conv-b").await.is_err());
    }

    #[tokio::test]
    async fn resolve_conflicts_on_divergent_prior_resolution() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-9", "o9", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-cf", "origin-9"))
            .await
            .unwrap();
        let child = child_conv_fixture(&db, "fork-9", "origin-9").await;
        db.resolve_fork_proposal_spawned("fp-cf", &child, &[seed_msg("fork-9")])
            .await
            .unwrap();

        // Re-resolving spawned to a DIFFERENT id is a conflict.
        let other = child_conv_fixture(&db, "fork-9b", "origin-9").await;
        let err = db
            .resolve_fork_proposal_spawned("fp-cf", &other, &[seed_msg("fork-9b")])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ForkProposalConflict(_)));

        // Promoting an already-spawned proposal is a conflict.
        let err = db
            .resolve_fork_proposal_promoted("fp-cf", &other, &[seed_msg("fork-9b")])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ForkProposalConflict(_)));
    }

    #[tokio::test]
    async fn dangling_breadcrumb_and_fork_id_tolerated() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-10", "o10", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-dang", "origin-10"))
            .await
            .unwrap();
        let child = child_conv_fixture(&db, "fork-10", "origin-10").await;
        db.resolve_fork_proposal_spawned("fp-dang", &child, &[seed_msg("fork-10")])
            .await
            .unwrap();

        // Hard-delete the fork: its raw id on the proposal must dangle (no FK
        // error), the proposal survives, and the origin breadcrumb pointing at a
        // (here also deletable) conversation is non-FK.
        db.delete_conversation("fork-10").await.unwrap();
        let p = db.get_fork_proposal("fp-dang").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Spawned);
        assert_eq!(p.fork_conversation_id.as_deref(), Some("fork-10"));
        assert!(db.get_conversation("fork-10").await.is_err());

        // A conversation whose breadcrumb points at a since-deleted origin
        // persists and reads back.
        db.create_conversation("late-fork", "lf", "/tmp", true, None, None)
            .await
            .unwrap();
        db.delete_conversation("origin-10").await.unwrap();
        // Breadcrumb to the now-gone origin is a raw, dangle-tolerant id.
        let standalone_product_conversation_id =
            phoenix_core::domain::product_conversation::ProductConversationId::new();
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES (?1, 'ordinary', 'open')",
        )
        .bind(standalone_product_conversation_id.as_str())
        .execute(&db.pool)
        .await
        .unwrap();
        let standalone = Conversation {
            id: "dangle-conv".to_string(),
            product_conversation_id: standalone_product_conversation_id,
            slug: Some("dangle-conv".to_string()),
            spawned_from_conversation_id: Some("origin-10".to_string()),
            ..db.get_conversation("late-fork").await.unwrap()
        };
        let mut tx = db.pool.begin().await.unwrap();
        insert_conversation_tx(&mut tx, &standalone).await.unwrap();
        tx.commit().await.unwrap();
        let got = db.get_conversation("dangle-conv").await.unwrap();
        assert_eq!(
            got.spawned_from_conversation_id.as_deref(),
            Some("origin-10")
        );
    }

    /// Transition-graph negative edges out of the `dismissed` terminal
    /// (REQ-PROJ-034, `ForkProposal` `transitions status`): once a proposal is
    /// `dismissed` it has no outbound resolution edge, so resolving it as
    /// `spawned` or `promoted` is a conflict, and a second `dismiss` is a no-op.
    /// `resolve_conflicts_on_divergent_prior_resolution` only exercises edges out
    /// of `spawned`; this covers the `dismissed` source row.
    #[tokio::test]
    async fn resolve_out_of_dismissed_terminal_is_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-dt", "odt", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-dt", "origin-dt"))
            .await
            .unwrap();
        assert!(db.dismiss_fork_proposal("fp-dt").await.unwrap());

        let child = child_conv_fixture(&db, "fork-dt", "origin-dt").await;
        // dismissed -> spawned: undeclared edge, rejected.
        let err = db
            .resolve_fork_proposal_spawned("fp-dt", &child, &[seed_msg("fork-dt")])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ForkProposalConflict(_)), "{err:?}");
        // dismissed -> promoted: undeclared edge, rejected.
        let err = db
            .resolve_fork_proposal_promoted("fp-dt", &child, &[seed_msg("fork-dt")])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ForkProposalConflict(_)), "{err:?}");
        // dismissed -> dismissed: terminal self-edge is a no-op (no row updated).
        assert!(!db.dismiss_fork_proposal("fp-dt").await.unwrap());

        // The proposal is unchanged: still dismissed, both resolution ids absent.
        let p = db.get_fork_proposal("fp-dt").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Dismissed);
        assert!(p.fork_conversation_id.is_none());
        assert!(p.refinement_conversation_id.is_none());
    }

    /// Transition-graph negative edges out of the `promoted` terminal
    /// (REQ-PROJ-037, `ForkProposal` `transitions status`): `promoted` is terminal,
    /// so a second `promote` to a different id, a cross-resolution to `spawned`,
    /// and a `dismiss` are all rejected / no-ops. Complements
    /// `resolve_conflicts_on_divergent_prior_resolution` (spawned source) and
    /// `resolve_out_of_dismissed_terminal_is_rejected` (dismissed source).
    #[tokio::test]
    async fn resolve_out_of_promoted_terminal_is_rejected() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-pt", "opt", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-pt", "origin-pt"))
            .await
            .unwrap();
        let child = child_conv_fixture(&db, "refine-pt", "origin-pt").await;
        db.resolve_fork_proposal_promoted("fp-pt", &child, &[seed_msg("refine-pt")])
            .await
            .unwrap();

        let other = child_conv_fixture(&db, "refine-pt-b", "origin-pt").await;
        // promoted -> promoted (different id): conflict.
        let err = db
            .resolve_fork_proposal_promoted("fp-pt", &other, &[seed_msg("refine-pt-b")])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ForkProposalConflict(_)), "{err:?}");
        // promoted -> spawned: undeclared cross-resolution edge, rejected.
        let err = db
            .resolve_fork_proposal_spawned("fp-pt", &other, &[seed_msg("refine-pt-b")])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::ForkProposalConflict(_)), "{err:?}");
        // promoted -> dismissed: terminal, dismiss is a no-op.
        assert!(!db.dismiss_fork_proposal("fp-pt").await.unwrap());

        // Field invariant: a `promoted` proposal has refinement present, fork absent.
        let p = db.get_fork_proposal("fp-pt").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Promoted);
        assert_eq!(p.refinement_conversation_id.as_deref(), Some("refine-pt"));
        assert!(p.fork_conversation_id.is_none());
    }

    /// `spawned -> dismissed` is an undeclared edge: `dismiss_fork_proposal` is
    /// guarded `WHERE status = pending`, so dismissing an already-`spawned`
    /// proposal updates nothing and leaves the spawned resolution intact. The
    /// fork conversation id survives (the live, decoupled child).
    #[tokio::test]
    async fn dismiss_after_spawned_is_a_noop() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-sd", "osd", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-sd", "origin-sd"))
            .await
            .unwrap();
        let child = child_conv_fixture(&db, "fork-sd", "origin-sd").await;
        db.resolve_fork_proposal_spawned("fp-sd", &child, &[seed_msg("fork-sd")])
            .await
            .unwrap();

        assert!(!db.dismiss_fork_proposal("fp-sd").await.unwrap());
        let p = db.get_fork_proposal("fp-sd").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Spawned);
        assert_eq!(p.fork_conversation_id.as_deref(), Some("fork-sd"));
        assert!(p.refinement_conversation_id.is_none());
    }

    /// State-dependent field invariant (`ForkProposal` entity): the resolution
    /// fields are present iff in their matching terminal state. A freshly
    /// inserted `pending` proposal carries BOTH `fork` and `refinement` absent.
    /// `fork_proposal_insert_get_roundtrip` asserts `fork` is absent; this also
    /// pins `refinement` absent while pending.
    #[tokio::test]
    async fn pending_proposal_has_no_resolution_fields() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-pp", "opp", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-pp", "origin-pp"))
            .await
            .unwrap();

        let p = db.get_fork_proposal("fp-pp").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Pending);
        assert!(p.fork_conversation_id.is_none());
        assert!(p.refinement_conversation_id.is_none());
    }

    /// State-dependent field invariant: a `dismissed` proposal has BOTH `fork`
    /// and `refinement` absent (the resolution spawned/promoted nothing).
    /// `fork_proposal_dismiss_is_idempotent` checks the status; this pins the
    /// field absence the entity's present-iff-terminal contract requires.
    #[tokio::test]
    async fn dismissed_proposal_has_no_resolution_fields() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("origin-df", "odf", "/tmp", true, None, None)
            .await
            .unwrap();
        db.insert_fork_proposal(&fork_proposal_fixture("fp-df", "origin-df"))
            .await
            .unwrap();
        assert!(db.dismiss_fork_proposal("fp-df").await.unwrap());

        let p = db.get_fork_proposal("fp-df").await.unwrap().unwrap();
        assert_eq!(p.status, ForkProposalStatus::Dismissed);
        assert!(p.fork_conversation_id.is_none());
        assert!(p.refinement_conversation_id.is_none());
    }

    async fn retirement_fixture(
        db: &Database,
        id: &str,
        role: RuntimeRole,
        state: &ConvState,
    ) -> WorkScopeId {
        let conv = db
            .create_conversation_with_project(
                id,
                id,
                "/tmp/retirement",
                true,
                None,
                None,
                None,
                &ConvMode::Direct,
                None,
                None,
                None,
                phoenix_core::llm_language::LlmLanguage::default(),
            )
            .await
            .unwrap();
        db.update_conversation_state(id, state).await.unwrap();
        if role != RuntimeRole::User {
            sqlx::query("UPDATE conversations SET runtime_role = ?1 WHERE id = ?2")
                .bind(role.as_str())
                .bind(id)
                .execute(db.pool())
                .await
                .unwrap();
        }
        conv.attached_work_scope_id.unwrap()
    }

    fn no_live_resource(scope: WorkScopeId) -> WorkScopeRetirementPrecondition {
        WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(scope)
    }

    #[tokio::test]
    async fn retirement_blocks_current_user_owner() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = retirement_fixture(&db, "owner", RuntimeRole::User, &ConvState::Idle).await;
        assert_eq!(
            db.retire_work_scope(no_live_resource(scope), "cleanup")
                .await
                .unwrap(),
            WorkScopeRetirementOutcome::Blocked(WorkScopeRetirementBlocker::CurrentUserOwner)
        );
    }

    #[tokio::test]
    async fn retirement_blocks_user_successor() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            retirement_fixture(&db, "predecessor", RuntimeRole::User, &ConvState::Terminal).await;
        retirement_fixture(&db, "successor", RuntimeRole::User, &ConvState::Terminal).await;
        let predecessor = db.get_conversation("predecessor").await.unwrap();
        recreate_test_conversation_in_product(
            &db,
            "successor",
            predecessor.product_conversation_id,
            "predecessor",
        )
        .await;
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = 'successor'")
            .bind(scope.as_str())
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            db.retire_work_scope(no_live_resource(scope), "cleanup")
                .await
                .unwrap(),
            WorkScopeRetirementOutcome::Blocked(WorkScopeRetirementBlocker::UserSuccessor)
        );
    }

    #[tokio::test]
    async fn retirement_blocks_active_subagent() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = retirement_fixture(
            &db,
            "terminal-user",
            RuntimeRole::User,
            &ConvState::Terminal,
        )
        .await;
        db.create_conversation_with_project(
            "active-child",
            "active-child",
            "/tmp/retirement",
            false,
            Some("terminal-user"),
            None,
            None,
            &ConvMode::Direct,
            None,
            None,
            None,
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            db.get_conversation("active-child")
                .await
                .unwrap()
                .attached_work_scope_id,
            Some(scope.clone())
        );
        assert_eq!(
            db.retire_work_scope(no_live_resource(scope), "cleanup")
                .await
                .unwrap(),
            WorkScopeRetirementOutcome::Blocked(WorkScopeRetirementBlocker::ActiveSubAgent)
        );
    }

    #[tokio::test]
    async fn retirement_blocks_pending_wake_workflow() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            retirement_fixture(&db, "wake-owner", RuntimeRole::User, &ConvState::Terminal).await;
        sqlx::query("INSERT INTO workflows (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled, external_acceptance_enabled, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at) VALUES (900, 'wake', 1, 1, 0, 0, 0, 'Active', 'wake', 1, X'00', 1, 1)")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workflow_effects (workflow_id, effect_id, declared_workflow_version, family, kind, intent_codec_family, intent_codec_version, intent_payload, generation, role, capability_kind, status) VALUES (900, 1, 0, 'wake', 'observe', 'wake', 1, X'00', 0, 'Required', 'ReclaimableObservation', 'Eligible')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO wake_bindings (workflow_id, conversation_id, contract_id, profile_kind, profile_version, work_scope_id, resource_kind, bash_handle_id, registering_tool_use_id, expires_at, prepared_fingerprint, observe_effect_id, created_at) VALUES (900, 'wake-owner', 'contract', 'wake', 1, ?1, 'Bash', 'b-900', 'tool', 100, 'fingerprint', 1, 1)")
            .bind(scope.as_str()).execute(db.pool()).await.unwrap();
        assert_eq!(
            db.retire_work_scope(no_live_resource(scope), "cleanup")
                .await
                .unwrap(),
            WorkScopeRetirementOutcome::Blocked(WorkScopeRetirementBlocker::PendingWakeOrWorkflow)
        );
    }

    #[tokio::test]
    async fn successful_retirement_preserves_conversation_and_scope_history() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = retirement_fixture(
            &db,
            "history-owner",
            RuntimeRole::User,
            &ConvState::Terminal,
        )
        .await;
        sqlx::query("INSERT INTO work_scope_observed_branches (work_scope_id, repository_identity, branch_name, first_observed_head_oid, last_observed_head_oid, first_observed_at, last_observed_at) VALUES (?1, 'repo', 'topic', 'a', 'b', '1', '2')")
            .bind(scope.as_str()).execute(db.pool()).await.unwrap();
        assert_eq!(
            db.retire_work_scope(no_live_resource(scope.clone()), "resources removed")
                .await
                .unwrap(),
            WorkScopeRetirementOutcome::Retired
        );
        assert!(db.get_conversation("history-owner").await.is_ok());
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle FROM work_scopes WHERE id = ?1")
                .bind(scope.as_str())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(lifecycle, "retired");
        let history: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scope_observed_branches WHERE work_scope_id = ?1",
        )
        .bind(scope.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(history, 1);
    }

    #[tokio::test]
    async fn normalized_environment_is_authoritative_for_reads_and_mode_updates() {
        let db = Database::open_in_memory().await.unwrap();
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new("topic").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/normalized-worktree").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("24703").unwrap(),
            task_title: NonEmptyString::new("normalized").unwrap(),
        };
        retirement_fixture(&db, "normalized", RuntimeRole::User, &ConvState::Idle).await;
        db.update_conversation_mode("normalized", &mode)
            .await
            .unwrap();
        let conv = db.get_conversation("normalized").await.unwrap();
        assert_eq!(
            conv.conv_mode.worktree_path(),
            Some("/tmp/normalized-worktree")
        );
        assert_eq!(
            db.managed_worktree_paths().await.unwrap(),
            vec!["/tmp/normalized-worktree"]
        );
    }

    #[tokio::test]
    async fn attachment_projection_preserves_shared_scope_participants_without_owners() {
        let db = Database::open_in_memory().await.unwrap();
        let root = setup_exhausted_parent(
            &db,
            "work-root",
            "work-root",
            "/tmp/work",
            &work_mode_fixture(),
        )
        .await;
        let successor = match db.continue_conversation(&root.id).await.unwrap() {
            ContinueOutcome::Created(conversation) => conversation,
            other @ (ContinueOutcome::AlreadyContinued(_)
            | ContinueOutcome::ParentNotContextExhausted { .. }) => {
                panic!("expected Created, got {other:?}")
            }
        };
        let scope = root.attached_work_scope_id.as_ref().unwrap();

        assert_eq!(successor.attached_work_scope_id.as_ref(), Some(scope));
        sqlx::query("UPDATE conversations SET transcript_generation = 7 WHERE id = ?1")
            .bind(&successor.id)
            .execute(db.pool())
            .await
            .unwrap();
        let attachments = db.conversation_work_scope_attachments(scope).await.unwrap();
        assert_eq!(
            attachments
                .iter()
                .map(|conversation| conversation.id.as_str())
                .collect::<Vec<_>>(),
            vec!["work-root", successor.id.as_str()]
        );
        assert_eq!(attachments[1].transcript_generation, 7);

        let view_columns: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('conversation_work_scope_attachments') ORDER BY cid",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            view_columns,
            vec!["conversation_id".to_string(), "work_scope_id".to_string()]
        );
    }

    #[tokio::test]
    async fn mode_and_cwd_promotion_updates_normalized_environment_atomically() {
        let db = Database::open_in_memory().await.unwrap();
        let scope =
            retirement_fixture(&db, "legacy-explore", RuntimeRole::User, &ConvState::Idle).await;
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new("topic").unwrap(),
            worktree_path: NonEmptyString::new("/tmp/promoted-worktree").unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("24703").unwrap(),
            task_title: NonEmptyString::new("promoted").unwrap(),
        };

        db.update_conversation_mode_and_cwd("legacy-explore", &mode, "/tmp/promoted-worktree")
            .await
            .unwrap();

        let conv = db.get_conversation("legacy-explore").await.unwrap();
        assert_eq!(conv.cwd, "/tmp/promoted-worktree");
        assert_eq!(conv.attached_work_scope_id.as_ref(), Some(&scope));
        let environment: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT environment_kind, cwd, worktree_path
             FROM work_scopes WHERE id = ?1",
        )
        .bind(scope.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(
            environment,
            (
                "allocated_worktree".to_string(),
                Some("/tmp/promoted-worktree".to_string()),
                Some("/tmp/promoted-worktree".to_string())
            )
        );
    }

    /// Task 02667: a fresh DB's `conversations` table must not carry the
    /// dead `state_data` column (SCHEMA no longer creates it).
    #[tokio::test]
    async fn fresh_db_has_no_state_data_column() {
        let db = Database::open_in_memory().await.unwrap();
        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(
            !columns.iter().any(|c| c == "state_data"),
            "fresh schema must not create state_data, got: {columns:?}"
        );
    }

    /// Task 02667: an upgraded DB that still carries `state_data` from the
    /// pre-typed-state schema gets the column dropped by `run_migrations`.
    #[tokio::test]
    async fn state_data_column_is_dropped_on_upgrade() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();

        // Pre-2667 shape: conversations table still has state_data. SCHEMA's
        // CREATE TABLE IF NOT EXISTS will not overwrite this.
        sqlx::raw_sql(
            "CREATE TABLE conversations (\
                id TEXT PRIMARY KEY, \
                slug TEXT UNIQUE, \
                cwd TEXT NOT NULL DEFAULT '/tmp', \
                parent_conversation_id TEXT, \
                user_initiated BOOLEAN NOT NULL DEFAULT 1, \
                state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', \
                state_data TEXT, \
                state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                created_at TEXT NOT NULL DEFAULT '2025-01-01', \
                updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                archived BOOLEAN NOT NULL DEFAULT 0\
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let db = Database::from_pool_for_tests(pool, String::new());
        db.run_migrations().await.unwrap();

        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(db.pool())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(
            !columns.iter().any(|c| c == "state_data"),
            "state_data should be dropped on upgrade, got: {columns:?}"
        );
    }
}
