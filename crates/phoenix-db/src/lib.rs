//! Database module for Phoenix IDE
//!
//! Provides persistence for conversations and messages.

mod migrations;
pub mod retrieval;
// The schema *types* (MessageContent, ToolResult, ConvState's persisted shape,
// …) moved to the phoenix-core domain crate to break the db↔state_machine
// cycle. Alias the module back as `schema` so the persistence logic in this
// file and `phoenix_db::*` call sites resolve unchanged.
use phoenix_core::domain::db_schema as schema;

pub use migrations::run_pending_migrations;
pub use retrieval::{
    Fts5Retriever, MessageRetriever, ReconcileStats, RetrievalError, RetrievalScope, RetrievedChunk,
};
pub use schema::*;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use thiserror::Error;

fn render_approved_task_brief(
    intro: &str,
    approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
) -> String {
    format!(
        "{intro}\n\n\
         Branch: {}\n\
         Worktree: {}\n\
         Base branch: {}\n\
         Task file: {}\n\n\
         ## Approved plan: {}\n\n\
         Priority: {}\n\n\
         {}",
        approval.branch_name,
        approval.worktree_path,
        approval.base_branch,
        approval.task_file,
        approval.title,
        approval.priority,
        approval.plan
    )
}

fn approved_task_seed_message(
    approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
) -> String {
    render_approved_task_brief("Task approved. Execute the approved plan below.", approval)
}

fn approved_task_handoff_summary(
    approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
) -> String {
    render_approved_task_brief(
        "Task approved and handed off to a fresh Work conversation.",
        approval,
    )
}

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("Conversation not found: {0}")]
    ConversationNotFound(String),
    #[error("Message not found: {0}")]
    MessageNotFound(String),
    #[error("Slug already exists: {0}")]
    SlugExists(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    /// A fork-proposal resolution was attempted but the proposal is already
    /// resolved to a different state or child id (REQ-PROJ-034/037). Distinct
    /// from the idempotent no-op case (same child id), which returns `Ok`.
    #[error("Fork proposal conflict: {0}")]
    ForkProposalConflict(String),
}

