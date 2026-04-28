//! Sequential database migrations.
//!
//! Each migration runs exactly once, tracked by the `_migrations` table.
//! Migrations run at startup before any conversation is loaded.

use sqlx::SqlitePool;

use super::DbResult;

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "rewrite_standalone_to_direct",
        sql: MIGRATION_001,
    },
    Migration {
        version: 2,
        name: "backfill_empty_convmode_fields",
        sql: MIGRATION_002,
    },
    Migration {
        version: 3,
        name: "add_continued_in_conv_id_column",
        sql: MIGRATION_003,
    },
    Migration {
        version: 4,
        name: "create_turn_usage_table",
        sql: MIGRATION_004,
    },
];

/// Rewrite the "Standalone" serde discriminator to "Direct" in `conv_mode` JSON,
/// closing the divergence between SQL `json_extract` queries and Rust deserialization.
const MIGRATION_001: &str = r#"
UPDATE conversations
SET conv_mode = REPLACE(conv_mode, '"Standalone"', '"Direct"')
WHERE json_extract(conv_mode, '$.mode') = 'Standalone';
"#;

/// Revert Work/Branch conversations with empty critical fields to Explore/Direct,
/// and clean up `__LEGACY_EMPTY__` sentinels from the `NonEmptyString` default shim.
const MIGRATION_002: &str = r#"
-- Revert Work conversations with empty critical fields to Explore
UPDATE conversations
SET conv_mode = '{"mode":"Explore"}',
    state = '{"type":"idle"}'
WHERE json_extract(conv_mode, '$.mode') = 'Work'
  AND (
    json_extract(conv_mode, '$.worktree_path') = ''
    OR json_extract(conv_mode, '$.worktree_path') IS NULL
    OR json_extract(conv_mode, '$.base_branch') = ''
    OR json_extract(conv_mode, '$.base_branch') IS NULL
    OR json_extract(conv_mode, '$.branch_name') = ''
    OR json_extract(conv_mode, '$.branch_name') IS NULL
  );

-- Same for Branch conversations
UPDATE conversations
SET conv_mode = '{"mode":"Direct"}',
    state = '{"type":"idle"}'
WHERE json_extract(conv_mode, '$.mode') = 'Branch'
  AND (
    json_extract(conv_mode, '$.worktree_path') = ''
    OR json_extract(conv_mode, '$.worktree_path') IS NULL
    OR json_extract(conv_mode, '$.base_branch') = ''
    OR json_extract(conv_mode, '$.base_branch') IS NULL
    OR json_extract(conv_mode, '$.branch_name') = ''
    OR json_extract(conv_mode, '$.branch_name') IS NULL
  );

