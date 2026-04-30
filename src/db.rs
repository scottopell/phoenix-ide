//! Database module for Phoenix IDE
//!
//! Provides persistence for conversations and messages.

mod migrations;
mod schema;

pub use migrations::run_pending_migrations;
pub use schema::*;
use schema::{
    MIGRATION_CREATE_MCP_DISABLED_SERVERS, MIGRATION_CREATE_PROJECTS,
    MIGRATION_CREATE_SHARE_TOKENS, MIGRATION_REMOVE_UNKNOWN_ERROR_KIND,
    MIGRATION_RENAME_MESSAGE_ID, MIGRATION_TYPED_STATE,
};

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use thiserror::Error;

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

/// Thread-safe database handle
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Access the underlying connection pool (for migrations and testing).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Open or create database at the given path
    pub async fn open(path: &str) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))?
            .journal_mode(SqliteJournalMode::Wal)
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
    #[allow(dead_code)] // Used in tests
    pub async fn open_in_memory() -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
            .journal_mode(SqliteJournalMode::Wal)
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

        Ok(())
    }

    // ==================== MCP Disabled Servers ====================

    /// Return the set of MCP server names that have been disabled.
    pub async fn get_disabled_mcp_servers(&self) -> DbResult<std::collections::HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT server_name FROM mcp_disabled_servers")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(name,)| name).collect())
    }

    /// Mark an MCP server as disabled (idempotent).
    pub async fn disable_mcp_server(&self, name: &str) -> DbResult<()> {
        sqlx::query("INSERT OR IGNORE INTO mcp_disabled_servers (server_name) VALUES (?1)")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Re-enable an MCP server by removing it from the disabled set.
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

        // Create new project
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES (?1, ?2, 'main', ?3)",
        )
        .bind(&id)
        .bind(canonical_path)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(Project {
            id,
            canonical_path: canonical_path.to_string(),
            main_ref: "main".to_string(),
            created_at: now,
            conversation_count: 0,
        })
    }

    /// Get a project by ID.
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

    // ==================== Conversation Operations ====================

    #[cfg(test)]
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
            &ConvMode::Explore,
            None,
            None,
            None,
        )
        .await
    }

    /// Create a new conversation, optionally associated with a project.
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
                "INSERT INTO conversations (id, slug, title, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived, model, project_id, conv_mode, desired_base_branch, seed_parent_id, seed_label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8, 0, ?9, ?10, ?11, ?12, ?13, ?14)",
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
        })
    }

    /// Get conversation by ID
    pub async fn get_conversation(&self, id: &str) -> DbResult<Conversation> {
        sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c WHERE c.id = ?1",
        )
        .bind(id)
        .try_map(parse_conversation_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => DbError::ConversationNotFound(id.to_string()),
            other => DbError::Sqlx(other),
        })
    }

    /// Get conversation by slug
    pub async fn get_conversation_by_slug(&self, slug: &str) -> DbResult<Conversation> {
        sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c WHERE c.slug = ?1",
        )
        .bind(slug)
        .try_map(parse_conversation_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => DbError::ConversationNotFound(slug.to_string()),
            other => DbError::Sqlx(other),
        })
    }

    /// List active (non-archived) user-initiated conversations
    pub async fn list_conversations(&self) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name,
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
    pub async fn list_archived_conversations(&self) -> DbResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name,
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

    /// Update conversation state
    pub async fn update_conversation_state(&self, id: &str, state: &ConvState) -> DbResult<()> {
        let now = Utc::now();
        let state_json = serde_json::to_string(state).unwrap();

        let result = sqlx::query(
            "UPDATE conversations SET state = ?1, state_updated_at = ?2, updated_at = ?2 WHERE id = ?3",
        )
        .bind(&state_json)
        .bind(now.to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Update conversation mode (e.g., Explore -> Work on task approval)
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

    /// Update conversation working directory (e.g., after worktree creation).
    pub async fn update_conversation_cwd(&self, id: &str, cwd: &str) -> DbResult<()> {
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
        // UUIDs are ASCII; a `[..8]` char boundary always aligns. We still
        // guard with `get(..8)` to keep clippy's string-slice lint happy.
        let new_id_prefix = new_id.get(..8).unwrap_or(new_id.as_str());
        // Always include the new conversation's id prefix in the slug so
        // each call is unique-by-construction. Two concurrent same-parent
        // calls can otherwise both observe the slug as available, both
        // INSERT, and one hit a UNIQUE constraint failure *before* the
        // TOCTOU `continued_in_conv_id IS NULL` UPDATE has a chance to
        // arbitrate idempotency. Including the per-call UUID prefix
        // eliminates that race entirely.
        let actual_slug = parent.slug.as_deref().map_or_else(
            || format!("continued-{new_id_prefix}"),
            |s| format!("{s}-continued-{new_id_prefix}"),
        );

        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();
        let conv_mode_json = serde_json::to_string(&parent.conv_mode).unwrap();

        let title_str = schema::title_from_slug(&actual_slug);

        // Atomic INSERT + UPDATE. On any error before `commit()`, the
        // transaction guard drops and SQLite rolls back.
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO conversations (id, slug, title, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived, model, project_id, conv_mode, desired_base_branch, seed_parent_id, seed_label, continued_in_conv_id)
             VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5, ?6, ?6, ?6, 0, ?7, ?8, ?9, ?10, ?11, ?12, NULL)",
        )
        .bind(&new_id)
        .bind(&actual_slug)
        .bind(&title_str)
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
        .execute(&mut *tx)
        .await?;

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
    #[allow(dead_code)] // Callers added in Phase 2 (chain Q&A backend)
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
    #[allow(dead_code)] // Callers added in Phase 2 (chain Q&A backend)
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

    /// Update the model for a conversation (e.g., upgrading from 200k to 1M context).
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
    pub async fn get_work_conversations(&self) -> DbResult<Vec<Conversation>> {
        sqlx::query(
            "SELECT c.id, c.slug, c.title, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    c.project_id, c.conv_mode, c.desired_base_branch,
                    c.seed_parent_id, c.seed_label, c.continued_in_conv_id, c.chain_name,
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

    /// Unarchive a conversation
    pub async fn unarchive_conversation(&self, id: &str) -> DbResult<()> {
        let now = Utc::now();

        let result =
            sqlx::query("UPDATE conversations SET archived = 0, updated_at = ?1 WHERE id = ?2")
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
    pub async fn delete_conversation(&self, id: &str) -> DbResult<()> {
        // Messages are deleted by CASCADE
        let result = sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Rename conversation (update slug)
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
             WHERE json_extract(state, '$.type') NOT IN ('idle', 'context_exhausted', 'awaiting_task_approval', 'awaiting_user_response', 'terminal')",
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
        use crate::llm::ContentBlock;

        // Skip conversations whose state is preserved across restarts; their
        // history is frozen and shouldn't be amended with synthetic results.
        let conv_rows: Vec<String> = sqlx::query(
            "SELECT id FROM conversations
             WHERE json_extract(state, '$.type') NOT IN
                 ('context_exhausted', 'terminal',
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

        Ok(Message {
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: msg_type,
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: now,
        })
    }

    /// Get messages for a conversation
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
    pub async fn get_message_by_id(&self, message_id: &str) -> DbResult<Message> {
        sqlx::query(
            "SELECT message_id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE message_id = ?1",
        )
        .bind(message_id)
        .try_map(parse_message_row)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => DbError::MessageNotFound(message_id.to_string()),
            other => DbError::Sqlx(other),
        })
    }

    /// Check if a message with the given `message_id` already exists
    /// Used for idempotent message sends - returns true if duplicate
    pub async fn message_exists(&self, message_id: &str) -> DbResult<bool> {
        let row = sqlx::query("SELECT COUNT(*) FROM messages WHERE message_id = ?1")
            .bind(message_id)
            .fetch_one(&self.pool)
            .await?;
        let count: i64 = row.get(0);
        Ok(count > 0)
    }

    /// Get the last sequence ID for a conversation
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
        Ok(())
    }

    /// Insert one row into `turn_usage` for token accounting.
    ///
    /// `root_conversation_id` is the top-level conversation that owns the work
    /// tree; for a top-level conversation it equals `conversation_id`.
    pub async fn insert_turn_usage(
        &self,
        conversation_id: &str,
        root_conversation_id: &str,
        model: &str,
        usage: &crate::llm::Usage,
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
    })
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
    async fn test_add_and_get_messages() {
        use crate::llm::ContentBlock;

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
            title: "Fix the widget".to_string(),
            priority: "p1".to_string(),
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
            title,
            priority,
            plan,
        } = conv_after.state
        {
            assert_eq!(title, "Fix the widget");
            assert_eq!(priority, "p1");
            assert_eq!(plan, "Step 1: read code\nStep 2: fix bug");
        }
    }

    #[tokio::test]
    async fn test_reset_repairs_orphaned_tool_use() {
        use crate::llm::ContentBlock;

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
                ContentBlock::tool_use("tool-123", "bash", serde_json::json!({"command": "ls"})),
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
        use crate::llm::ContentBlock;

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
                serde_json::json!({"command": "ls"}),
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
        use crate::llm::ContentBlock;

        let db = Database::open_in_memory().await.unwrap();

        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .await
            .unwrap();

        // Agent message with multiple tool_use blocks
        db.add_message(
            "msg-1",
            "conv-1",
            &MessageContent::agent(vec![
                ContentBlock::tool_use("tool-1", "bash", serde_json::json!({"command": "ls"})),
                ContentBlock::tool_use("tool-2", "bash", serde_json::json!({"command": "pwd"})),
                ContentBlock::tool_use("tool-3", "bash", serde_json::json!({"command": "date"})),
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
        use crate::llm::ContentBlock;
        use crate::state_machine::ConvState;

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
                    &format!("{id}-tool"),
                    "bash",
                    serde_json::json!({"command": "ls"}),
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

    /// Helper: create a parent conversation with the given ConvMode, force-set its
    /// state to ContextExhausted, and return the refreshed record.
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

    /// Work -> Work: worktree fields and task_id all transfer; parent's
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

    /// Branch -> Branch: branch_name/worktree_path/base_branch transfer; no task_id.
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
    /// the ConvMode variant — REQ-PROJ-028's on-first-message worktree isn't
    /// encoded in ConvMode::Explore, so this is just cwd + mode inheritance).
    #[tokio::test]
    async fn test_continue_conversation_explore_to_explore() {
        let db = Database::open_in_memory().await.unwrap();
        let parent = setup_exhausted_parent(
            &db,
            "parent-explore",
            "parent-explore",
            "/tmp/explore-cwd",
            &ConvMode::Explore,
        )
        .await;

        let outcome = db.continue_conversation("parent-explore").await.unwrap();
        let new_conv = match outcome {
            ContinueOutcome::Created(c) => c,
            other => panic!("expected Created, got {other:?}"),
        };

        assert!(matches!(new_conv.conv_mode, ConvMode::Explore));
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

    /// Parent not in ContextExhausted state: transaction does not run;
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

    /// Parent id does not exist: returns DbError::ConversationNotFound so the
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
}