pub type DbResult<T> = Result<T, DbError>;

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
pub struct WorkScopePrFeedbackBaseline {
    pub work_scope_id: i64,
    pub pr_number: u64,
    pub captured_at: String,
    pub github_updated_at: Option<String>,
    pub feedback_identities: Vec<String>,
    pub feedback_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkScopePrFeedbackBaselineInput {
    pub pr_number: u64,
    pub captured_at: String,
    pub github_updated_at: Option<String>,
    pub feedback_identities: Vec<String>,
    pub feedback_fingerprints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkScopePrAssociation {
    pub work_scope_id: i64,
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
    pub first_seen_at: String,
    pub last_seen_at: String,
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

fn row_to_work_scope_pr(row: &SqliteRow) -> DbResult<WorkScopePrAssociation> {
    let display_state: String = row.get("display_state");
    Ok(WorkScopePrAssociation {
        work_scope_id: row.get("work_scope_id"),
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

fn pr_association_rank(state: &phoenix_core::domain::pr_display_state::PrDisplayState) -> u8 {
    match state {
        phoenix_core::domain::pr_display_state::PrDisplayState::Open => 0,
        phoenix_core::domain::pr_display_state::PrDisplayState::Draft => 1,
        phoenix_core::domain::pr_display_state::PrDisplayState::Merged => 2,
        phoenix_core::domain::pr_display_state::PrDisplayState::Closed => 3,
    }
}

fn work_scope_db_key(scope: &phoenix_core::work_scope::WorkScope) -> (&'static str, &str) {
    match scope {
        phoenix_core::work_scope::WorkScope::Worktree(value) => ("Worktree", value.as_str()),
        phoenix_core::work_scope::WorkScope::Conversation(value) => {
            ("Conversation", value.as_str())
        }
        phoenix_core::work_scope::WorkScope::Global => ("Global", ""),
    }
}

/// Thread-safe database handle
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Access the underlying connection pool (for migrations and testing).
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn work_scope_id(
        &self,
        scope: &phoenix_core::work_scope::WorkScope,
    ) -> DbResult<Option<i64>> {
        let (scope_type, scope_value) = work_scope_db_key(scope);
        let id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM work_scopes WHERE scope_type = ?1 AND scope_value = ?2",
        )
        .bind(scope_type)
        .bind(scope_value)
        .fetch_optional(&self.pool)
        .await?;
        Ok(id)
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn upsert_work_scope_pr_observations(
        &self,
        scope: &phoenix_core::work_scope::WorkScope,
        observations: &[WorkScopePrObservation],
    ) -> DbResult<i64> {
        let (scope_type, scope_value) = work_scope_db_key(scope);
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO work_scopes (scope_type, scope_value, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(scope_type, scope_value) DO UPDATE SET updated_at = excluded.updated_at",
        )
        .bind(scope_type)
        .bind(scope_value)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        let work_scope_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM work_scopes WHERE scope_type = ?1 AND scope_value = ?2",
        )
        .bind(scope_type)
        .bind(scope_value)
        .fetch_one(&mut *tx)
        .await?;
        for pr in observations {
            sqlx::query(
                "INSERT INTO work_scope_pr_associations (
                    work_scope_id, repo_owner, repo_name, pr_number, title, url, state, draft,
                    display_state, base, head, github_updated_at, first_seen_at, last_seen_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
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
            .bind(work_scope_id)
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
        scope: &phoenix_core::work_scope::WorkScope,
    ) -> DbResult<Vec<WorkScopePrAssociation>> {
        let Some(work_scope_id) = self.work_scope_id(scope).await? else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query(
            "SELECT work_scope_id, repo_owner, repo_name, pr_number, title, url, state, draft,
                    display_state, base, head, github_updated_at, first_seen_at, last_seen_at
             FROM work_scope_pr_associations
             WHERE work_scope_id = ?1",
        )
        .bind(work_scope_id)
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
        scope: &phoenix_core::work_scope::WorkScope,
    ) -> DbResult<Option<WorkScopePrAssociation>> {
        let mut prs = self.list_work_scope_pr_associations(scope).await?;
        sort_work_scope_pr_associations(&mut prs);
        Ok(prs.into_iter().next())
    }

    /// # Errors
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn upsert_work_scope_pr_feedback_baseline(
        &self,
        scope: &phoenix_core::work_scope::WorkScope,
        baseline: &WorkScopePrFeedbackBaselineInput,
    ) -> DbResult<i64> {
        let (scope_type, scope_value) = work_scope_db_key(scope);
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO work_scopes (scope_type, scope_value, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(scope_type, scope_value) DO UPDATE SET updated_at = excluded.updated_at",
        )
        .bind(scope_type)
        .bind(scope_value)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        let work_scope_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM work_scopes WHERE scope_type = ?1 AND scope_value = ?2",
        )
        .bind(scope_type)
        .bind(scope_value)
        .fetch_one(&mut *tx)
        .await?;
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
                work_scope_id, pr_number, captured_at, github_updated_at, feedback_identities, feedback_fingerprints
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(work_scope_id, pr_number) DO UPDATE SET
                captured_at = excluded.captured_at,
                github_updated_at = excluded.github_updated_at,
                feedback_identities = excluded.feedback_identities,
                feedback_fingerprints = excluded.feedback_fingerprints",
        )
        .bind(work_scope_id)
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
    pub async fn work_scope_pr_feedback_baseline(
        &self,
        scope: &phoenix_core::work_scope::WorkScope,
        pr_number: u64,
    ) -> DbResult<Option<WorkScopePrFeedbackBaseline>> {
        let Some(work_scope_id) = self.work_scope_id(scope).await? else {
            return Ok(None);
        };
        let row = sqlx::query(
            "SELECT work_scope_id, pr_number, captured_at, github_updated_at, feedback_identities, feedback_fingerprints
             FROM work_scope_pr_feedback_baselines
             WHERE work_scope_id = ?1 AND pr_number = ?2",
        )
        .bind(work_scope_id)
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
                work_scope_id: row.get("work_scope_id"),
                pr_number: row.get::<i64, _>("pr_number").cast_unsigned(),
                captured_at: row.get("captured_at"),
                github_updated_at: row.get("github_updated_at"),
                feedback_identities,
                feedback_fingerprints,
            })
        })
        .transpose()
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
        let pool = SqlitePoolOptions::new().connect_with(opts).await?;
        let db = Self { pool };
        db.run_migrations().await?;
        Ok(db)
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
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        let db = Self { pool };
        db.run_migrations().await?;
        migrations::run_pending_migrations(&db.pool).await?;
        Ok(db)
    }

    async fn run_migrations(&self) -> DbResult<()> {
        sqlx::raw_sql(SCHEMA).execute(&self.pool).await?;
        sqlx::raw_sql(MIGRATION_TYPED_STATE)
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
        let _ = sqlx::raw_sql(MIGRATION_RENAME_MESSAGE_ID)
            .execute(&self.pool)
            .await;

        // Replace "unknown" error_kind with "server_error" in stored conversation state
        let _ = sqlx::raw_sql(MIGRATION_REMOVE_UNKNOWN_ERROR_KIND)
            .execute(&self.pool)
            .await;

        // Create projects table (idempotent via IF NOT EXISTS)
        let _ = sqlx::raw_sql(MIGRATION_CREATE_PROJECTS)
            .execute(&self.pool)
            .await;

        // Add project_id and conv_mode columns to conversations
        // Each ALTER TABLE is independent; ignore errors if columns already exist
        let _ = sqlx::raw_sql(
            "ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id)",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::raw_sql(
            "ALTER TABLE conversations ADD COLUMN conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}'",
        )
        .execute(&self.pool)
        .await;

        // Add title column for human-readable conversation names
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN title TEXT")
            .execute(&self.pool)
            .await;

        // Add desired_base_branch for Managed mode branch selection
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN desired_base_branch TEXT")
            .execute(&self.pool)
            .await;

        // Create mcp_disabled_servers table (idempotent via IF NOT EXISTS)
        let _ = sqlx::raw_sql(MIGRATION_CREATE_MCP_DISABLED_SERVERS)
            .execute(&self.pool)
            .await;

        // Create share_tokens table (REQ-AUTH-008, idempotent via IF NOT EXISTS)
        let _ = sqlx::raw_sql(MIGRATION_CREATE_SHARE_TOKENS)
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

        // Steering queue: pending user messages queued while the conversation
        // was busy. Delivered FIFO when the conversation next reaches Idle.
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
        let main_ref = schema::resolve_default_branch(std::path::Path::new(canonical_path))
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
        let now = Utc::now();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();
        let conv_mode_json = serde_json::to_string(conv_mode).unwrap();
        let now_str = now.to_rfc3339();

        // Retry with a random suffix on slug collision (UNIQUE constraint).
        let mut actual_slug = slug.to_string();
        let mut attempts = 0u8;
        loop {
            let title_str = schema::title_from_slug(&actual_slug);
            let result = sqlx::query(
                "INSERT INTO conversations (id, slug, title, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived, model, project_id, conv_mode, desired_base_branch, seed_parent_id, seed_label, llm_language)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8, 0, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )
            .bind(id)
            .bind(&actual_slug)
            .bind(&title_str)
            .bind(cwd)
            .bind(parent_id)
            .bind(user_initiated)
            .bind(&idle_state)
            .bind(&now_str)
            .bind(model)
            .bind(project_id)
            .bind(&conv_mode_json)
            .bind(desired_base_branch)
            .bind(seed_parent_id)
            .bind(seed_label)
            .bind(llm_language.as_str())
            .execute(&self.pool)
            .await;

            match result {
                Ok(_) => break,
                Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("2067") => {
                    attempts += 1;
                    if attempts >= 10 {
                        // Last resort: full UUID fragment (UUIDs are ASCII, first 8 bytes always valid)
                        let uuid_str = uuid::Uuid::new_v4().to_string();
                        actual_slug = format!("{slug}-{}", uuid_str.get(..8).unwrap_or(&uuid_str));
                    } else {
                        actual_slug = format!("{slug}-{:04x}", rand::random::<u16>());
                    }
                }
                Err(e) => return Err(DbError::Sqlx(e)),
            }
        }

        let title = schema::title_from_slug(&actual_slug);
        Ok(Conversation {
            id: id.to_string(),
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
            conv_mode: conv_mode.clone(),
            desired_base_branch: desired_base_branch.map(String::from),
            message_count: 0,
            seed_parent_id: seed_parent_id.map(String::from),
            seed_label: seed_label.map(String::from),
            // REQ-BED-030: fresh conversations have not been continued.
            continued_in_conv_id: None,
            // REQ-CHN-007: fresh conversations have no user-set chain name.
            chain_name: None,
            steering_queue: vec![],
            llm_language,
            spawned_from_conversation_id: None,
        })
    }

    /// Get conversation by ID
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_conversation(&self, id: &str) -> DbResult<Conversation> {
        sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.steering_queue, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c WHERE c.id = ?1",
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
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.steering_queue, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c WHERE c.slug = ?1",
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

    /// List active (non-archived) user-initiated conversations
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_conversations(&self) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.steering_queue, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             WHERE c.archived = 0 AND c.user_initiated = 1
             ORDER BY c.updated_at DESC",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// List archived conversations
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn list_archived_conversations(&self) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.steering_queue, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             WHERE c.archived = 1 AND c.user_initiated = 1
             ORDER BY c.updated_at DESC",
        )
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
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
            "UPDATE conversations SET state = ?1, state_updated_at = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(&state_json)
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

    /// Update the steering queue for a conversation. Persists the FIFO queue
    /// of pending steering messages to `conversations.steering_queue` wrapped
    /// in the versioned [`phoenix_core::domain::sm_event::SteeringQueueEnvelope`]
    /// (see [`phoenix_core::domain::sm_event`]).
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
        let queue_json = phoenix_core::domain::sm_event::SteeringQueueEnvelope::to_json(queue)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let result = sqlx::query(
            "UPDATE conversations SET steering_queue = ?1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(&queue_json)
        .bind(now.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Remove the specified `message_ids` from `conversations.steering_queue`.
    /// Read-filter-write inside a transaction so a concurrent
    /// `enqueue_steer_message` cannot lose a steer that arrived during the
    /// drain window.
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
        let row = sqlx::query("SELECT steering_queue FROM conversations WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Err(DbError::ConversationNotFound(id.to_string()));
        };
        let queue_str: Option<String> = row.try_get("steering_queue")?;
        let queue_str = queue_str.unwrap_or_else(|| "[]".to_string());
        let mut queue = phoenix_core::domain::sm_event::decode_steering_queue(id, &queue_str);
        let to_remove: std::collections::HashSet<&str> =
            message_ids.iter().map(String::as_str).collect();
        queue.retain(|entry| !to_remove.contains(entry.message_id.as_str()));
        let new_json = phoenix_core::domain::sm_event::SteeringQueueEnvelope::to_json(&queue)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        sqlx::query("UPDATE conversations SET steering_queue = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(&new_json)
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
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
        let now = Utc::now();
        let mode_json = serde_json::to_string(mode).unwrap();

        let result =
            sqlx::query("UPDATE conversations SET conv_mode = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(&mode_json)
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
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
             AND json_extract(conv_mode, '$.mode') = 'Work'",
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
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.steering_queue, c.llm_language,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             WHERE c.archived = 0
               AND json_extract(c.conv_mode, '$.worktree_path') = ?1",
        )
        .bind(worktree_path)
        .try_map(parse_conversation_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Update a conversation's working directory.
    ///
    /// Conversation `cwd` is immutable post-creation. The only legitimate
    /// mutations are recovery/teardown fallbacks: promoting an Explore
    /// worktree in place at task approval, and pointing a terminal
    /// conversation at the repo root after its worktree is deleted. The
    /// `_recovery_only` suffix exists so this mutation is not casually
    /// reachable — see task 13012 and `cwd_immutability_tests`.
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
        let now = Utc::now();
        let result =
            sqlx::query("UPDATE conversations SET cwd = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(cwd)
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Create a fresh Work conversation for an approved-task handoff and link
    /// the Explore predecessor through `continued_in_conv_id`.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    ///
    /// # Panics
    ///
    /// Panics if persisted JSON columns cannot be (de)serialized.
    #[allow(clippy::too_many_lines)]
    pub async fn create_task_approval_handoff_conversation(
        &self,
        parent_id: &str,
        approval: &phoenix_core::task_handoff::TaskApprovalHandoffData,
    ) -> DbResult<Conversation> {
        let parent = self.get_conversation(parent_id).await?;
        if let Some(existing_id) = parent.continued_in_conv_id.as_deref() {
            return self.get_conversation(existing_id).await;
        }

        let new_id = uuid::Uuid::new_v4().to_string();
        let root_id = self
            .chain_root_of(parent_id)
            .await?
            .unwrap_or_else(|| parent_id.to_string());
        let root = self.get_conversation(&root_id).await?;
        let chain_len = self.chain_members_forward(&root_id).await?.len();
        let root_slug = root.slug.as_deref().unwrap_or("conversation");
        let mut candidate_slug = format!("{root_slug}-{}", chain_len + 1);
        let mut slug_offset = 0usize;
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let handed_off_state = serde_json::to_string(&ConvState::HandedOff {
            successor_conv_id: new_id.clone(),
        })
        .unwrap();
        let work_mode = ConvMode::Work {
            branch_name: schema::NonEmptyString::new(approval.branch_name.clone())
                .expect("approved branch name is non-empty"),
            worktree_path: schema::NonEmptyString::new(approval.worktree_path.clone())
                .expect("approved worktree path is non-empty"),
            base_branch: schema::NonEmptyString::new(approval.base_branch.clone())
                .expect("approved base branch is non-empty"),
            task_id: schema::NonEmptyString::new(approval.task_id.clone())
                .expect("approved task id is non-empty"),
            task_title: schema::NonEmptyString::new(approval.task_title.clone())
                .expect("approved task title is non-empty"),
        };
        let work_mode_json = serde_json::to_string(&work_mode).unwrap();
        let seed_message_id = uuid::Uuid::new_v4().to_string();
        let seeded_state = serde_json::to_string(&ConvState::SeededLlmRequesting {
            seed_message_id: seed_message_id.clone(),
            attempt: 1,
        })
        .unwrap();
        let seed_content =
            MessageContent::User(UserContent::meta(approved_task_seed_message(approval)));
        let seed_content_str = serde_json::to_string(&seed_content.to_json()).unwrap();
        let seed_display = serde_json::json!({ "user_agent": "Phoenix Task Handoff" });
        let seed_display_str = serde_json::to_string(&seed_display).unwrap();
        let handoff_summary = MessageContent::continuation(approved_task_handoff_summary(approval));
        let handoff_summary_str = serde_json::to_string(&handoff_summary.to_json()).unwrap();

        let mut tx = self.pool.begin().await?;
        let actual_slug = loop {
            let title_for_insert = schema::title_from_slug(&candidate_slug);
            let result = sqlx::query(
                "INSERT INTO conversations (id, slug, title, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived, model, project_id, conv_mode, desired_base_branch, seed_parent_id, seed_label, continued_in_conv_id, llm_language)
                 VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5, ?6, ?6, ?6, 0, ?7, ?8, ?9, ?10, NULL, NULL, NULL, ?11)",
            )
            .bind(&new_id)
            .bind(&candidate_slug)
            .bind(&title_for_insert)
            .bind(&approval.worktree_path)
            .bind(&seeded_state)
            .bind(&now_str)
            .bind(parent.model.as_deref())
            .bind(parent.project_id.as_deref())
            .bind(&work_mode_json)
            .bind(parent.desired_base_branch.as_deref())
            .bind(parent.llm_language.as_str())
            .execute(&mut *tx)
            .await;

            match result {
                Ok(_) => break candidate_slug,
                Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("2067") => {
                    slug_offset += 1;
                    candidate_slug = if slug_offset <= 20 {
                        format!("{root_slug}-{}", chain_len + 1 + slug_offset)
                    } else {
                        let uid = uuid::Uuid::new_v4().to_string();
                        format!(
                            "{root_slug}-{}-{}",
                            chain_len + 1,
                            uid.get(..8).unwrap_or(&uid)
                        )
                    };
                }
                Err(e) => return Err(DbError::Sqlx(e)),
            }
        };

        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, NULL, ?6)",
        )
        .bind(&seed_message_id)
        .bind(&new_id)
        .bind(seed_content.message_type().to_string())
        .bind(&seed_content_str)
        .bind(&seed_display_str)
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;

        let parent_next_sequence_id: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(parent_id)
        .fetch_one(&mut *tx)
        .await?;

        let handoff_summary_msg_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6)",
        )
        .bind(&handoff_summary_msg_id)
        .bind(parent_id)
        .bind(parent_next_sequence_id)
        .bind(handoff_summary.message_type().to_string())
        .bind(&handoff_summary_str)
        .bind(&now_str)
        .execute(&mut *tx)
        .await?;

        let updated = sqlx::query(
            "UPDATE conversations
             SET continued_in_conv_id = ?1, state = ?2, state_updated_at = ?3, updated_at = ?3
             WHERE id = ?4 AND continued_in_conv_id IS NULL",
        )
        .bind(&new_id)
        .bind(&handed_off_state)
        .bind(&now_str)
        .bind(parent_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            drop(tx);
            let refetched = self.get_conversation(parent_id).await?;
            if let Some(existing_id) = refetched.continued_in_conv_id.as_deref() {
                return self.get_conversation(existing_id).await;
            }
            return Err(DbError::ConversationNotFound(parent_id.to_string()));
        }

        // Index the two messages this handoff inserts (the seed message and the
        // parent's continuation summary) in the same transaction — they go in
        // via raw INSERTs that bypass the `add_message` index hook, so without
        // this they'd be missing from retrieval until the next startup
        // reconcile (specs/conversation-retrieval/ REQ-RET-003).
        let seed_msg = Message {
            message_id: seed_message_id.clone(),
            conversation_id: new_id.clone(),
            sequence_id: 1,
            message_type: seed_content.message_type(),
            content: seed_content.clone(),
            display_data: None,
            usage_data: None,
            created_at: now,
        };
        let handoff_msg = Message {
            message_id: handoff_summary_msg_id,
            conversation_id: parent_id.to_string(),
            sequence_id: parent_next_sequence_id,
            message_type: handoff_summary.message_type(),
            content: handoff_summary.clone(),
            display_data: None,
            usage_data: None,
            created_at: now,
        };
        retrieval::fts_upsert_conn(&mut tx, &seed_msg).await?;
        retrieval::fts_upsert_conn(&mut tx, &handoff_msg).await?;

        tx.commit().await?;

        Ok(Conversation {
            id: new_id,
            slug: Some(actual_slug.clone()),
            title: Some(schema::title_from_slug(&actual_slug)),
            cwd: approval.worktree_path.clone(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::SeededLlmRequesting {
                seed_message_id,
                attempt: 1,
            },
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: parent.model,
            project_id: parent.project_id,
            conv_mode: work_mode,
            desired_base_branch: parent.desired_base_branch,
            message_count: 1,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            chain_name: None,
            steering_queue: vec![],
            llm_language: parent.llm_language,
            spawned_from_conversation_id: None,
        })
    }

    /// Create a continuation conversation for a context-exhausted parent, atomically.
    ///
    /// Implements REQ-BED-030 (see `specs/bedrock/design.md` §"Context Continuation
    /// Worktree Transfer" and `projects.allium` rules
    /// `WorktreeTransferredOnContinuation` / `DirectContinuationInheritsCwd`).
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
        let conv_mode_json = serde_json::to_string(&parent.conv_mode).unwrap();

        // Atomic INSERT + UPDATE. On any error before `commit()`, the
        // transaction guard drops and SQLite rolls back.
        let mut tx = self.pool.begin().await?;

        // Retry on slug collision (UNIQUE constraint, SQLite error 2067).
        // Collisions are rare: concurrent continuations racing for the same
        // sequential number, or an unrelated conversation sharing the name.
        let actual_slug = loop {
            let title_for_insert = schema::title_from_slug(&candidate_slug);
            let result = sqlx::query(
                "INSERT INTO conversations (id, slug, title, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived, model, project_id, conv_mode, desired_base_branch, seed_parent_id, seed_label, continued_in_conv_id, llm_language)
                 VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5, ?6, ?6, ?6, 0, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13)",
            )
            .bind(&new_id)
            .bind(&candidate_slug)
            .bind(&title_for_insert)
            .bind(&parent.cwd)
            .bind(&idle_state)
            .bind(&now_str)
            .bind(parent.model.as_deref())
            .bind(parent.project_id.as_deref())
            .bind(&conv_mode_json)
            .bind(parent.desired_base_branch.as_deref())
            // Continuations do not inherit the parent's seed fields — those are
            // decorative UI metadata for a different concept (REQ-SEED-003/004).
            .bind::<Option<&str>>(None)
            .bind::<Option<&str>>(None)
            .bind(parent.llm_language.as_str())
            .execute(&mut *tx)
            .await;

            match result {
                Ok(_) => break candidate_slug,
                Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("2067") => {
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

        // Guard against TOCTOU: only clear-parent continues succeed. This
        // WHERE clause is the concurrent-continuation check — if another
        // caller raced us between the SELECT above and this UPDATE, the
        // rows_affected will be 0 and we roll back.
        let updated = sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = ?1, updated_at = ?2 \
             WHERE id = ?3 AND continued_in_conv_id IS NULL",
        )
        .bind(&new_id)
        .bind(&now_str)
        .bind(parent_id)
        .execute(&mut *tx)
        .await?;

        if updated.rows_affected() == 0 {
            // Parent got continued by another request between our fetch and
            // our UPDATE. Drop `tx` (rollback) and report the existing
            // continuation via a fresh fetch.
            drop(tx);
            let refetched = self.get_conversation(parent_id).await?;
            if let Some(ref existing_id) = refetched.continued_in_conv_id {
                let existing = self.get_conversation(existing_id).await?;
                tracing::info!(
                    parent_id = %parent_id,
                    existing_continuation = %existing_id,
                    "continue_conversation: lost TOCTOU race, returning winner's continuation",
                );
                return Ok(ContinueOutcome::AlreadyContinued(existing));
            }
            // Parent vanished during the race. Surface as NotFound.
            return Err(DbError::ConversationNotFound(parent_id.to_string()));
        }

        tx.commit().await?;

        let title_str = schema::title_from_slug(&actual_slug);
        let new_conversation = Conversation {
            id: new_id,
            slug: Some(actual_slug),
            title: Some(title_str),
            cwd: parent.cwd,
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: parent.model,
            project_id: parent.project_id,
            conv_mode: parent.conv_mode,
            desired_base_branch: parent.desired_base_branch,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id: None,
            // Continuations are not chain roots — chain_name lives on the
            // root only (REQ-CHN-007).
            chain_name: None,
            steering_queue: vec![],
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
                snapshot_member_count, snapshot_total_messages,
                created_at, completed_at
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, NULL)",
        )
        .bind(&row.id)
        .bind(&row.root_conv_id)
        .bind(&row.question)
        .bind(&row.model)
        .bind(ChainQaStatus::InFlight.as_str())
        .bind(row.snapshot_member_count)
        .bind(row.snapshot_total_messages)
        .bind(row.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a Q&A row complete with the final answer (REQ-CHN-005).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    #[allow(dead_code)] // Wired through `chain_qa::ChainQa::submit_question` (Phase 2/3)
    pub async fn complete_chain_qa(
        &self,
        id: &str,
        answer: &str,
        completed_at: DateTime<Utc>,
    ) -> DbResult<()> {
        let result = sqlx::query(
            "UPDATE chain_qa
             SET answer = ?1, status = ?2, completed_at = ?3
             WHERE id = ?4",
        )
        .bind(answer)
        .bind(ChainQaStatus::Completed.as_str())
        .bind(completed_at.to_rfc3339())
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
                    snapshot_member_count, snapshot_total_messages,
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

    /// Update the model for a conversation (e.g., upgrading from 200k to 1M context).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn update_conversation_model(&self, id: &str, model: &str) -> DbResult<()> {
        let now = Utc::now();
        let result =
            sqlx::query("UPDATE conversations SET model = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(model)
                .bind(now.to_rfc3339())
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Get all non-archived Work/Branch conversations (for startup worktree reconciliation).
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_work_conversations(&self) -> DbResult<Vec<Conversation>> {
        sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name, c.steering_queue, c.llm_language, c.spawned_from_conversation_id,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             WHERE c.archived = 0
               AND json_extract(c.conv_mode, '$.mode') IN ('Work', 'Branch')",
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

    /// Delete a conversation and all its messages
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn delete_conversation(&self, id: &str) -> DbResult<()> {
        // Conversation + messages (FK CASCADE) and the standalone FTS prune
        // run in one transaction. The FTS table has no FK cascade, so without
        // the shared transaction a crash between the source delete and the
        // prune would leave orphaned index rows and hard-deleted content could
        // resurface in recall until the next reconcile (REQ-RET-003).
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            // tx dropped without commit → rollback.
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        retrieval::fts_delete_conversation_conn(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
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

        // First, repair any orphaned tool_use blocks
        self.repair_orphaned_tool_use(&now).await?;

        // Reset non-terminal conversations to idle.
        // Preserved states (NOT reset):
        //   - context_exhausted: completed conversations that cannot accept new messages
        //   - awaiting_task_approval: user approval pending; state data (title/priority/plan)
        //     is in the JSON column and must survive restart
        //   - awaiting_user_response: user questions pending; state data (questions/tool_use_id)
        //     is in the JSON column and must survive restart
        //   - terminal: task lifecycle ended (complete/abandon) — permanently read-only
        sqlx::query(
            "UPDATE conversations SET state = ?1, state_updated_at = ?2, updated_at = ?2
             WHERE json_extract(state, '$.type') NOT IN ('idle', 'context_exhausted', 'handed_off', 'seeded_llm_requesting', 'awaiting_task_approval', 'awaiting_user_response', 'terminal')",
        )
        .bind(&idle_state)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Scan all conversations for orphaned `tool_use` and inject synthetic `tool_result`.
    /// An orphaned `tool_use` is an agent message containing `tool_use` blocks where
    /// not all `tool_use` IDs have a corresponding `tool_result` in the following messages.
    ///
    /// Skips conversations in preserved (frozen) states — `context_exhausted`,
    /// `terminal`, `awaiting_task_approval`, `awaiting_user_response`. Those
    /// match the allowlist in `reset_all_to_idle` (the conversation is not
    /// going to make another LLM call, so injecting a synthetic `tool_result`
    /// only adds noise to history).
    async fn repair_orphaned_tool_use(&self, now: &DateTime<Utc>) -> DbResult<()> {
        use phoenix_core::domain::llm_types::ContentBlock;

        // Skip conversations whose state is preserved across restarts; their
        // history is frozen and shouldn't be amended with synthetic results.
        let conv_rows: Vec<String> = sqlx::query(
            "SELECT id FROM conversations
             WHERE json_extract(state, '$.type') NOT IN
                 ('context_exhausted', 'handed_off', 'terminal',
                  'awaiting_task_approval', 'awaiting_user_response')",
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

        let content_str = serde_json::to_string(&content.to_json()).unwrap();
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
        if let Err(e) = retrieval::fts_upsert(&self.pool, &message).await {
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
        let content_str = serde_json::to_string(&content.to_json()).unwrap();
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
        if let Err(e) = retrieval::fts_upsert(&self.pool, &message).await {
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
        let rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
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
        let rows = sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 AND sequence_id > ?2 ORDER BY sequence_id ASC",
        )
        .bind(conversation_id)
        .bind(after_sequence)
        .try_map(parse_message_row)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Get a message by its `message_id`
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the underlying database operation fails.
    pub async fn get_message_by_id(&self, message_id: &str) -> DbResult<Message> {
        sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE message_id = ?1",
        )
        .bind(message_id)
        .try_map(parse_message_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if matches!(e, sqlx::Error::RowNotFound) {
                DbError::MessageNotFound(message_id.to_string())
            } else {
                DbError::Sqlx(e)
            }
        })
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
    ) -> DbResult<()> {
        let display_str = serde_json::to_string(display_data)
            .map_err(|e| DbError::Serialization(e.to_string()))?;
        let result = sqlx::query("UPDATE messages SET display_data = ?1 WHERE message_id = ?2")
            .bind(&display_str)
            .bind(message_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::MessageNotFound(message_id.to_string()));
        }
        Ok(())
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
    ) -> DbResult<()> {
        let result = sqlx::query(
            "UPDATE messages SET content = json_set(content, '$.content', ?1) WHERE message_id = ?2",
        )
        .bind(new_content)
        .bind(message_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::MessageNotFound(message_id.to_string()));
        }
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
        if let Some(message) = updated {
            if let Err(e) = retrieval::fts_upsert(&self.pool, &message).await {
                tracing::warn!(
                    message_id = %message.message_id, error = %e,
                    "failed to index message for retrieval; startup reconcile will repair",
                );
            }
        }
        Ok(())
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
        usage: &phoenix_core::domain::llm_types::Usage,
    ) -> DbResult<()> {
        let now_str = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO turn_usage \
             (conversation_id, root_conversation_id, model, \
              input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(conversation_id)
        .bind(root_conversation_id)
        .bind(model)
        .bind(usage.input_tokens.cast_signed())
        .bind(usage.output_tokens.cast_signed())
        .bind(usage.cache_creation_tokens.cast_signed())
        .bind(usage.cache_read_tokens.cast_signed())
        .bind(&now_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return aggregated token usage for a conversation.
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
}

/// Parse a conversation row from the database
#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_conversation_row(row: SqliteRow) -> Result<Conversation, sqlx::Error> {
    let id: String = row.try_get("id")?;

    let state_json: String = row.try_get("state")?;
    let state: ConvState = match serde_json::from_str(&state_json) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(conv_id = %id, error = %e, raw = %state_json, "Failed to deserialize ConvState, defaulting to Idle");
            ConvState::Idle
        }
    };

    // conv_mode: parse from JSON, default to Explore for old rows without the column
    let conv_mode_raw: Option<String> =
        row.try_get::<Option<String>, _>("conv_mode").ok().flatten();
    let conv_mode: ConvMode = match &conv_mode_raw {
        Some(s) => match serde_json::from_str(s) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(conv_id = %id, error = %e, raw = %s, "Failed to deserialize ConvMode, defaulting to Explore");
                ConvMode::default()
            }
        },
        None => ConvMode::default(),
    };

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

    let steering_queue = row
        .try_get::<Option<String>, _>("steering_queue")
        .unwrap_or(None)
        .as_deref()
        .map(|s| phoenix_core::domain::sm_event::decode_steering_queue(&id, s))
        .unwrap_or_default();

    Ok(Conversation {
        id,
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
        project_id: row
            .try_get::<Option<String>, _>("project_id")
            .unwrap_or(None),
        conv_mode,
        desired_base_branch,
        message_count: row.try_get("message_count")?,
        seed_parent_id,
        seed_label,
        continued_in_conv_id,
        chain_name,
        steering_queue,
        llm_language,
        spawned_from_conversation_id,
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
        snapshot_member_count: row.try_get("snapshot_member_count")?,
        snapshot_total_messages: row.try_get("snapshot_total_messages")?,
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
    let state_json =
        serde_json::to_string(&conv.state).map_err(|e| DbError::Serialization(e.to_string()))?;
    let conv_mode_json = serde_json::to_string(&conv.conv_mode)
        .map_err(|e| DbError::Serialization(e.to_string()))?;
    let steering_json = serde_json::to_string(&conv.steering_queue)
        .map_err(|e| DbError::Serialization(e.to_string()))?;

    sqlx::query(
        "INSERT INTO conversations (
            id, slug, title, cwd, parent_conversation_id, user_initiated, state,
            state_updated_at, created_at, updated_at, archived, model, project_id,
            conv_mode, desired_base_branch, seed_parent_id, seed_label,
            continued_in_conv_id, chain_name, steering_queue, llm_language,
            spawned_from_conversation_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
        ON CONFLICT(id) DO NOTHING",
    )
    .bind(&conv.id)
    .bind(&conv.slug)
    .bind(&conv.title)
    .bind(&conv.cwd)
    .bind(&conv.parent_conversation_id)
    .bind(conv.user_initiated)
    .bind(&state_json)
    .bind(conv.state_updated_at.to_rfc3339())
    .bind(conv.created_at.to_rfc3339())
    .bind(conv.updated_at.to_rfc3339())
    .bind(conv.archived)
    .bind(&conv.model)
    .bind(&conv.project_id)
    .bind(&conv_mode_json)
    .bind(&conv.desired_base_branch)
    .bind(&conv.seed_parent_id)
    .bind(&conv.seed_label)
    .bind(&conv.continued_in_conv_id)
    .bind(&conv.chain_name)
    .bind(&steering_json)
    .bind(conv.llm_language.as_str())
    .bind(&conv.spawned_from_conversation_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert a seed `Message` row inside a transaction, reusing the same column
/// mapping as [`Database::add_message_with_seq`]. `INSERT OR IGNORE` keyed on
/// `message_id` makes a crash-retry a no-op rather than a duplicate.
async fn insert_message_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    msg: &Message,
) -> DbResult<()> {
    let content_str = serde_json::to_string(&msg.content.to_json())
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

    sqlx::query(
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

/// Parse a message row from the database
#[allow(clippy::needless_pass_by_value)] // sqlx try_map passes rows by value
fn parse_message_row(row: SqliteRow) -> Result<Message, sqlx::Error> {
    let msg_type = parse_message_type(&row.try_get::<String, _>("message_type")?);
    let content_str: String = row.try_get("content")?;
    let content_value: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();

    // Parse content using the message type as discriminator
    let content = MessageContent::from_json(msg_type, content_value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::llm_language::LlmLanguage;

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

    #[tokio::test]
    async fn work_scope_pr_association_upsert_preserves_first_seen_and_updates_primary() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScope::Worktree("/tmp/ws-pr".to_string());
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
    }

    #[tokio::test]
    async fn work_scope_pr_feedback_baseline_roundtrips_and_replaces() {
        let db = Database::open_in_memory().await.unwrap();
        let scope = phoenix_core::work_scope::WorkScope::Worktree("/tmp/ws-baseline".to_string());

        db.upsert_work_scope_pr_feedback_baseline(
            &scope,
            &WorkScopePrFeedbackBaselineInput {
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
            .work_scope_pr_feedback_baseline(&scope, 7)
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
    async fn test_create_and_get_conversation() {
        let db = Database::open_in_memory().await.unwrap();

        let conv = db
            .create_conversation("test-id", "test-slug", "/tmp/test", true, None, None)
            .await
            .unwrap();

        assert_eq!(conv.id, "test-id");
        assert_eq!(conv.slug, Some("test-slug".to_string()));
        assert_eq!(conv.cwd, "/tmp/test");
        assert!(matches!(conv.state, ConvState::Idle));

        let fetched = db.get_conversation("test-id").await.unwrap();
        assert_eq!(fetched.id, conv.id);
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
            _ => panic!("Expected User content"),
        }

        let after = db.get_messages_after("conv-1", 1).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message_id, "msg-2");
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
            _ => panic!("Expected Tool content"),
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
                _ => None,
            })
            .collect();
        assert!(tool_ids.contains(&"tool-1".to_string()));
        assert!(tool_ids.contains(&"tool-2".to_string()));
        assert!(tool_ids.contains(&"tool-3".to_string()));
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

        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind("conv-child")
            .bind("conv-parent")
            .execute(&db.pool)
            .await
            .unwrap();

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

    /// Work -> Work: worktree fields and `task_id` all transfer; parent's
    /// `continued_in_conv_id` points at the new conv.
    #[tokio::test]
    async fn test_continue_conversation_work_to_work() {
        let db = Database::open_in_memory().await.unwrap();
        let parent_mode = work_mode_fixture();
        let parent =
            setup_exhausted_parent(&db, "parent-work", "parent-work", "/tmp", &parent_mode).await;

        let outcome = db.continue_conversation("parent-work").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };

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
            other => panic!("expected Created, got {other:?}"),
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
            },
        )
        .await;

        let outcome = db.continue_conversation("parent-explore").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
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
            other => panic!("expected Created, got {other:?}"),
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
            other => panic!("first call should create, got {other:?}"),
        };

        let second = match db.continue_conversation("parent-double").await.unwrap() {
            ContinueOutcome::AlreadyContinued(c) => c,
            other => panic!("second call should return AlreadyContinued, got {other:?}"),
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
            other => panic!("expected ParentNotContextExhausted, got {other:?}"),
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
            other => panic!("expected Created, got {other:?}"),
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
            other => panic!("expected Created, got {other:?}"),
        };
        assert_eq!(
            second.slug.as_deref(),
            Some("my-task-3"),
            "second continuation slug must be {{root_slug}}-3, not the parent slug appended"
        );
    }

    // ------------------------------------------------------------------
    // Phoenix Chains v1 (task 02686): chain_name + chain walk methods
    // ------------------------------------------------------------------

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
        for pair in ids.windows(2) {
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(pair[1])
                .bind(pair[0])
                .execute(&db.pool)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn test_create_task_approval_handoff_links_parent_to_work_successor() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("handoff-parent", "handoff-parent", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message_with_seq(
            "preexisting-parent-message",
            "handoff-parent",
            42,
            &MessageContent::user("existing parent message"),
            None,
            None,
        )
        .await
        .unwrap();

        let approval = phoenix_core::task_handoff::TaskApprovalHandoffData {
            task_id: "27002".to_string(),
            task_title: "Approve Fresh".to_string(),
            branch_name: "task-27002-approve-fresh".to_string(),
            worktree_path: "/tmp/.phoenix/worktrees/handoff-parent".to_string(),
            base_branch: "main".to_string(),
            title: "Approve Fresh".to_string(),
            priority: phoenix_core::task_source::Priority::P1,
            plan: "Do the work".to_string(),
            task_file: "tasks/27002-p1-ready--approve-fresh.md".to_string(),
        };

        let successor = db
            .create_task_approval_handoff_conversation("handoff-parent", &approval)
            .await
            .unwrap();

        let parent = db.get_conversation("handoff-parent").await.unwrap();
        assert_eq!(
            parent.continued_in_conv_id.as_deref(),
            Some(successor.id.as_str())
        );
        assert!(matches!(
            parent.state,
            ConvState::HandedOff { ref successor_conv_id } if successor_conv_id == &successor.id
        ));
        assert!(matches!(
            successor.state,
            ConvState::SeededLlmRequesting { ref seed_message_id, attempt: 1 } if !seed_message_id.is_empty()
        ));
        assert_eq!(successor.message_count, 1);
        let successor_messages = db.get_messages(&successor.id).await.unwrap();
        assert_eq!(successor_messages.len(), 1);
        match &successor_messages[0].content {
            MessageContent::User(user) => {
                assert!(user.is_meta);
                assert!(user.text.contains(&approval.task_file));
                assert!(user.text.contains(&approval.branch_name));
            }
            other => panic!("expected meta user seed message, got {other:?}"),
        }
        let parent_messages = db.get_messages("handoff-parent").await.unwrap();
        assert!(parent_messages.iter().any(|m| {
            matches!(m.content, MessageContent::Continuation(_)) && m.sequence_id == 43
        }));
        match successor.conv_mode {
            ConvMode::Work {
                branch_name,
                worktree_path,
                base_branch,
                task_id,
                task_title,
            } => {
                assert_eq!(branch_name.as_str(), approval.branch_name);
                assert_eq!(worktree_path.as_str(), approval.worktree_path);
                assert_eq!(base_branch.as_str(), approval.base_branch);
                assert_eq!(task_id.as_str(), approval.task_id);
                assert_eq!(task_title.as_str(), approval.task_title);
            }
            other => panic!("successor should be Work mode, got {other:?}"),
        }
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
            snapshot_member_count: 3,
            snapshot_total_messages: 17,
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
        assert_eq!(row.snapshot_member_count, 3);
        assert_eq!(row.snapshot_total_messages, 17);
        assert!(row.completed_at.is_none());
    }

    /// REQ-CHN-005: `complete_chain_qa` sets answer + `completed_at` + status.
    #[tokio::test]
    async fn test_complete_chain_qa_transitions_row() {
        let db = Database::open_in_memory().await.unwrap();
        build_linear_chain(&db, &["qac-a", "qac-b"]).await;
        db.insert_chain_qa(fresh_new_chain_qa("qac-1", "qac-a"))
            .await
            .unwrap();

        let now = Utc::now();
        db.complete_chain_qa("qac-1", "the final answer", now)
            .await
            .unwrap();

        let row = &db.list_chain_qa("qac-a").await.unwrap()[0];
        assert_eq!(row.status, ChainQaStatus::Completed);
        assert_eq!(row.answer.as_deref(), Some("the final answer"));
        assert!(row.completed_at.is_some());
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
        db.complete_chain_qa("qas-c", "done", Utc::now())
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
        let standalone = Conversation {
            id: "dangle-conv".to_string(),
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
    /// (REQ-PROJ-034, ForkProposal `transitions status`): once a proposal is
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
    /// (REQ-PROJ-037, ForkProposal `transitions status`): `promoted` is terminal,
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

    /// State-dependent field invariant (ForkProposal entity): the resolution
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

        let db = Database { pool };
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