-- Rewrite __LEGACY_EMPTY__ sentinels (from A1's NonEmptyString default)
UPDATE conversations
SET conv_mode = '{"mode":"Explore"}',
    state = '{"type":"idle"}'
WHERE conv_mode LIKE '%__LEGACY_EMPTY__%';
"#;

/// Create the `turn_usage` table for per-LLM-turn token tracking.
const MIGRATION_004: &str = r"
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
";

/// Add the `continued_in_conv_id` column to `conversations` (REQ-BED-030).
///
/// Phase 1 of task 24696: data-foundation for Context Continuation Worktree
/// Transfer. A nullable self-referential foreign key; existing rows default to
/// NULL (`SQLite`'s default for a nullable ADD COLUMN). The column is unused at
/// runtime in Phase 1 — later phases wire it into the continuation handoff.
const MIGRATION_003: &str = r"
ALTER TABLE conversations ADD COLUMN continued_in_conv_id TEXT REFERENCES conversations(id);
";

/// Run all pending migrations against the database.
///
/// Returns the number of migrations applied.
pub async fn run_pending_migrations(pool: &SqlitePool) -> DbResult<u32> {
    // Ensure the tracking table exists
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _migrations (\
            version INTEGER PRIMARY KEY, \
            name TEXT NOT NULL, \
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))\
        )",
    )
    .execute(pool)
    .await?;

    // Find the highest version already applied
    let current_version: u32 =
        sqlx::query_scalar::<_, Option<u32>>("SELECT MAX(version) FROM _migrations")
            .fetch_one(pool)
            .await?
            .unwrap_or(0);

    let mut applied = 0u32;

    for migration in MIGRATIONS {
        if migration.version <= current_version {
            continue;
        }

        tracing::info!(
            version = migration.version,
            name = migration.name,
            "Applying database migration"
        );

        sqlx::raw_sql(migration.sql).execute(pool).await?;

        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?, ?)")
            .bind(migration.version)
            .bind(migration.name)
            .execute(pool)
            .await?;

        applied += 1;
    }

    if applied > 0 {
        tracing::info!(applied, "Database migrations complete");
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::Row;
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap()
    }

    /// Create the conversations table with conv_mode and state columns
    /// (minimal schema needed for migration tests).
    async fn setup_conversations_table(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE conversations (\
                id TEXT PRIMARY KEY, \
                conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', \
                state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', \
                cwd TEXT NOT NULL DEFAULT '/tmp', \
                user_initiated BOOLEAN NOT NULL DEFAULT 1, \
                state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                created_at TEXT NOT NULL DEFAULT '2025-01-01', \
                updated_at TEXT NOT NULL DEFAULT '2025-01-01'\
            )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        let first = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(first, 4);

        let second = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(second, 0);
    }

    #[tokio::test]
    async fn migration_001_rewrites_standalone_to_direct() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Insert a row with "Standalone" mode
        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c1', '{\"mode\":\"Standalone\"}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        let row = sqlx::query("SELECT conv_mode FROM conversations WHERE id = 'c1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get("conv_mode");
        assert!(mode.contains("\"Direct\""), "Expected Direct, got: {mode}");
        assert!(
            !mode.contains("Standalone"),
            "Standalone should be gone: {mode}"
        );
    }

    #[tokio::test]
    async fn migration_002_reverts_work_with_empty_fields() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Work with empty worktree_path
        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c2', '{\"mode\":\"Work\",\"branch_name\":\"b\",\"worktree_path\":\"\",\"base_branch\":\"main\",\"task_id\":\"T1\",\"task_title\":\"t\"}', \
             '{\"type\":\"tool_executing\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        let row = sqlx::query("SELECT conv_mode, state FROM conversations WHERE id = 'c2'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get("conv_mode");
        let state: String = row.get("state");
        assert_eq!(mode, "{\"mode\":\"Explore\"}");
        assert_eq!(state, "{\"type\":\"idle\"}");
    }

    #[tokio::test]
    async fn migration_002_reverts_branch_with_empty_fields() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Branch with empty base_branch
        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c3', '{\"mode\":\"Branch\",\"branch_name\":\"b\",\"worktree_path\":\"/wt\",\"base_branch\":\"\"}', \
             '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        let row = sqlx::query("SELECT conv_mode FROM conversations WHERE id = 'c3'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get("conv_mode");
        assert_eq!(mode, "{\"mode\":\"Direct\"}");
    }

    #[tokio::test]
    async fn migration_002_cleans_legacy_empty_sentinels() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c4', '{\"mode\":\"Work\",\"branch_name\":\"__LEGACY_EMPTY__\",\"worktree_path\":\"/wt\",\"base_branch\":\"main\",\"task_id\":\"T1\",\"task_title\":\"t\"}', \
             '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        let row = sqlx::query("SELECT conv_mode FROM conversations WHERE id = 'c4'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get("conv_mode");
        assert_eq!(mode, "{\"mode\":\"Explore\"}");
    }

    #[tokio::test]
    async fn valid_work_conversation_is_not_reverted() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c5', '{\"mode\":\"Work\",\"branch_name\":\"b\",\"worktree_path\":\"/wt\",\"base_branch\":\"main\",\"task_id\":\"T1\",\"task_title\":\"Fix it\"}', \
             '{\"type\":\"tool_executing\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        let row = sqlx::query("SELECT conv_mode, state FROM conversations WHERE id = 'c5'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get("conv_mode");
        let state: String = row.get("state");
        assert!(
            mode.contains("\"Work\""),
            "Valid Work should be preserved: {mode}"
        );
        assert!(
            state.contains("tool_executing"),
            "State should be preserved: {state}"
        );
    }

    /// Migration 003 (REQ-BED-030): adds a nullable `continued_in_conv_id`
    /// column on `conversations`. Existing rows default to NULL and the column
    /// should be queryable via `PRAGMA table_info` after migration.
    #[tokio::test]
    async fn migration_003_adds_continued_in_conv_id_column() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Seed a row before the migration so we can assert the backfill is NULL.
        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c-pre', '{\"mode\":\"Explore\"}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(
            columns.iter().any(|c| c == "continued_in_conv_id"),
            "Expected continued_in_conv_id column after migration 003, got: {columns:?}"
        );

        let row = sqlx::query("SELECT continued_in_conv_id FROM conversations WHERE id = 'c-pre'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let continued: Option<String> = row.get("continued_in_conv_id");
        assert!(
            continued.is_none(),
            "Existing rows should backfill NULL, got: {continued:?}"
        );
    }
}
