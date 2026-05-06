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
    Migration {
        version: 5,
        name: "add_chain_name_and_chain_qa",
        sql: MIGRATION_005,
    },
    Migration {
        version: 6,
        name: "archive_partially_archived_chains",
        sql: MIGRATION_006,
    },
    Migration {
        version: 7,
        name: "backfill_explore_worktree_path",
        sql: MIGRATION_007,
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

/// Phoenix Chains v1 (task 02686): chain identity/name + Q&A history.
///
/// Adds nullable `chain_name` to `conversations` (REQ-CHN-007: user-editable
/// chain name persisted on the chain root) and creates `chain_qa` for the
/// per-chain Q&A history (REQ-CHN-005). `status` is application-side enforced
/// across `in_flight | completed | failed | abandoned`; the FK cascade on
/// `root_conv_id` matches the design's hard-delete semantics. Index on
/// `(root_conv_id, created_at)` serves the per-chain history query.
const MIGRATION_005: &str = r"
ALTER TABLE conversations ADD COLUMN chain_name TEXT;

CREATE TABLE IF NOT EXISTS chain_qa (
    id TEXT PRIMARY KEY,
    root_conv_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    question TEXT NOT NULL,
    answer TEXT,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    snapshot_member_count INTEGER NOT NULL,
    snapshot_total_messages INTEGER NOT NULL,
    created_at DATETIME NOT NULL,
    completed_at DATETIME
);

CREATE INDEX IF NOT EXISTS idx_chain_qa_root ON chain_qa(root_conv_id, created_at);
";

/// Coerce legacy partially-archived chains to fully archived.
///
/// Before chain-as-unit lifecycle (PR #21), per-member archive on a chain
/// member was permitted, producing chains with mixed `archived` state — one
/// member hidden, the rest visible. After PR #21, per-member archive on chain
/// members returns 409, so a partial chain has no API path back to a coherent
/// state and the UI would render the leftover unarchived members alongside
/// the chain block (with per-row Restore that 409s). Migrate by archiving
/// the entire chain — preserves the user's "I wanted this hidden" intent and
/// gives them an Unarchive Chain action to bring it back.
const MIGRATION_006: &str = r"
WITH RECURSIVE
chain_members(root_id, member_id) AS (
    -- Roots: rows whose id is not referenced by any predecessor pointer.
    SELECT c.id, c.id
    FROM conversations c
    WHERE NOT EXISTS (
        SELECT 1 FROM conversations p WHERE p.continued_in_conv_id = c.id
    )
    UNION ALL
    SELECT cm.root_id, c.continued_in_conv_id
    FROM chain_members cm
    JOIN conversations c ON c.id = cm.member_id
    WHERE c.continued_in_conv_id IS NOT NULL
),
mixed_roots AS (
    SELECT cm.root_id
    FROM chain_members cm
    JOIN conversations c ON c.id = cm.member_id
    GROUP BY cm.root_id
    HAVING COUNT(*) >= 2
       AND SUM(CASE WHEN c.archived THEN 1 ELSE 0 END) > 0
       AND SUM(CASE WHEN c.archived THEN 1 ELSE 0 END) < COUNT(*)
)
UPDATE conversations
SET archived = 1
WHERE id IN (
    SELECT cm.member_id
    FROM chain_members cm
    WHERE cm.root_id IN (SELECT root_id FROM mixed_roots)
);
";

/// Backfill `worktree_path` onto top-level Explore conversations.
///
/// Phase 2 of task 03001 follow-up: `ConvMode::Explore` now carries an
/// optional `worktree_path: Option<NonEmptyString>`. Top-level managed
/// Explore conversations always have a worktree (the conv runs in it
/// pre-approval, REQ-PROJ-028); sub-agent Explore conversations do not.
///
/// Heuristic for "top-level managed": (a) `parent_conversation_id IS NULL`
/// (sub-agents always carry a parent pointer) AND (b) `cwd` ends with
/// `.phoenix/worktrees/{id}`, the canonical managed-worktree layout from
/// `git_ops.rs`. The cwd-suffix check is load-bearing: legacy Explore rows
/// can have `cwd = repo_root`, and migration 002 demotes invalid Work/Branch
/// rows to `Explore` while leaving their old (non-managed) cwd intact.
/// Without (b), unrelated Explore conversations sharing a cwd would key to
/// the same worktree-scoped tmux socket and tear each other down on cascade.
///
/// Without this backfill, existing top-level managed Explore conversations
/// would lose tmux session continuity on the first restart after upgrade:
/// the cwd-fallback in `terminal/ws.rs` and `api/handlers.rs` was removed
/// in the same commit, so `worktree_path()` would return `None` and the
/// session would key to a new conv-id-based socket.
const MIGRATION_007: &str = r"
UPDATE conversations
SET conv_mode = json_set(conv_mode, '$.worktree_path', cwd)
WHERE json_extract(conv_mode, '$.mode') = 'Explore'
  AND parent_conversation_id IS NULL
  AND cwd IS NOT NULL
  AND cwd != ''
  AND cwd LIKE '%/.phoenix/worktrees/' || id
  AND json_extract(conv_mode, '$.worktree_path') IS NULL;
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

    /// Create the conversations table with `conv_mode` and state columns
    /// (minimal schema needed for migration tests).
    async fn setup_conversations_table(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE conversations (\
                id TEXT PRIMARY KEY, \
                conv_mode TEXT NOT NULL DEFAULT '{\"mode\":\"Explore\"}', \
                state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', \
                cwd TEXT NOT NULL DEFAULT '/tmp', \
                parent_conversation_id TEXT, \
                user_initiated BOOLEAN NOT NULL DEFAULT 1, \
                archived BOOLEAN NOT NULL DEFAULT 0, \
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
        assert_eq!(first, 7);

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
        // Migration 002 reverts to Explore. Migration 007 only backfills
        // `worktree_path` from cwd when cwd matches the canonical managed-
        // worktree layout `.phoenix/worktrees/{id}`; cwd `/tmp` does not
        // match, so the row stays as bare `{"mode":"Explore"}`. This is
        // load-bearing: backfilling `/tmp` here would let two demoted convs
        // share the same worktree-scoped tmux socket on cascade.
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
        // 002 reverts to Explore; 007 leaves it alone (cwd `/tmp` is not
        // the canonical managed-worktree layout `.phoenix/worktrees/{id}`).
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

    /// Migration 005 (task 02686, REQ-CHN-005/007): adds `chain_name` to
    /// `conversations` and creates the `chain_qa` table with its index.
    /// Existing rows backfill `chain_name` to NULL.
    #[tokio::test]
    async fn migration_005_adds_chain_name_and_chain_qa() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

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
            columns.iter().any(|c| c == "chain_name"),
            "Expected chain_name column after migration 005, got: {columns:?}"
        );

        let row = sqlx::query("SELECT chain_name FROM conversations WHERE id = 'c-pre'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let chain_name: Option<String> = row.get("chain_name");
        assert!(
            chain_name.is_none(),
            "Existing rows should backfill chain_name to NULL, got: {chain_name:?}"
        );

        // chain_qa table exists with the expected columns
        let qa_columns: Vec<String> = sqlx::query("PRAGMA table_info(chain_qa)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        for expected in [
            "id",
            "root_conv_id",
            "question",
            "answer",
            "model",
            "status",
            "snapshot_member_count",
            "snapshot_total_messages",
            "created_at",
            "completed_at",
        ] {
            assert!(
                qa_columns.iter().any(|c| c == expected),
                "Expected chain_qa column {expected:?}, got: {qa_columns:?}"
            );
        }

        // Index on (root_conv_id, created_at) exists
        let indexes: Vec<String> = sqlx::query("PRAGMA index_list(chain_qa)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(
            indexes.iter().any(|n| n == "idx_chain_qa_root"),
            "Expected idx_chain_qa_root index, got: {indexes:?}"
        );
    }

    /// Migration 006: a chain with mixed `archived` state has every member
    /// flipped to archived; fully-archived and fully-unarchived chains are
    /// untouched; standalones are untouched.
    #[tokio::test]
    async fn migration_006_archives_partially_archived_chains() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Chain A: 3 members, only mid is archived → migration archives all 3.
        // Chain B: 2 members, both archived already → unchanged.
        // Chain C: 2 members, neither archived → unchanged.
        // Standalone S: archived in isolation → unchanged.
        for (id, archived) in [
            ("a-root", 0),
            ("a-mid", 1),
            ("a-leaf", 0),
            ("b-root", 1),
            ("b-leaf", 1),
            ("c-root", 0),
            ("c-leaf", 0),
            ("solo-s", 1),
        ] {
            sqlx::query(
                "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, \
                 archived, state_updated_at, created_at, updated_at) \
                 VALUES (?1, '{\"mode\":\"Explore\"}', '{\"type\":\"idle\"}', \
                 '/tmp', 1, ?2, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(archived)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_pending_migrations(&pool).await.unwrap();

        // Wire chain edges *after* migrations so 003 (the column) is in place.
        for (parent, child) in [
            ("a-root", "a-mid"),
            ("a-mid", "a-leaf"),
            ("b-root", "b-leaf"),
            ("c-root", "c-leaf"),
        ] {
            sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
                .bind(child)
                .bind(parent)
                .execute(&pool)
                .await
                .unwrap();
        }

        // Re-run the partial-archive cleanup directly so we exercise it on
        // the now-wired chain (the migration table thinks 006 is done).
        sqlx::raw_sql(MIGRATION_006).execute(&pool).await.unwrap();

        let archived_for = |id: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query("SELECT archived FROM conversations WHERE id = ?1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .get::<bool, _>("archived")
            }
        };

        // Chain A: every member ends archived.
        assert!(archived_for("a-root").await);
        assert!(archived_for("a-mid").await);
        assert!(archived_for("a-leaf").await);
        // Chain B: untouched (already fully archived).
        assert!(archived_for("b-root").await);
        assert!(archived_for("b-leaf").await);
        // Chain C: untouched (none archived).
        assert!(!archived_for("c-root").await);
        assert!(!archived_for("c-leaf").await);
        // Standalone: untouched.
        assert!(archived_for("solo-s").await);
    }

    /// Migration 007: top-level Explore conversations get `worktree_path`
    /// backfilled from `cwd`; sub-agents and non-Explore rows are untouched.
    #[tokio::test]
    async fn migration_007_backfills_explore_worktree_path() {
        let pool = test_pool().await;
        // Need parent_conversation_id column for this migration.
        sqlx::raw_sql(
            "CREATE TABLE conversations (\
                id TEXT PRIMARY KEY, \
                conv_mode TEXT NOT NULL, \
                state TEXT NOT NULL DEFAULT '{\"type\":\"idle\"}', \
                cwd TEXT NOT NULL DEFAULT '/tmp', \
                parent_conversation_id TEXT, \
                user_initiated BOOLEAN NOT NULL DEFAULT 1, \
                archived BOOLEAN NOT NULL DEFAULT 0, \
                state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                created_at TEXT NOT NULL DEFAULT '2025-01-01', \
                updated_at TEXT NOT NULL DEFAULT '2025-01-01'\
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // (id, conv_mode, cwd, parent_conv_id) seed rows covering each case.
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            // 1. Top-level managed Explore (cwd matches `.phoenix/worktrees/{id}`)
            //    — backfilled.
            (
                "top-explore",
                r#"{"mode":"Explore"}"#,
                "/repo/.phoenix/worktrees/top-explore",
                None,
            ),
            // 2. Sub-agent Explore (parent set) — left alone even if cwd matches
            //    a managed-worktree-shaped path.
            (
                "sub-explore",
                r#"{"mode":"Explore"}"#,
                "/repo/.phoenix/worktrees/top-explore",
                Some("top-explore"),
            ),
            // 3. Top-level Explore with empty cwd — not backfilled.
            ("empty-cwd", r#"{"mode":"Explore"}"#, "", None),
            // 4. Direct mode — untouched (mode != Explore).
            ("direct", r#"{"mode":"Direct"}"#, "/anywhere", None),
            // 5. Already-backfilled Explore — idempotent (worktree_path stays).
            (
                "already",
                r#"{"mode":"Explore","worktree_path":"/preexisting"}"#,
                "/repo/.phoenix/worktrees/already",
                None,
            ),
            // 6. Top-level Explore with a non-managed cwd (legacy pre-REQ-PROJ-028
            //    row, or a row demoted by migration 002 with its old cwd intact)
            //    — NOT backfilled. If we backfilled, two unrelated Explore convs
            //    sharing this cwd would collide on the same tmux socket.
            ("legacy-repo-root", r#"{"mode":"Explore"}"#, "/repo", None),
            // 7. Top-level Explore whose cwd points at *another* conv's managed
            //    worktree (pathological). The id-suffix predicate rejects this:
            //    `/repo/.phoenix/worktrees/top-explore` does not end with this
            //    row's id (`other-conv`).
            (
                "other-conv",
                r#"{"mode":"Explore"}"#,
                "/repo/.phoenix/worktrees/top-explore",
                None,
            ),
        ];

        for (id, conv_mode, cwd, parent) in rows {
            sqlx::query(
                "INSERT INTO conversations (id, conv_mode, cwd, parent_conversation_id) \
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(id)
            .bind(conv_mode)
            .bind(cwd)
            .bind(*parent)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::raw_sql(MIGRATION_007).execute(&pool).await.unwrap();

        let mode_for = |id: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query("SELECT conv_mode FROM conversations WHERE id = ?1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .get::<String, _>("conv_mode")
            }
        };

        // 1. Top-level managed Explore: worktree_path backfilled to cwd.
        let top: serde_json::Value = serde_json::from_str(&mode_for("top-explore").await).unwrap();
        assert_eq!(top["mode"], "Explore");
        assert_eq!(top["worktree_path"], "/repo/.phoenix/worktrees/top-explore");

        // 2. Sub-agent Explore: untouched (no worktree_path field).
        let sub: serde_json::Value = serde_json::from_str(&mode_for("sub-explore").await).unwrap();
        assert_eq!(sub["mode"], "Explore");
        assert!(
            sub.get("worktree_path").is_none(),
            "sub-agent Explore must not get a worktree_path: {sub:?}"
        );

        // 3. Empty cwd: not backfilled (would deserialise as empty NonEmptyString).
        let empty: serde_json::Value = serde_json::from_str(&mode_for("empty-cwd").await).unwrap();
        assert_eq!(empty["mode"], "Explore");
        assert!(empty.get("worktree_path").is_none());

        // 4. Direct: completely untouched.
        let direct: serde_json::Value = serde_json::from_str(&mode_for("direct").await).unwrap();
        assert_eq!(direct["mode"], "Direct");
        assert!(direct.get("worktree_path").is_none());

        // 5. Pre-existing worktree_path: untouched (idempotent).
        let pre: serde_json::Value = serde_json::from_str(&mode_for("already").await).unwrap();
        assert_eq!(pre["worktree_path"], "/preexisting");

        // 6. Legacy non-managed cwd (e.g. repo root): NOT backfilled. Backfilling
        //    would let two such rows collide on the same tmux socket.
        let legacy: serde_json::Value =
            serde_json::from_str(&mode_for("legacy-repo-root").await).unwrap();
        assert_eq!(legacy["mode"], "Explore");
        assert!(
            legacy.get("worktree_path").is_none(),
            "non-managed cwd must not be backfilled: {legacy:?}"
        );

        // 7. Cwd matches some other conv's managed-worktree path: NOT backfilled
        //    (id-suffix predicate guards against cross-conversation collisions).
        let other: serde_json::Value = serde_json::from_str(&mode_for("other-conv").await).unwrap();
        assert_eq!(other["mode"], "Explore");
        assert!(
            other.get("worktree_path").is_none(),
            "cwd pointing at another conv's worktree must not be backfilled: {other:?}"
        );

        // Idempotency: re-run migration, no changes.
        sqlx::raw_sql(MIGRATION_007).execute(&pool).await.unwrap();
        let top2: serde_json::Value = serde_json::from_str(&mode_for("top-explore").await).unwrap();
        assert_eq!(
            top2["worktree_path"],
            "/repo/.phoenix/worktrees/top-explore"
        );
    }
}
