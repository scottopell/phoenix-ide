//! Sequential database migrations.
//!
//! Each migration runs exactly once, tracked by the `_migrations` table.
//! Migrations run at startup before any conversation is loaded.

use std::collections::HashSet;

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
    Migration {
        version: 8,
        name: "create_notification_settings",
        sql: MIGRATION_008,
    },
    Migration {
        version: 9,
        name: "rewrite_one_million_context_model_ids_to_base",
        sql: MIGRATION_009,
    },
    Migration {
        version: 10,
        name: "rewrite_opus_4_5_to_4_6",
        sql: MIGRATION_010,
    },
    Migration {
        version: 11,
        name: "add_llm_language_and_app_settings",
        sql: MIGRATION_011,
    },
    Migration {
        version: 12,
        name: "create_work_scope_pr_associations",
        sql: MIGRATION_012,
    },
    Migration {
        version: 13,
        name: "create_pr_feedback_baselines",
        sql: MIGRATION_013,
    },
    Migration {
        version: 14,
        name: "backfill_user_content_files",
        sql: MIGRATION_014,
    },
    Migration {
        version: 15,
        name: "create_sub_agent_personas",
        sql: MIGRATION_015,
    },
    Migration {
        version: 16,
        name: "create_fork_proposals",
        sql: MIGRATION_016,
    },
    Migration {
        version: 17,
        name: "add_spawned_from_conversation_id",
        sql: MIGRATION_017,
    },
    Migration {
        version: 18,
        name: "create_message_fts",
        sql: MIGRATION_018,
    },
    Migration {
        version: 19,
        name: "rename_chain_qa_snapshot_columns",
        sql: MIGRATION_019,
    },
    Migration {
        version: 20,
        name: "backfill_awaiting_recovery_resume_target",
        sql: MIGRATION_020,
    },
    Migration {
        version: 21,
        name: "normalize_explore_taskmd_id_hint",
        sql: MIGRATION_021,
    },
    Migration {
        version: 22,
        name: "create_mcp_oauth_tables",
        sql: MIGRATION_022,
    },
    Migration {
        version: 23,
        name: "add_mcp_oauth_registration_redirect_uri",
        sql: MIGRATION_023,
    },
    Migration {
        version: 24,
        name: "create_auth_sessions",
        sql: MIGRATION_024,
    },
    Migration {
        version: 25,
        name: "create_message_attachment_tables",
        sql: MIGRATION_025,
    },
    Migration {
        version: 26,
        name: "strip_message_content_attachments",
        sql: MIGRATION_026,
    },
    Migration {
        version: 27,
        name: "create_steering_message_tables",
        sql: MIGRATION_027,
    },
    Migration {
        version: 28,
        name: "add_conv_mode_columns",
        sql: MIGRATION_028,
    },
    Migration {
        version: 29,
        name: "drop_conv_mode_blob",
        sql: MIGRATION_029,
    },
    Migration {
        version: 30,
        name: "add_clear_watermark_to_conversations",
        sql: MIGRATION_030,
    },
    Migration {
        version: 31,
        name: "add_turn_usage_first_byte_at",
        sql: MIGRATION_031,
    },
    Migration {
        version: 32,
        name: "add_pr_feedback_status_cache",
        sql: MIGRATION_032,
    },
    Migration {
        version: 33,
        name: "add_transcript_generation_to_conversations",
        sql: MIGRATION_033,
    },
    Migration {
        version: 34,
        name: "create_global_recall_sessions",
        sql: MIGRATION_034,
    },
    Migration {
        version: 35,
        name: "create_conversation_creation_jobs",
        sql: MIGRATION_035,
    },
    Migration {
        version: 36,
        name: "create_conversation_creation_job_files",
        sql: MIGRATION_036,
    },
    Migration {
        version: 37,
        name: "create_conversation_creation_job_images",
        sql: MIGRATION_037,
    },
    Migration {
        version: 38,
        name: "add_fenced_creation_protocol",
        sql: MIGRATION_038,
    },
    Migration {
        version: 39,
        name: "add_creation_resource_reservations",
        sql: MIGRATION_039,
    },
    Migration {
        version: 40,
        name: "add_creation_cleanup_claims",
        sql: MIGRATION_040,
    },
    Migration {
        version: 41,
        name: "create_work_scope_observed_branches",
        sql: MIGRATION_041,
    },
    Migration {
        version: 42,
        name: "create_work_scope_active_pr_selection",
        sql: MIGRATION_042,
    },
    Migration {
        version: 43,
        name: "normalize_pr_feedback_baselines_by_full_identity",
        sql: MIGRATION_043,
    },
    Migration {
        version: 44,
        name: "create_workflow_protocol_registry_tables",
        sql: MIGRATION_044,
    },
    Migration {
        version: 45,
        name: "create_durable_workflow_core_tables",
        sql: MIGRATION_045,
    },
    Migration {
        version: 46,
        name: "create_wake_profile_tables",
        sql: MIGRATION_046,
    },
    Migration {
        version: 47,
        name: "create_creation_shadow_tables",
        sql: MIGRATION_047,
    },
    Migration {
        version: 48,
        name: "fence_and_archive_creation_shadow_projections",
        sql: MIGRATION_048,
    },
    Migration {
        version: 49,
        name: "preserve_creation_shadow_acceptance_evidence",
        sql: MIGRATION_049,
    },
    Migration {
        version: 50,
        name: "fence_creation_shadow_reservation_updates",
        sql: MIGRATION_050,
    },
    Migration {
        version: 51,
        name: "align_creation_shadow_divergence_actions",
        sql: MIGRATION_051,
    },
    Migration {
        version: 52,
        name: "persist_creation_shadow_execution_mode",
        sql: MIGRATION_052,
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

const MIGRATION_008: &str = r"
CREATE TABLE IF NOT EXISTS notification_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Rewrite `claude-*-1m` model ids back to their base ids across persisted
/// state. As of Anthropic's 2026-03-13 GA announcement, the 1M context
/// window is native on Opus 4.6, Sonnet 4.6, and Opus 4.7 at standard
/// pricing — no beta header, no separate `api_name`. The `-1m` variants in
/// `all_models()` are being removed in the same commit; without this
/// migration, conversations and chain Q&A rows that pinned a `-1m` id
/// would fail to resolve a service after upgrade. Rewriting is loss-less:
/// the old `-1m` rows shared `api_name` and pricing with their base.
const MIGRATION_009: &str = r"
UPDATE conversations SET model = 'claude-opus-4-7'   WHERE model = 'claude-opus-4-7-1m';
UPDATE conversations SET model = 'claude-opus-4-6'   WHERE model = 'claude-opus-4-6-1m';
UPDATE conversations SET model = 'claude-sonnet-4-6' WHERE model = 'claude-sonnet-4-6-1m';

UPDATE turn_usage SET model = 'claude-opus-4-7'   WHERE model = 'claude-opus-4-7-1m';
UPDATE turn_usage SET model = 'claude-opus-4-6'   WHERE model = 'claude-opus-4-6-1m';
UPDATE turn_usage SET model = 'claude-sonnet-4-6' WHERE model = 'claude-sonnet-4-6-1m';

UPDATE chain_qa SET model = 'claude-opus-4-7'   WHERE model = 'claude-opus-4-7-1m';
UPDATE chain_qa SET model = 'claude-opus-4-6'   WHERE model = 'claude-opus-4-6-1m';
UPDATE chain_qa SET model = 'claude-sonnet-4-6' WHERE model = 'claude-sonnet-4-6-1m';
";

/// Rewrite `claude-opus-4-5` to `claude-opus-4-6` across persisted state.
/// Opus 4.5's spec is removed in the same commit; without this migration,
/// any `conversations`, `turn_usage`, or `chain_qa` row pinned to 4-5 would
/// fail `ModelRegistry::get()` after upgrade. 4-6 is the nearest still-
/// supported Opus, preserving the user's "I picked Opus" intent. Mirrors
/// the migration-009 pattern.
const MIGRATION_010: &str = r"
UPDATE conversations SET model = 'claude-opus-4-6' WHERE model = 'claude-opus-4-5';
UPDATE turn_usage   SET model = 'claude-opus-4-6' WHERE model = 'claude-opus-4-5';
UPDATE chain_qa     SET model = 'claude-opus-4-6' WHERE model = 'claude-opus-4-5';
";

/// Add per-conversation `llm_language` column and a generic `app_settings`
/// key/value table for global preferences. `llm_language` defaults to
/// `phoenix-native` for both fresh installs and backfilled old rows.
const MIGRATION_011: &str = r"
ALTER TABLE conversations
    ADD COLUMN llm_language TEXT NOT NULL DEFAULT 'phoenix-native';

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

const MIGRATION_012: &str = r"
CREATE TABLE IF NOT EXISTS work_scopes (
    id INTEGER PRIMARY KEY,
    scope_type TEXT NOT NULL,
    scope_value TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(scope_type, scope_value)
);

CREATE TABLE IF NOT EXISTS work_scope_pr_associations (
    work_scope_id INTEGER NOT NULL REFERENCES work_scopes(id) ON DELETE CASCADE,
    repo_owner TEXT NOT NULL,
    repo_name TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    state TEXT NOT NULL,
    draft INTEGER NOT NULL,
    display_state TEXT NOT NULL,
    base TEXT NOT NULL,
    head TEXT NOT NULL,
    github_updated_at TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (work_scope_id, repo_owner, repo_name, pr_number)
);

CREATE INDEX IF NOT EXISTS idx_work_scope_pr_primary
ON work_scope_pr_associations(work_scope_id, display_state, github_updated_at, last_seen_at);
";

const MIGRATION_013: &str = r"
CREATE TABLE IF NOT EXISTS work_scope_pr_feedback_baselines (
    work_scope_id INTEGER NOT NULL REFERENCES work_scopes(id) ON DELETE CASCADE,
    pr_number INTEGER NOT NULL,
    captured_at TEXT NOT NULL,
    github_updated_at TEXT,
    feedback_identities TEXT NOT NULL,
    feedback_fingerprints TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (work_scope_id, pr_number)
);
";

const MIGRATION_041: &str = r"
CREATE TABLE IF NOT EXISTS work_scope_observed_branches (
    work_scope_id INTEGER NOT NULL REFERENCES work_scopes(id) ON DELETE CASCADE,
    repository_identity TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    first_observed_head_oid TEXT NOT NULL,
    last_observed_head_oid TEXT NOT NULL,
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    PRIMARY KEY (work_scope_id, repository_identity, branch_name)
);

CREATE INDEX IF NOT EXISTS idx_work_scope_observed_branches_last_seen
ON work_scope_observed_branches(work_scope_id, last_observed_at);
";
const MIGRATION_042: &str = r"
CREATE TABLE IF NOT EXISTS work_scope_active_pr_selection (
    work_scope_id INTEGER PRIMARY KEY REFERENCES work_scopes(id) ON DELETE CASCADE,
    repo_owner TEXT,
    repo_name TEXT,
    pr_number INTEGER,
    provenance TEXT NOT NULL,
    latest_observed_repository_identity TEXT,
    latest_observed_branch_name TEXT,
    inference_generation INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    CHECK (
        (repo_owner IS NULL AND repo_name IS NULL AND pr_number IS NULL)
        OR (repo_owner IS NOT NULL AND repo_name IS NOT NULL AND pr_number IS NOT NULL)
    )
);
";

const MIGRATION_043: &str = r"
ALTER TABLE work_scope_pr_feedback_baselines RENAME TO work_scope_pr_feedback_baselines_old;

CREATE TABLE work_scope_pr_feedback_baselines (
    work_scope_id INTEGER NOT NULL REFERENCES work_scopes(id) ON DELETE CASCADE,
    repo_owner TEXT NOT NULL,
    repo_name TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    captured_at TEXT NOT NULL,
    github_updated_at TEXT,
    feedback_identities TEXT NOT NULL DEFAULT '[]',
    feedback_fingerprints TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (work_scope_id, repo_owner, repo_name, pr_number)
);

INSERT INTO work_scope_pr_feedback_baselines (
    work_scope_id, repo_owner, repo_name, pr_number, captured_at, github_updated_at,
    feedback_identities, feedback_fingerprints
)
SELECT b.work_scope_id, a.repo_owner, a.repo_name, b.pr_number, b.captured_at, b.github_updated_at,
       b.feedback_identities, b.feedback_fingerprints
FROM work_scope_pr_feedback_baselines_old b
JOIN work_scope_pr_associations a
  ON a.work_scope_id = b.work_scope_id AND a.pr_number = b.pr_number
WHERE 1 = (
    SELECT COUNT(*)
    FROM work_scope_pr_associations candidate
    WHERE candidate.work_scope_id = b.work_scope_id
      AND candidate.pr_number = b.pr_number
);

DROP TABLE work_scope_pr_feedback_baselines_old;
";

const MIGRATION_014: &str = r"
UPDATE messages
SET content = json_set(content, '$.files', json('[]'))
WHERE message_type IN ('user', 'skill')
  AND json_type(content, '$.files') IS NULL;
";

/// Persist named-agent personas for sub-agent conversations so a sub-agent
/// runtime recreated mid-run (e.g. model-upgrade eviction) keeps its persona
/// instead of falling back to the generic sub-agent prompt (REQ-AG-006).
/// Sub-agent-only metadata, kept in its own cascade-deleted table rather than
/// widening the `conversations` row that the vast majority of conversations
/// would leave NULL.
const MIGRATION_015: &str = r"
CREATE TABLE IF NOT EXISTS sub_agent_personas (
    conversation_id TEXT PRIMARY KEY
        REFERENCES conversations(id) ON DELETE CASCADE,
    persona TEXT NOT NULL
);
";

/// Decoupled task fork proposals (REQ-PROJ-033/034/035/037).
///
/// `status` is application-enforced across `pending | spawned | dismissed |
/// promoted`. The `origin_conv_id` FK cascade-deletes proposals with their
/// originating conversation (proposals are bound to origin — REQ-PROJ-035).
/// `fork_conv_id` and `refinement_conv_id` carry NO foreign key: the spawned
/// fork / promoted refinement has an independent lifecycle and may be
/// hard-deleted while this origin-bound proposal lives, so the audit link is a
/// raw id that may dangle (no cascade, no null-on-delete). Index on
/// `(origin_conv_id, status)` serves the per-origin pending lookup.
const MIGRATION_016: &str = r"
CREATE TABLE IF NOT EXISTS fork_proposals (
    id TEXT PRIMARY KEY,
    origin_conv_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    task_file TEXT NOT NULL,
    title TEXT NOT NULL,
    priority TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL,
    fork_conv_id TEXT,
    refinement_conv_id TEXT,
    created_at DATETIME NOT NULL,
    resolved_at DATETIME
);

CREATE INDEX IF NOT EXISTS idx_fork_proposals_origin ON fork_proposals(origin_conv_id, status);
";

/// Add the `spawned_from_conversation_id` column to `conversations`
/// (REQ-PROJ-035). A nullable, non-FK provenance breadcrumb pointing at the
/// conversation that proposed a fork; existing rows default to NULL. No
/// `REFERENCES` clause — the breadcrumb may dangle (the origin is decoupled and
/// may be hard-deleted), so there is no cascade and no null-on-delete.
const MIGRATION_017: &str = r"
ALTER TABLE conversations ADD COLUMN spawned_from_conversation_id TEXT;
";

/// Conversation-retrieval index (`specs/conversation-retrieval/`).
///
/// A standalone FTS5 table over extracted message prose — *not* an
/// external-content table over `messages`, because the indexed text is a
/// Rust-side extraction of typed `MessageContent`, not the raw JSON column.
/// `text` is the only tokenized column; the rest are `UNINDEXED` provenance /
/// change-detection columns available to filtering and projection without
/// participating in the match. The migration creates the empty structure; the
/// typed backfill from existing `messages` is performed by the Rust startup
/// reconciliation (`Fts5Retriever::reconcile`), which static SQL cannot do.
const MIGRATION_018: &str = r"
CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    text,
    message_id      UNINDEXED,
    chunk_ordinal   UNINDEXED,
    conversation_id UNINDEXED,
    message_type    UNINDEXED,
    created_at      UNINDEXED,
    content_hash    UNINDEXED
);
";

/// Rename the `chain_qa` chain-size markers to age-of-answer names.
///
/// The columns record the chain's size *at answer time* so the UI can show an
/// age-of-answer freshness tag (`specs/chains/` REQ-CHN-005). The original
/// `snapshot_*` names implied the answer was computed against a frozen snapshot,
/// which contradicts the read-only agentic loop's live-content contract
/// (REQ-CHN-009): there is no during-answer snapshot. The names now match what
/// the integers mean.
const MIGRATION_019: &str = r"
ALTER TABLE chain_qa RENAME COLUMN snapshot_member_count TO chain_members_at_answer;
ALTER TABLE chain_qa RENAME COLUMN snapshot_total_messages TO chain_messages_at_answer;
";

/// Backfill typed recovery resume targets for rows created before `AwaitingRecovery`
/// carried the suspended LLM operation explicitly. Existing recovery rows could
/// only represent ordinary conversation turns, so they resume `RequestLlm`.
const MIGRATION_020: &str = r#"
UPDATE conversations
SET state = json_set(state, '$.resume', json('{"type":"conversation_turn"}'))
WHERE json_extract(state, '$.type') = 'awaiting_recovery'
  AND json_extract(state, '$.resume') IS NULL;
"#;

/// Normalize persisted Explore `conv_mode` JSON for the taskmd ID hint field.
const MIGRATION_021: &str = r"
UPDATE conversations
SET conv_mode = json_remove(conv_mode, '$.next_taskmd_id_hint')
WHERE json_extract(conv_mode, '$.mode') = 'Explore'
  AND json_extract(conv_mode, '$.next_taskmd_id_hint') IS NULL;
";

/// MCP OAuth 2.1 persistence (`specs/mcp/` REQ-MCP-010, REQ-MCP-012).
///
/// `mcp_oauth_registrations` holds one client identity per **authorization
/// server** (not per MCP server), so resources sharing an authorization server
/// share a registration. `mcp_oauth_tokens` holds at most one token per MCP
/// server (`OneTokenPerServer` in `specs/mcp/mcp.allium`), audience-bound to
/// `resource_uri`; `scopes` is the space-separated granted scope set, kept so
/// a post-restart `insufficient_scope` step-up can request the union of prior
/// and challenged scopes. Tokens are plaintext by design — the database file's
/// on-disk protection is the trust boundary.
const MIGRATION_022: &str = r"
CREATE TABLE IF NOT EXISTS mcp_oauth_registrations (
    auth_server TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    client_secret TEXT,
    token_endpoint_auth_method TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS mcp_oauth_tokens (
    server_name TEXT PRIMARY KEY,
    resource_uri TEXT NOT NULL,
    scopes TEXT NOT NULL,
    access_token TEXT NOT NULL,
    refresh_token TEXT,
    expires_at INTEGER NOT NULL
);
";

/// Track the `redirect_uri` a dynamic client registration was made with, so a
/// changed canonical redirect base (REQ-MCP-020) re-registers instead of
/// reusing a client the authorization server will reject on a redirect-URI
/// mismatch (REQ-MCP-011). NULL for pre-configured clients (operator-managed
/// redirect) and rows written before this column existed.
const MIGRATION_023: &str = r"
ALTER TABLE mcp_oauth_registrations ADD COLUMN redirect_uri TEXT;
";

/// Persist browser session tokens so they survive a server restart. Before this
/// table the valid-token set lived only in process memory, so every redeploy
/// silently invalidated every logged-in browser even though the `phoenix-auth`
/// cookie itself is durable. The token is opaque and random, so the primary-key
/// index is the only lookup path needed; `expires_at` makes the cookie's
/// advertised lifetime authoritative and lets a sweep reclaim stale rows.
///
/// `password_fingerprint` binds each token to the password it was minted under
/// (a non-reversible hash of `PHOENIX_PASSWORD`). Validation requires it to
/// match the currently-configured password, so rotating the password
/// invalidates every prior session — matching the pre-persistence behaviour
/// where a restart cleared the in-memory store.
const MIGRATION_024: &str = r"
CREATE TABLE IF NOT EXISTS auth_sessions (
    token TEXT PRIMARY KEY,
    password_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
";

/// Normalize user/skill message attachments out of the `messages.content` blob
/// into child tables (`specs`-tracked task: normalize-message-attachments).
///
/// Child collections never belong inside a JSON-TEXT aggregate. This creates the
/// tables and performs the one-time extraction *out of* the blob via `json_each`
/// — the legitimate move that the "if a migration needs `json_extract`, the
/// field wanted to be a row" rule points at. `ordinal` preserves array order;
/// `message_files` covers user+skill rows, `message_images` covers user rows
/// (`SkillContent` has no images). Stripping the now-redundant keys from the
/// blob and cutting the read/write paths over to these tables lands with the
/// code change that makes them authoritative.
const MIGRATION_025: &str = r"
CREATE TABLE IF NOT EXISTS message_files (
    message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    original_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    stored_path TEXT NOT NULL,
    PRIMARY KEY (message_id, ordinal)
);

CREATE TABLE IF NOT EXISTS message_images (
    message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (message_id, ordinal)
);

INSERT INTO message_files (message_id, ordinal, original_name, media_type, size_bytes, stored_path)
SELECT m.message_id, f.key,
       json_extract(f.value, '$.original_name'),
       json_extract(f.value, '$.media_type'),
       json_extract(f.value, '$.size_bytes'),
       json_extract(f.value, '$.stored_path')
FROM messages m, json_each(m.content, '$.files') f
WHERE m.message_type IN ('user', 'skill')
  AND json_type(m.content, '$.files') = 'array';

INSERT INTO message_images (message_id, ordinal, media_type, data)
SELECT m.message_id, im.key,
       json_extract(im.value, '$.media_type'),
       json_extract(im.value, '$.data')
FROM messages m, json_each(m.content, '$.images') im
WHERE m.message_type = 'user'
  AND json_type(m.content, '$.images') = 'array';
";

/// Strip the now-redundant `files`/`images` keys from the `messages.content`
/// blob, completing the cutover begun in migration 025. After 025 copied the
/// attachments into `message_files`/`message_images`, the blob copy is a stale
/// parallel representation; the read/write paths now treat the child tables as
/// the single source of truth, so the blob keys are removed. `json_remove` is a
/// no-op when the path is absent, so this is safe across both back-filled and
/// freshly written (already attachment-free) rows.
const MIGRATION_026: &str = r"
UPDATE messages
SET content = json_remove(content, '$.files', '$.images')
WHERE message_type = 'user'
  AND (json_type(content, '$.files') IS NOT NULL OR json_type(content, '$.images') IS NOT NULL);

UPDATE messages
SET content = json_remove(content, '$.files')
WHERE message_type = 'skill'
  AND json_type(content, '$.files') IS NOT NULL;
";

/// Normalize `conversations.steering_queue` into child tables (task 58035,
/// follows the message-attachment normalization).
///
/// Creates `steering_messages` and its grandchild attachment tables, then
/// extracts the pending queue out of the blob via `json_each`. The column holds
/// either the versioned envelope (`{"v":"v1","entries":[…]}`) or a pre-envelope
/// bare array; the `norm` CTE normalizes both to one array. `e.key` is the FIFO
/// ordinal; per-entry `files`/`images` become grandchild rows; `skill_invocation`
/// flattens to the `skill_*` columns. The blob column is left in place but unread
/// after the code cutover (it carries no data — every write defaults it to `[]`).
///
/// The backfill is at least as tolerant as the `decode_steering_queue` it
/// replaces, because a migration abort here would brick startup: malformed-JSON
/// rows are skipped (`json_valid` guard), entries missing the required
/// `message_id`/`text` are dropped, duplicate `message_id`s keep the first FIFO
/// occurrence (the column had no uniqueness), the `skill_*` trio is written only
/// when complete (else NULL, honoring the CHECK), and attachment rows missing a
/// NOT NULL field are skipped.
const MIGRATION_027: &str = r"
CREATE TABLE IF NOT EXISTS steering_messages (
    message_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    text TEXT NOT NULL,
    llm_text TEXT,
    user_agent TEXT,
    skill_name TEXT,
    skill_body TEXT,
    skill_dir TEXT,
    UNIQUE (conversation_id, ordinal),
    CHECK ((skill_name IS NULL) = (skill_body IS NULL)
       AND (skill_name IS NULL) = (skill_dir IS NULL))
);

CREATE INDEX IF NOT EXISTS idx_steering_messages_conversation
    ON steering_messages(conversation_id, ordinal);

CREATE TABLE IF NOT EXISTS steering_message_files (
    message_id TEXT NOT NULL REFERENCES steering_messages(message_id) ON DELETE CASCADE,
    file_ordinal INTEGER NOT NULL,
    original_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    stored_path TEXT NOT NULL,
    PRIMARY KEY (message_id, file_ordinal)
);

CREATE TABLE IF NOT EXISTS steering_message_images (
    message_id TEXT NOT NULL REFERENCES steering_messages(message_id) ON DELETE CASCADE,
    image_ordinal INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (message_id, image_ordinal)
);

INSERT INTO steering_messages
    (message_id, conversation_id, ordinal, text, llm_text, user_agent,
     skill_name, skill_body, skill_dir)
WITH norm AS (
    SELECT c.id AS conversation_id,
           CASE
               WHEN NOT json_valid(c.steering_queue) THEN '[]'
               WHEN json_type(c.steering_queue, '$.entries') = 'array'
                   THEN json_extract(c.steering_queue, '$.entries')
               WHEN json_type(c.steering_queue, '$') = 'array'
                   THEN c.steering_queue
               ELSE '[]'
           END AS arr
    FROM conversations c
),
elems AS (
    -- Substitute an empty object for any non-object array element so the
    -- json_extract predicates below never parse a scalar element (a bare
    -- string would raise SQLite 'malformed JSON' and abort the migration).
    SELECT norm.conversation_id AS conversation_id,
           e.key AS ordinal,
           CASE WHEN e.type = 'object' THEN e.value ELSE '{}' END AS entry
    FROM norm, json_each(norm.arr) e
),
valid_entries AS (
    SELECT conversation_id, ordinal, entry FROM (
        SELECT conversation_id, ordinal, entry,
               ROW_NUMBER() OVER (
                   PARTITION BY json_extract(entry, '$.message_id')
                   ORDER BY conversation_id, ordinal
               ) AS rn
        FROM elems
        WHERE json_extract(entry, '$.message_id') IS NOT NULL
          AND json_extract(entry, '$.text') IS NOT NULL
    )
    WHERE rn = 1
)
SELECT json_extract(entry, '$.message_id'),
       conversation_id,
       ordinal,
       json_extract(entry, '$.text'),
       json_extract(entry, '$.llm_text'),
       json_extract(entry, '$.user_agent'),
       CASE WHEN json_extract(entry, '$.skill_invocation.name') IS NOT NULL
             AND json_extract(entry, '$.skill_invocation.body') IS NOT NULL
             AND json_extract(entry, '$.skill_invocation.skill_dir') IS NOT NULL
            THEN json_extract(entry, '$.skill_invocation.name') END,
       CASE WHEN json_extract(entry, '$.skill_invocation.name') IS NOT NULL
             AND json_extract(entry, '$.skill_invocation.body') IS NOT NULL
             AND json_extract(entry, '$.skill_invocation.skill_dir') IS NOT NULL
            THEN json_extract(entry, '$.skill_invocation.body') END,
       CASE WHEN json_extract(entry, '$.skill_invocation.name') IS NOT NULL
             AND json_extract(entry, '$.skill_invocation.body') IS NOT NULL
             AND json_extract(entry, '$.skill_invocation.skill_dir') IS NOT NULL
            THEN json_extract(entry, '$.skill_invocation.skill_dir') END
FROM valid_entries;

INSERT INTO steering_message_files
    (message_id, file_ordinal, original_name, media_type, size_bytes, stored_path)
WITH norm AS (
    SELECT c.id AS conversation_id,
           CASE
               WHEN NOT json_valid(c.steering_queue) THEN '[]'
               WHEN json_type(c.steering_queue, '$.entries') = 'array'
                   THEN json_extract(c.steering_queue, '$.entries')
               WHEN json_type(c.steering_queue, '$') = 'array'
                   THEN c.steering_queue
               ELSE '[]'
           END AS arr
    FROM conversations c
),
elems AS (
    -- Substitute an empty object for any non-object array element so the
    -- json_extract predicates below never parse a scalar element (a bare
    -- string would raise SQLite 'malformed JSON' and abort the migration).
    SELECT norm.conversation_id AS conversation_id,
           e.key AS ordinal,
           CASE WHEN e.type = 'object' THEN e.value ELSE '{}' END AS entry
    FROM norm, json_each(norm.arr) e
),
valid_entries AS (
    SELECT conversation_id, ordinal, entry FROM (
        SELECT conversation_id, ordinal, entry,
               ROW_NUMBER() OVER (
                   PARTITION BY json_extract(entry, '$.message_id')
                   ORDER BY conversation_id, ordinal
               ) AS rn
        FROM elems
        WHERE json_extract(entry, '$.message_id') IS NOT NULL
          AND json_extract(entry, '$.text') IS NOT NULL
    )
    WHERE rn = 1
),
file_elems AS (
    SELECT json_extract(entry, '$.message_id') AS message_id,
           f.key AS file_ordinal,
           CASE WHEN f.type = 'object' THEN f.value ELSE '{}' END AS fval
    FROM valid_entries, json_each(entry, '$.files') f
    WHERE json_type(entry, '$.files') = 'array'
)
SELECT message_id,
       file_ordinal,
       json_extract(fval, '$.original_name'),
       json_extract(fval, '$.media_type'),
       json_extract(fval, '$.size_bytes'),
       json_extract(fval, '$.stored_path')
FROM file_elems
WHERE json_extract(fval, '$.original_name') IS NOT NULL
  AND json_extract(fval, '$.media_type') IS NOT NULL
  AND json_extract(fval, '$.size_bytes') IS NOT NULL
  AND json_extract(fval, '$.stored_path') IS NOT NULL;

INSERT INTO steering_message_images
    (message_id, image_ordinal, media_type, data)
WITH norm AS (
    SELECT c.id AS conversation_id,
           CASE
               WHEN NOT json_valid(c.steering_queue) THEN '[]'
               WHEN json_type(c.steering_queue, '$.entries') = 'array'
                   THEN json_extract(c.steering_queue, '$.entries')
               WHEN json_type(c.steering_queue, '$') = 'array'
                   THEN c.steering_queue
               ELSE '[]'
           END AS arr
    FROM conversations c
),
elems AS (
    -- Substitute an empty object for any non-object array element so the
    -- json_extract predicates below never parse a scalar element (a bare
    -- string would raise SQLite 'malformed JSON' and abort the migration).
    SELECT norm.conversation_id AS conversation_id,
           e.key AS ordinal,
           CASE WHEN e.type = 'object' THEN e.value ELSE '{}' END AS entry
    FROM norm, json_each(norm.arr) e
),
valid_entries AS (
    SELECT conversation_id, ordinal, entry FROM (
        SELECT conversation_id, ordinal, entry,
               ROW_NUMBER() OVER (
                   PARTITION BY json_extract(entry, '$.message_id')
                   ORDER BY conversation_id, ordinal
               ) AS rn
        FROM elems
        WHERE json_extract(entry, '$.message_id') IS NOT NULL
          AND json_extract(entry, '$.text') IS NOT NULL
    )
    WHERE rn = 1
),
image_elems AS (
    SELECT json_extract(entry, '$.message_id') AS message_id,
           im.key AS image_ordinal,
           CASE WHEN im.type = 'object' THEN im.value ELSE '{}' END AS imval
    FROM valid_entries, json_each(entry, '$.images') im
    WHERE json_type(entry, '$.images') = 'array'
)
SELECT message_id,
       image_ordinal,
       json_extract(imval, '$.media_type'),
       json_extract(imval, '$.data')
FROM image_elems
WHERE json_extract(imval, '$.media_type') IS NOT NULL
  AND json_extract(imval, '$.data') IS NOT NULL;

-- Clear the now-migrated blob so the column carries no data: the child tables
-- are the single source of truth and nothing reads this column anymore.
UPDATE conversations
SET steering_queue = '[]'
WHERE steering_queue IS NOT NULL AND steering_queue <> '[]';
";

/// Normalize `conversations.conv_mode` (the `ConvMode` tagged union) out of its
/// JSON blob into columns (task 58037, the final JSON-in-TEXT audit item).
///
/// `cm_kind` is the discriminator (`explore` | `direct` | `work` | `branch`);
/// the `cm_*` columns hold each variant's fields. The values are extracted out
/// of the blob via `json_extract` — the legitimate move the "if a migration
/// needs `json_extract`, the field wanted to be a column" rule points at. A row
/// with malformed/absent `conv_mode` leaves `cm_kind` NULL, which the read path
/// treats as the default `explore`. The invariant that the field columns match
/// the kind is upheld by the `ConvMode` enum (the sole writer); the blob column
/// itself is dropped in the following migration once the code reads the columns.
const MIGRATION_028: &str = r"
ALTER TABLE conversations ADD COLUMN cm_kind TEXT;
ALTER TABLE conversations ADD COLUMN cm_branch_name TEXT;
ALTER TABLE conversations ADD COLUMN cm_worktree_path TEXT;
ALTER TABLE conversations ADD COLUMN cm_base_branch TEXT;
ALTER TABLE conversations ADD COLUMN cm_task_id TEXT;
ALTER TABLE conversations ADD COLUMN cm_task_title TEXT;
ALTER TABLE conversations ADD COLUMN cm_next_taskmd_id_hint TEXT;

UPDATE conversations
SET cm_kind = lower(json_extract(conv_mode, '$.mode')),
    cm_branch_name = NULLIF(json_extract(conv_mode, '$.branch_name'), ''),
    cm_worktree_path = NULLIF(json_extract(conv_mode, '$.worktree_path'), ''),
    cm_base_branch = NULLIF(json_extract(conv_mode, '$.base_branch'), ''),
    cm_task_id = NULLIF(json_extract(conv_mode, '$.task_id'), ''),
    cm_task_title = NULLIF(json_extract(conv_mode, '$.task_title'), ''),
    cm_next_taskmd_id_hint = NULLIF(json_extract(conv_mode, '$.next_taskmd_id_hint'), '')
WHERE json_valid(conv_mode)
  AND json_extract(conv_mode, '$.mode') IS NOT NULL;
";

/// Drop the now-redundant `conv_mode` JSON blob column: the `cm_*` columns
/// (migration 028) are the single source of truth and the code reads/writes
/// them exclusively. The column is moved into the base schema and the idempotent
/// legacy `ALTER` that used to add it is removed, so this `DROP COLUMN` is not
/// resurrected on the next boot.
const MIGRATION_029: &str = r"
ALTER TABLE conversations DROP COLUMN conv_mode;
";

/// Monotonic per-conversation clear watermark for stale tool-result clearing
/// (`specs/stale-tool-results`): every clearable tool result at or before this
/// message `sequence_id` is elided from the model-bound history. `DEFAULT 0`
/// means "nothing cleared", the correct value for every existing row, so no
/// backfill is owed.
const MIGRATION_030: &str = r"
ALTER TABLE conversations ADD COLUMN clear_watermark INTEGER NOT NULL DEFAULT 0;
";

const MIGRATION_032: &str = r"
ALTER TABLE work_scope_pr_associations ADD COLUMN feedback_status TEXT NOT NULL DEFAULT 'open';
";

const MIGRATION_035: &str = r"
CREATE TABLE IF NOT EXISTS conversation_creation_jobs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT UNIQUE,
    phase TEXT NOT NULL,
    intent_json TEXT NOT NULL,
    error TEXT,
    accepted_at TEXT,
    provisioning_started_at TEXT,
    completed_at TEXT,
    failed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (phase IN ('accepted', 'provisioning', 'ready', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_creation_jobs_phase_updated
    ON conversation_creation_jobs(phase, updated_at);
";

const MIGRATION_036: &str = r"
CREATE TABLE IF NOT EXISTS conversation_creation_job_files (
    job_id TEXT NOT NULL REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    original_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    stored_path TEXT NOT NULL,
    PRIMARY KEY (job_id, ordinal)
);

CREATE INDEX IF NOT EXISTS idx_creation_job_files_stored_path
    ON conversation_creation_job_files(stored_path);

INSERT OR IGNORE INTO conversation_creation_job_files (
    job_id, ordinal, original_name, media_type, size_bytes, stored_path
)
SELECT j.id,
       file.key,
       json_extract(file.value, '$.original_name'),
       json_extract(file.value, '$.media_type'),
       json_extract(file.value, '$.size_bytes'),
       json_extract(file.value, '$.stored_path')
FROM conversation_creation_jobs j, json_each(j.intent_json, '$.files') AS file
WHERE json_type(j.intent_json, '$.files') = 'array'
  AND json_extract(file.value, '$.original_name') IS NOT NULL
  AND json_extract(file.value, '$.media_type') IS NOT NULL
  AND json_extract(file.value, '$.size_bytes') IS NOT NULL
  AND json_extract(file.value, '$.stored_path') IS NOT NULL;
";

const MIGRATION_037: &str = r"
CREATE TABLE IF NOT EXISTS conversation_creation_job_images (
    job_id TEXT NOT NULL REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (job_id, ordinal)
);

INSERT OR IGNORE INTO conversation_creation_job_images (
    job_id, ordinal, media_type, data
)
SELECT j.id,
       image.key,
       json_extract(image.value, '$.media_type'),
       json_extract(image.value, '$.data')
FROM conversation_creation_jobs j, json_each(j.intent_json, '$.images') AS image
WHERE json_type(j.intent_json, '$.images') = 'array'
  AND json_extract(image.value, '$.media_type') IS NOT NULL
  AND json_extract(image.value, '$.data') IS NOT NULL;
";

const MIGRATION_038: &str = r"
ALTER TABLE conversation_creation_jobs RENAME TO conversation_creation_jobs_legacy;
ALTER TABLE conversation_creation_job_files RENAME TO conversation_creation_job_files_legacy;
ALTER TABLE conversation_creation_job_images RENAME TO conversation_creation_job_images_legacy;

CREATE TABLE conversation_creation_jobs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT UNIQUE,
    status TEXT NOT NULL,
    stage TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 0,
    generation INTEGER NOT NULL DEFAULT 0,
    claim_worker_id TEXT,
    claim_token TEXT,
    lease_until TEXT,
    next_attempt_at TEXT,
    intent_json TEXT NOT NULL,
    error TEXT,
    accepted_at TEXT NOT NULL,
    provisioning_started_at TEXT,
    completed_at TEXT,
    failed_at TEXT,
    cancelled_at TEXT,
    deletion_requested_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'deletion_pending', 'ready', 'failed')),
    CHECK (stage IN ('validate_intent', 'resolve_repository', 'reserve_resources', 'materialize_worktree', 'finalize_attachments', 'expand_initial_message', 'commit_metadata', 'bootstrap_initial_turn', 'finalize')),
    CHECK (attempt >= 0 AND attempt <= 4),
    CHECK (generation >= attempt),
    CHECK ((status = 'claimed') = (claim_worker_id IS NOT NULL AND claim_token IS NOT NULL AND lease_until IS NOT NULL)),
    CHECK ((status = 'retry_scheduled') = (next_attempt_at IS NOT NULL)),
    CHECK ((status = 'ready') = (completed_at IS NOT NULL)),
    CHECK ((status = 'failed') = (failed_at IS NOT NULL AND error IS NOT NULL)),
    CHECK ((status = 'cancelled') = (cancelled_at IS NOT NULL)),
    CHECK ((status = 'deletion_pending') = (deletion_requested_at IS NOT NULL))
);

INSERT INTO conversation_creation_jobs (
    id, conversation_id, message_id, status, stage, attempt, generation,
    intent_json, error, accepted_at, provisioning_started_at, completed_at,
    failed_at, created_at, updated_at
)
SELECT id, conversation_id, message_id,
       CASE phase WHEN 'provisioning' THEN 'accepted' ELSE phase END,
       'validate_intent', 0, 0, intent_json,
       CASE WHEN phase = 'failed' THEN COALESCE(error, 'creation failed') ELSE error END,
       COALESCE(accepted_at, created_at), provisioning_started_at,
       completed_at, failed_at, created_at, updated_at
FROM conversation_creation_jobs_legacy;

CREATE TABLE conversation_creation_job_files (
    job_id TEXT NOT NULL REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    original_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    stored_path TEXT NOT NULL,
    PRIMARY KEY (job_id, ordinal)
);
INSERT INTO conversation_creation_job_files SELECT * FROM conversation_creation_job_files_legacy;

CREATE TABLE conversation_creation_job_images (
    job_id TEXT NOT NULL REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (job_id, ordinal)
);
INSERT INTO conversation_creation_job_images SELECT * FROM conversation_creation_job_images_legacy;

DROP TABLE conversation_creation_job_files_legacy;
DROP TABLE conversation_creation_job_images_legacy;
DROP TABLE conversation_creation_jobs_legacy;

DROP INDEX IF EXISTS idx_creation_jobs_phase_updated;
CREATE INDEX idx_creation_jobs_due
    ON conversation_creation_jobs(status, next_attempt_at, lease_until, accepted_at);
CREATE INDEX idx_creation_job_files_stored_path
    ON conversation_creation_job_files(stored_path);
";

const MIGRATION_039: &str = r"
CREATE TABLE conversation_creation_resource_reservations (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL,
    repository_identity TEXT NOT NULL,
    resource_identity TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(job_id, resource_identity),
    CHECK (generation > 0),
    CHECK (status IN ('reserved', 'present', 'cleanup_required', 'released', 'conflict'))
);

CREATE INDEX idx_creation_resource_reservations_job
    ON conversation_creation_resource_reservations(job_id, status);
";

const MIGRATION_040: &str = r"
ALTER TABLE conversation_creation_jobs ADD COLUMN cleanup_worker_id TEXT;
ALTER TABLE conversation_creation_jobs ADD COLUMN cleanup_token TEXT;
ALTER TABLE conversation_creation_jobs ADD COLUMN cleanup_lease_until TEXT;

CREATE INDEX idx_creation_cleanup_due
    ON conversation_creation_jobs(status, cleanup_lease_until, updated_at);
";

const MIGRATION_044: &str = r"
CREATE TABLE IF NOT EXISTS workflow_protocol_selections (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    selector_identity TEXT NOT NULL,
    selector_version INTEGER NOT NULL,
    protocol_version INTEGER NOT NULL,
    authority TEXT NOT NULL,
    accepting INTEGER NOT NULL,
    runtime_acceptance_enabled INTEGER NOT NULL,
    external_acceptance_enabled INTEGER NOT NULL,
    registered_at TEXT NOT NULL,
    drained_at TEXT,
    UNIQUE (id, profile_id, protocol_version, authority),
    UNIQUE (profile_id, selector_identity, selector_version, protocol_version, authority),
    CHECK (authority IN ('legacy_protocol', 'engine_protocol')),
    CHECK (accepting IN (0, 1)),
    CHECK (runtime_acceptance_enabled IN (0, 1)),
    CHECK (external_acceptance_enabled IN (0, 1)),
    CHECK (selector_version >= 0),
    CHECK (protocol_version >= 0),
    CHECK ((accepting = 0) = (drained_at IS NOT NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_protocol_one_accepting_per_profile
    ON workflow_protocol_selections(profile_id)
    WHERE accepting = 1;

CREATE TABLE IF NOT EXISTS workflow_profile_codecs (
    selection_id TEXT NOT NULL REFERENCES workflow_protocol_selections(id) ON DELETE CASCADE,
    codec_family TEXT NOT NULL,
    codec_version INTEGER NOT NULL,
    PRIMARY KEY (selection_id, codec_family, codec_version)
);

CREATE TABLE IF NOT EXISTS workflow_profile_executors (
    selection_id TEXT NOT NULL REFERENCES workflow_protocol_selections(id) ON DELETE CASCADE,
    executor_kind TEXT NOT NULL,
    PRIMARY KEY (selection_id, executor_kind)
);
";

const MIGRATION_045: &str = r"
CREATE TABLE IF NOT EXISTS external_acceptance_bindings (
    id TEXT PRIMARY KEY,
    selection_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    protocol_version INTEGER NOT NULL,
    authority TEXT NOT NULL,
    authority_scope TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    intent_fingerprint TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    receipt_codec_family TEXT NOT NULL,
    receipt_codec_version INTEGER NOT NULL,
    receipt_payload TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    FOREIGN KEY (selection_id, profile_id, protocol_version, authority)
        REFERENCES workflow_protocol_selections(id, profile_id, protocol_version, authority)
        ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, profile_id, protocol_version, authority)
        REFERENCES workflows(id, profile_id, protocol_version, authority)
        ON DELETE CASCADE,
    CHECK (authority IN ('legacy_protocol', 'engine_protocol')),
    CHECK (protocol_version >= 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_external_acceptance_idempotency
    ON external_acceptance_bindings(
        profile_id,
        protocol_version,
        authority_scope,
        idempotency_key
    );

CREATE TABLE IF NOT EXISTS workflows (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    protocol_version INTEGER NOT NULL,
    authority TEXT NOT NULL,
    execution_mode TEXT NOT NULL,
    authoritative_workflow_id TEXT REFERENCES workflows(id) ON DELETE RESTRICT,
    protocol_selection_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    status TEXT NOT NULL,
    snapshot_codec_family TEXT NOT NULL,
    snapshot_codec_version INTEGER NOT NULL,
    snapshot_payload TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    UNIQUE (id, version),
    UNIQUE (id, profile_id, protocol_version, authority),
    FOREIGN KEY (protocol_selection_id, profile_id, protocol_version, authority)
        REFERENCES workflow_protocol_selections(id, profile_id, protocol_version, authority)
        ON DELETE RESTRICT,
    CHECK (authority IN ('legacy_protocol', 'engine_protocol')),
    CHECK (execution_mode IN ('authoritative', 'shadow')),
    CHECK (status IN ('active', 'cancelling', 'cancelled', 'deletion_pending', 'completed', 'failed')),
    CHECK (version >= 0),
    CHECK (generation >= 0),
    CHECK ((execution_mode = 'shadow') = (authoritative_workflow_id IS NOT NULL))
);

CREATE TABLE IF NOT EXISTS workflow_transitions (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    from_version INTEGER NOT NULL,
    to_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    event_codec_family TEXT NOT NULL,
    event_codec_version INTEGER NOT NULL,
    event_payload TEXT NOT NULL,
    committed_at TEXT NOT NULL,
    UNIQUE (workflow_id, from_version),
    UNIQUE (workflow_id, to_version),
    UNIQUE (id, workflow_id, to_version),
    CHECK (to_version = from_version + 1),
    CHECK (from_version >= 0),
    CHECK (generation >= 0)
);

CREATE TABLE IF NOT EXISTS workflow_effects (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declaring_transition_id TEXT NOT NULL REFERENCES workflow_transitions(id) ON DELETE CASCADE,
    declared_workflow_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    family TEXT NOT NULL,
    kind TEXT NOT NULL,
    codec_family TEXT NOT NULL,
    codec_version INTEGER NOT NULL,
    role TEXT NOT NULL,
    ambiguity_policy TEXT NOT NULL,
    intent_payload TEXT NOT NULL,
    status TEXT NOT NULL,
    pending_reconciliation INTEGER NOT NULL DEFAULT 0,
    next_eligible_at TEXT,
    destructive_resource TEXT,
    UNIQUE (id, workflow_id, declared_workflow_version, generation),
    FOREIGN KEY (declaring_transition_id, workflow_id, declared_workflow_version)
        REFERENCES workflow_transitions(id, workflow_id, to_version)
        ON DELETE CASCADE,
    CHECK (role IN ('required', 'optional', 'compensation')),
    CHECK (ambiguity_policy IN ('observable_reconciliation', 'external_idempotency', 'safe_repeatability', 'manual_resolution')),
    CHECK (status IN ('blocked', 'eligible', 'claimed', 'retry_wait', 'ambiguity_wait', 'receipted', 'invalidated')),
    CHECK (pending_reconciliation IN (0, 1))
);

CREATE TABLE IF NOT EXISTS workflow_effect_dependencies (
    effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    dependency_effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    PRIMARY KEY (effect_id, dependency_effect_id),
    CHECK (effect_id <> dependency_effect_id)
);

CREATE TABLE IF NOT EXISTS workflow_barriers (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declaring_transition_id TEXT NOT NULL REFERENCES workflow_transitions(id) ON DELETE CASCADE,
    declaring_workflow_version INTEGER NOT NULL,
    status TEXT NOT NULL,
    satisfied_at TEXT,
    event_codec_family TEXT NOT NULL,
    event_codec_version INTEGER NOT NULL,
    event_payload TEXT NOT NULL,
    FOREIGN KEY (declaring_transition_id, workflow_id, declaring_workflow_version)
        REFERENCES workflow_transitions(id, workflow_id, to_version)
        ON DELETE CASCADE,
    CHECK (status IN ('waiting', 'satisfied', 'invalidated')),
    CHECK ((status = 'satisfied') = (satisfied_at IS NOT NULL))
);

CREATE TABLE IF NOT EXISTS workflow_barrier_members (
    barrier_id TEXT NOT NULL REFERENCES workflow_barriers(id) ON DELETE CASCADE,
    effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    receipt_family TEXT NOT NULL,
    PRIMARY KEY (barrier_id, effect_id),
    CHECK (receipt_family IN ('current_generation_effect', 'compensation_effect'))
);

CREATE TABLE IF NOT EXISTS workflow_claims (
    effect_id TEXT PRIMARY KEY REFERENCES workflow_effects(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declared_workflow_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    claim_token TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    lease_until TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    revoked_at TEXT,
    UNIQUE (effect_id, workflow_id, declared_workflow_version, generation),
    UNIQUE (workflow_id, claim_token),
    FOREIGN KEY (effect_id, workflow_id, declared_workflow_version, generation)
        REFERENCES workflow_effects(id, workflow_id, declared_workflow_version, generation)
        ON DELETE CASCADE,
    CHECK (declared_workflow_version >= 0),
    CHECK (generation >= 0),
    CHECK (claim_token <> ''),
    CHECK (worker_id <> '')
);

CREATE TABLE IF NOT EXISTS workflow_attempts (
    id TEXT PRIMARY KEY,
    effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declared_workflow_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    claim_token TEXT NOT NULL,
    claim_worker_id TEXT NOT NULL,
    claim_lease_until TEXT NOT NULL,
    claim_issued_at TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    status TEXT NOT NULL,
    begun_at TEXT NOT NULL,
    UNIQUE (id, effect_id, workflow_id, declared_workflow_version, generation),
    UNIQUE (effect_id, ordinal),
    FOREIGN KEY (effect_id, workflow_id, declared_workflow_version, generation)
        REFERENCES workflow_effects(id, workflow_id, declared_workflow_version, generation)
        ON DELETE CASCADE,
    CHECK (status IN ('begun', 'observation_recorded', 'receipt_accepted', 'authority_lost')),
    CHECK (ordinal >= 0),
    CHECK (claim_token <> ''),
    CHECK (claim_worker_id <> '')
);

CREATE TABLE IF NOT EXISTS workflow_observations (
    id TEXT PRIMARY KEY,
    effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES workflow_attempts(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declared_workflow_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    claim_token TEXT NOT NULL,
    claim_worker_id TEXT NOT NULL,
    claim_lease_until TEXT NOT NULL,
    claim_issued_at TEXT NOT NULL,
    codec_family TEXT NOT NULL,
    codec_version INTEGER NOT NULL,
    payload TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    authoritative INTEGER NOT NULL,
    FOREIGN KEY (attempt_id, effect_id, workflow_id, declared_workflow_version, generation)
        REFERENCES workflow_attempts(id, effect_id, workflow_id, declared_workflow_version, generation)
        ON DELETE CASCADE,
    CHECK (authoritative = 1),
    CHECK (claim_token <> ''),
    CHECK (claim_worker_id <> '')
);

CREATE TABLE IF NOT EXISTS workflow_stale_observations (
    id TEXT PRIMARY KEY,
    effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES workflow_attempts(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declared_workflow_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    claim_token TEXT NOT NULL,
    claim_worker_id TEXT NOT NULL,
    claim_lease_until TEXT NOT NULL,
    claim_issued_at TEXT NOT NULL,
    codec_family TEXT NOT NULL,
    codec_version INTEGER NOT NULL,
    payload TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    stale_reason TEXT NOT NULL,
    FOREIGN KEY (attempt_id, effect_id, workflow_id, declared_workflow_version, generation)
        REFERENCES workflow_attempts(id, effect_id, workflow_id, declared_workflow_version, generation)
        ON DELETE CASCADE,
    CHECK (claim_token <> ''),
    CHECK (claim_worker_id <> '')
);

CREATE TABLE IF NOT EXISTS workflow_receipts (
    id TEXT PRIMARY KEY,
    effect_id TEXT NOT NULL UNIQUE REFERENCES workflow_effects(id) ON DELETE CASCADE,
    attempt_id TEXT REFERENCES workflow_attempts(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    declared_workflow_version INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    claim_token TEXT,
    claim_worker_id TEXT,
    claim_lease_until TEXT,
    claim_issued_at TEXT,
    codec_family TEXT NOT NULL,
    codec_version INTEGER NOT NULL,
    payload TEXT NOT NULL,
    origin TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    FOREIGN KEY (attempt_id, effect_id, workflow_id, declared_workflow_version, generation)
        REFERENCES workflow_attempts(id, effect_id, workflow_id, declared_workflow_version, generation)
        ON DELETE CASCADE,
    CHECK (origin IN ('execution', 'adoption', 'reconciliation', 'manual')),
    CHECK (generation >= 0),
    CHECK (
        (attempt_id IS NULL AND claim_token IS NULL AND claim_worker_id IS NULL AND claim_lease_until IS NULL AND claim_issued_at IS NULL)
        OR
        (attempt_id IS NOT NULL AND claim_token IS NOT NULL AND claim_worker_id IS NOT NULL AND claim_lease_until IS NOT NULL AND claim_issued_at IS NOT NULL AND claim_token <> '' AND claim_worker_id <> '')
    )
);

CREATE TABLE IF NOT EXISTS workflow_reducer_inbox (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    receipt_id TEXT REFERENCES workflow_receipts(id) ON DELETE CASCADE,
    barrier_id TEXT REFERENCES workflow_barriers(id) ON DELETE CASCADE,
    event_codec_family TEXT NOT NULL,
    event_codec_version INTEGER NOT NULL,
    event_payload TEXT NOT NULL,
    requires_runtime_acceptance INTEGER NOT NULL DEFAULT 1,
    delivery_status TEXT NOT NULL,
    consumed_by_transition_id TEXT REFERENCES workflow_transitions(id) ON DELETE SET NULL,
    CHECK (requires_runtime_acceptance IN (0, 1)),
    CHECK (delivery_status IN ('pending', 'consumed', 'suppressed')),
    CHECK ((delivery_status = 'consumed') = (consumed_by_transition_id IS NOT NULL)),
    CHECK (NOT (receipt_id IS NOT NULL AND barrier_id IS NOT NULL))
);

CREATE TABLE IF NOT EXISTS workflow_inbox_consumer_dispositions (
    reducer_inbox_id TEXT NOT NULL REFERENCES workflow_reducer_inbox(id) ON DELETE CASCADE,
    consumer_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    consumed_at TEXT,
    PRIMARY KEY (reducer_inbox_id, consumer_kind),
    CHECK (status IN ('pending', 'consumed')),
    CHECK ((status = 'consumed') = (consumed_at IS NOT NULL))
);

CREATE TABLE IF NOT EXISTS workflow_owed_acceptance (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    reducer_inbox_id TEXT NOT NULL UNIQUE REFERENCES workflow_reducer_inbox(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    event_codec_family TEXT NOT NULL,
    event_codec_version INTEGER NOT NULL,
    event_payload TEXT NOT NULL,
    status TEXT NOT NULL,
    resolving_transition_id TEXT REFERENCES workflow_transitions(id) ON DELETE SET NULL,
    suppression_reason TEXT,
    CHECK (source_kind <> ''),
    CHECK (status IN ('owed', 'accepted', 'suppressed')),
    CHECK (
        (status = 'owed' AND resolving_transition_id IS NULL AND suppression_reason IS NULL)
        OR (status = 'accepted' AND resolving_transition_id IS NOT NULL AND suppression_reason IS NULL)
        OR (status = 'suppressed' AND resolving_transition_id IS NOT NULL AND suppression_reason IS NOT NULL AND suppression_reason IN ('cancelled', 'superseded', 'lifecycle_terminal', 'operator_rejected'))
    )
);

CREATE TABLE IF NOT EXISTS workflow_manual_resolutions (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    effect_id TEXT NOT NULL REFERENCES workflow_effects(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    evidence_codec_family TEXT NOT NULL,
    evidence_codec_version INTEGER NOT NULL,
    evidence_payload TEXT NOT NULL,
    accepted_choice_id TEXT,
    resolved_by TEXT,
    UNIQUE (id, workflow_id),
    CHECK (status IN ('required', 'resolved', 'cancelled')),
    CHECK (
        (status IN ('required', 'cancelled') AND accepted_choice_id IS NULL AND resolved_by IS NULL)
        OR (status = 'resolved' AND accepted_choice_id IS NOT NULL AND resolved_by IS NOT NULL AND resolved_by <> '')
    )
);

CREATE TABLE IF NOT EXISTS workflow_manual_resolution_choices (
    id TEXT PRIMARY KEY,
    resolution_id TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    codec_family TEXT NOT NULL,
    codec_version INTEGER NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (id, resolution_id, workflow_id),
    FOREIGN KEY (resolution_id, workflow_id)
        REFERENCES workflow_manual_resolutions(id, workflow_id)
        ON DELETE CASCADE,
    CHECK (kind IN ('adopt', 'retry', 'compensate', 'fail', 'suppress'))
);

CREATE TRIGGER IF NOT EXISTS trg_workflow_manual_resolution_choice_owner_insert
BEFORE INSERT ON workflow_manual_resolutions
FOR EACH ROW
WHEN NEW.accepted_choice_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1
    FROM workflow_manual_resolution_choices c
    WHERE c.id = NEW.accepted_choice_id
      AND c.resolution_id = NEW.id
      AND c.workflow_id = NEW.workflow_id
 )
BEGIN
    SELECT RAISE(ABORT, 'accepted choice must belong to same resolution');
END;

CREATE TRIGGER IF NOT EXISTS trg_workflow_manual_resolution_choice_owner_update
BEFORE UPDATE OF accepted_choice_id, workflow_id, id ON workflow_manual_resolutions
FOR EACH ROW
WHEN NEW.accepted_choice_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1
    FROM workflow_manual_resolution_choices c
    WHERE c.id = NEW.accepted_choice_id
      AND c.resolution_id = NEW.id
      AND c.workflow_id = NEW.workflow_id
 )
BEGIN
    SELECT RAISE(ABORT, 'accepted choice must belong to same resolution');
END;

CREATE TABLE IF NOT EXISTS workflow_manual_resolution_evidence_links (
    resolution_id TEXT NOT NULL REFERENCES workflow_manual_resolutions(id) ON DELETE CASCADE,
    evidence_kind TEXT NOT NULL,
    evidence_id TEXT NOT NULL,
    PRIMARY KEY (resolution_id, evidence_kind, evidence_id)
);

CREATE TABLE IF NOT EXISTS workflow_shadow_divergences (
    id TEXT PRIMARY KEY,
    shadow_workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    authoritative_workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL,
    profile_detail_kind TEXT NOT NULL,
    severity TEXT NOT NULL,
    required_action TEXT NOT NULL,
    evidence_identity TEXT NOT NULL,
    resolution_action TEXT,
    resolved_by TEXT,
    resolved_at TEXT,
    expected_codec_family TEXT,
    expected_codec_version INTEGER,
    expected_payload TEXT,
    actual_codec_family TEXT,
    actual_codec_version INTEGER,
    actual_payload TEXT,
    recorded_at TEXT NOT NULL,
    CHECK (kind IN ('snapshot', 'transition', 'effect_plan', 'observation', 'receipt', 'reducer_event', 'capability', 'user_projection')),
    CHECK (severity IN ('blocking', 'actionable', 'informational')),
    CHECK (required_action IN ('halt_acceptance', 'retain_authority_and_investigate', 'record_only')),
    CHECK (resolution_action IS NULL OR resolution_action IN ('rollback', 'reauthorize')),
    CHECK (
        (resolved_at IS NULL AND resolution_action IS NULL AND resolved_by IS NULL)
        OR (resolved_at IS NOT NULL AND resolution_action IS NOT NULL AND resolved_by IS NOT NULL AND resolved_by <> '')
    ),
    CHECK (shadow_workflow_id <> authoritative_workflow_id),
    CHECK ((expected_codec_family IS NULL) = (expected_codec_version IS NULL) AND (expected_codec_family IS NULL) = (expected_payload IS NULL)),
    CHECK ((actual_codec_family IS NULL) = (actual_codec_version IS NULL) AND (actual_codec_family IS NULL) = (actual_payload IS NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_shadow_divergence_one_active
    ON workflow_shadow_divergences(shadow_workflow_id, kind, evidence_identity)
    WHERE resolved_at IS NULL;
";

const MIGRATION_046: &str = r"
CREATE TABLE IF NOT EXISTS wake_registration_fences (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    status TEXT NOT NULL,
    UNIQUE (conversation_id, version),
    CHECK (status IN ('open', 'closed')),
    CHECK (version >= 0)
);

CREATE TABLE IF NOT EXISTS wake_workflow_bindings (
    contract_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL UNIQUE REFERENCES workflows(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    registration_scope_kind TEXT NOT NULL,
    registration_scope_stable_key TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    bash_work_scope_kind TEXT,
    bash_work_scope_stable_key TEXT,
    bash_handle_id TEXT,
    tmux_work_scope_kind TEXT,
    tmux_work_scope_stable_key TEXT,
    tmux_server_generation TEXT,
    tmux_window_id TEXT,
    registering_tool_use_id TEXT NOT NULL,
    registered_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    registration_fence_version INTEGER NOT NULL,
    observe_effect_id TEXT NOT NULL,
    lifecycle_fence_status TEXT NOT NULL,
    UNIQUE (contract_id, workflow_id),
    UNIQUE (contract_id, workflow_id, observe_effect_id),
    FOREIGN KEY (conversation_id)
        REFERENCES wake_registration_fences(conversation_id)
        ON DELETE RESTRICT,
    FOREIGN KEY (observe_effect_id, workflow_id)
        REFERENCES workflow_effects(id, workflow_id)
        ON DELETE RESTRICT,
    CHECK (resource_kind IN ('bash', 'tmux_window', 'subagent')),
    CHECK (lifecycle_fence_status IN ('open', 'closed')),
    CHECK (registering_tool_use_id <> ''),
    CHECK (
        (resource_kind = 'bash'
        AND bash_work_scope_kind IS NOT NULL
        AND bash_work_scope_stable_key IS NOT NULL
        AND bash_handle_id IS NOT NULL
        AND tmux_work_scope_kind IS NULL
        AND tmux_work_scope_stable_key IS NULL
        AND tmux_server_generation IS NULL
        AND tmux_window_id IS NULL)
        OR
        (resource_kind = 'tmux_window'
        AND bash_work_scope_kind IS NULL
        AND bash_work_scope_stable_key IS NULL
        AND bash_handle_id IS NULL
        AND tmux_work_scope_kind IS NOT NULL
        AND tmux_work_scope_stable_key IS NOT NULL
        AND tmux_server_generation IS NOT NULL
        AND tmux_window_id IS NOT NULL)
    )
);

CREATE TRIGGER IF NOT EXISTS trg_wake_binding_registration_fence_insert
BEFORE INSERT ON wake_workflow_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM wake_registration_fences f
    WHERE f.conversation_id = NEW.conversation_id
      AND f.version IN (NEW.registration_fence_version, NEW.registration_fence_version + 1)
      AND f.status = 'open'
)
BEGIN
    SELECT RAISE(ABORT, 'registration_fence_version must name the immediately consumed open fence');
END;

CREATE TRIGGER IF NOT EXISTS trg_wake_binding_observe_effect_insert
BEFORE INSERT ON wake_workflow_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM workflow_effects e
    WHERE e.id = NEW.observe_effect_id
      AND e.workflow_id = NEW.workflow_id
      AND e.family = 'wake'
      AND e.kind = 'observe_handle'
      AND e.role = 'required'
)
BEGIN
    SELECT RAISE(ABORT, 'observe_effect_id must be wake required observe_handle effect for workflow');
END;

CREATE TRIGGER IF NOT EXISTS trg_wake_binding_observe_effect_update
BEFORE UPDATE OF observe_effect_id, workflow_id ON wake_workflow_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM workflow_effects e
    WHERE e.id = NEW.observe_effect_id
      AND e.workflow_id = NEW.workflow_id
      AND e.family = 'wake'
      AND e.kind = 'observe_handle'
      AND e.role = 'required'
)
BEGIN
    SELECT RAISE(ABORT, 'observe_effect_id must be wake required observe_handle effect for workflow');
END;

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_receipts_id_effect_workflow
    ON workflow_receipts(id, effect_id, workflow_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_effects_id_workflow
    ON workflow_effects(id, workflow_id);

CREATE TABLE IF NOT EXISTS wake_terminal_receipts (
    receipt_id TEXT PRIMARY KEY REFERENCES workflow_receipts(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    contract_id TEXT NOT NULL,
    observe_effect_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    bash_status TEXT,
    bash_occurred_at TEXT,
    bash_exit_code INTEGER,
    bash_duration_ms INTEGER,
    bash_signal_number INTEGER,
    bash_kill_signal_sent TEXT,
    bash_tail_start_offset INTEGER,
    bash_tail_end_offset INTEGER,
    bash_tail_truncated_before INTEGER,
    tmux_status TEXT,
    tmux_occurred_at TEXT,
    tmux_server_generation TEXT,
    tmux_exit_code INTEGER,
    tmux_duration_ms INTEGER,
    forgotten_reason TEXT,
    cancellation_reason TEXT,
    UNIQUE (receipt_id, workflow_id, observe_effect_id),
    FOREIGN KEY (contract_id, workflow_id, observe_effect_id)
        REFERENCES wake_workflow_bindings(contract_id, workflow_id, observe_effect_id)
        ON DELETE CASCADE,
    FOREIGN KEY (receipt_id, observe_effect_id, workflow_id)
        REFERENCES workflow_receipts(id, effect_id, workflow_id)
        ON DELETE CASCADE,
    CHECK (resource_kind IN ('bash', 'tmux_window', 'subagent')),
    CHECK (status IN ('fired', 'expired', 'cancelled', 'forgotten')),
    CHECK (bash_status IS NULL OR bash_status IN ('exited', 'killed', 'kill_pending_kernel')),
    CHECK (tmux_status IS NULL OR tmux_status IN ('exit_marker_observed', 'window_killed')),
    CHECK (bash_kill_signal_sent IS NULL OR bash_kill_signal_sent IN ('TERM', 'KILL')),
    CHECK (bash_tail_start_offset IS NULL OR bash_tail_start_offset >= 0),
    CHECK (bash_tail_end_offset IS NULL OR bash_tail_end_offset >= bash_tail_start_offset),
    CHECK (bash_tail_truncated_before IS NULL OR bash_tail_truncated_before IN (0, 1)),
    CHECK ((resource_kind = 'bash' AND status = 'fired') = (bash_tail_start_offset IS NOT NULL AND bash_tail_end_offset IS NOT NULL AND bash_tail_truncated_before IS NOT NULL)),
    CHECK (forgotten_reason IS NULL OR forgotten_reason IN ('handle_missing', 'runtime_unrecoverable_after_restart', 'phoenix_restart', 'cascade_destroyed_handle', 'tmux_handle_missing')),
    CHECK (cancellation_reason IS NULL OR cancellation_reason IN ('explicit_cancel')),
    CHECK (
        (status = 'fired'
            AND (
                (resource_kind = 'bash'
                    AND bash_status IS NOT NULL
                    AND bash_occurred_at IS NOT NULL
                    AND bash_duration_ms IS NOT NULL
                    AND tmux_status IS NULL
                    AND tmux_occurred_at IS NULL
                    AND tmux_server_generation IS NULL
                    AND tmux_exit_code IS NULL
                    AND tmux_duration_ms IS NULL
                    AND forgotten_reason IS NULL
                    AND cancellation_reason IS NULL)
                OR
                (resource_kind = 'tmux_window'
                    AND tmux_status IS NOT NULL
                    AND tmux_occurred_at IS NOT NULL
                    AND tmux_server_generation IS NOT NULL
                    AND (tmux_status = 'window_killed' OR tmux_exit_code IS NOT NULL)
                    AND bash_status IS NULL
                    AND bash_occurred_at IS NULL
                    AND bash_exit_code IS NULL
                    AND bash_duration_ms IS NULL
                    AND bash_signal_number IS NULL
                    AND bash_kill_signal_sent IS NULL
                    AND forgotten_reason IS NULL
                    AND cancellation_reason IS NULL)
            )
        )
        OR
        (status = 'expired'
            AND bash_status IS NULL
            AND bash_occurred_at IS NULL
            AND bash_exit_code IS NULL
            AND bash_duration_ms IS NULL
            AND bash_signal_number IS NULL
            AND bash_kill_signal_sent IS NULL
            AND tmux_status IS NULL
            AND tmux_occurred_at IS NULL
            AND tmux_server_generation IS NULL
            AND tmux_exit_code IS NULL
            AND tmux_duration_ms IS NULL
            AND forgotten_reason IS NULL
            AND cancellation_reason IS NULL)
        OR
        (status = 'cancelled'
            AND bash_status IS NULL
            AND bash_occurred_at IS NULL
            AND bash_exit_code IS NULL
            AND bash_duration_ms IS NULL
            AND bash_signal_number IS NULL
            AND bash_kill_signal_sent IS NULL
            AND tmux_status IS NULL
            AND tmux_occurred_at IS NULL
            AND tmux_server_generation IS NULL
            AND tmux_exit_code IS NULL
            AND tmux_duration_ms IS NULL
            AND forgotten_reason IS NULL
            AND cancellation_reason IS NOT NULL)
        OR
        (status = 'forgotten'
            AND bash_status IS NULL
            AND bash_occurred_at IS NULL
            AND bash_exit_code IS NULL
            AND bash_duration_ms IS NULL
            AND bash_signal_number IS NULL
            AND bash_kill_signal_sent IS NULL
            AND tmux_status IS NULL
            AND tmux_occurred_at IS NULL
            AND tmux_server_generation IS NULL
            AND tmux_exit_code IS NULL
            AND tmux_duration_ms IS NULL
            AND forgotten_reason IS NOT NULL
            AND cancellation_reason IS NULL)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_wake_terminal_receipts_receipt_workflow
    ON wake_terminal_receipts(receipt_id, workflow_id);

CREATE TABLE IF NOT EXISTS wake_terminal_receipt_bash_tail (
    receipt_id TEXT NOT NULL REFERENCES wake_terminal_receipts(receipt_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    stream TEXT,
    offset INTEGER,
    line TEXT NOT NULL,
    PRIMARY KEY (receipt_id, ordinal)
);

CREATE TABLE IF NOT EXISTS wake_terminal_receipt_tmux_tail (
    receipt_id TEXT NOT NULL REFERENCES wake_terminal_receipts(receipt_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    line TEXT NOT NULL,
    PRIMARY KEY (receipt_id, ordinal)
);

CREATE TABLE IF NOT EXISTS wake_inbox_sequences (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= 0)
);

CREATE TABLE IF NOT EXISTS wake_observation_inbox (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    contract_id TEXT NOT NULL,
    terminal_receipt_id TEXT NOT NULL UNIQUE REFERENCES workflow_receipts(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    committed_at TEXT NOT NULL,
    consumed_at TEXT,
    UNIQUE (conversation_id, sequence),
    UNIQUE (id, contract_id, terminal_receipt_id, conversation_id),
    FOREIGN KEY (contract_id, workflow_id)
        REFERENCES wake_workflow_bindings(contract_id, workflow_id)
        ON DELETE CASCADE,
    FOREIGN KEY (terminal_receipt_id, workflow_id)
        REFERENCES wake_terminal_receipts(receipt_id, workflow_id)
        ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS wake_runtime_obligations (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    snapshot_upper_bound INTEGER NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT,
    terminal_reason TEXT,
    UNIQUE (id, conversation_id),
    CHECK (status IN ('owed', 'accepted', 'suppressed')),
    CHECK (
        (status = 'owed' AND resolved_at IS NULL AND terminal_reason IS NULL)
        OR (status = 'accepted' AND resolved_at IS NOT NULL AND terminal_reason = 'accepted')
        OR (status = 'suppressed' AND resolved_at IS NOT NULL AND terminal_reason IN ('suppressed', 'lifecycle_terminal', 'superseded'))
    )
);

CREATE TABLE IF NOT EXISTS wake_runtime_obligation_items (
    obligation_id TEXT NOT NULL REFERENCES wake_runtime_obligations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    inbox_item_id TEXT NOT NULL REFERENCES wake_observation_inbox(id) ON DELETE CASCADE,
    PRIMARY KEY (obligation_id, ordinal),
    UNIQUE (obligation_id, inbox_item_id)
);

CREATE TABLE IF NOT EXISTS wake_shadow_parity (
    id TEXT PRIMARY KEY,
    contract_id TEXT NOT NULL REFERENCES wake_workflow_bindings(contract_id) ON DELETE CASCADE,
    authoritative_contract_id TEXT NOT NULL REFERENCES wake_workflow_bindings(contract_id) ON DELETE CASCADE,
    canonical_kind TEXT NOT NULL,
    profile_detail_kind TEXT NOT NULL,
    expected_codec_family TEXT NOT NULL,
    expected_codec_version INTEGER NOT NULL,
    expected_payload TEXT NOT NULL,
    actual_codec_family TEXT NOT NULL,
    actual_codec_version INTEGER NOT NULL,
    actual_payload TEXT NOT NULL,
    compared_at TEXT NOT NULL,
    equal INTEGER NOT NULL,
    severity TEXT NOT NULL,
    required_action TEXT NOT NULL,
    resolved_at TEXT,
    CHECK (canonical_kind IN ('snapshot', 'transition', 'effect_plan', 'observation', 'receipt', 'reducer_event', 'capability', 'user_projection')),
    CHECK (equal IN (0, 1)),
    CHECK (severity IN ('blocking', 'actionable', 'informational')),
    CHECK (required_action IN ('halt_acceptance', 'retain_authority_and_investigate', 'record_only'))
);

";

const MIGRATION_047: &str = r"
CREATE TABLE IF NOT EXISTS creation_shadow_bindings (
    shadow_workflow_id TEXT PRIMARY KEY REFERENCES workflows(id) ON DELETE CASCADE,
    authoritative_workflow_id TEXT NOT NULL UNIQUE REFERENCES workflows(id) ON DELETE CASCADE,
    creation_job_id TEXT NOT NULL UNIQUE REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    CHECK (shadow_workflow_id <> authoritative_workflow_id)
);

CREATE TABLE IF NOT EXISTS creation_shadow_projections (
    shadow_workflow_id TEXT PRIMARY KEY REFERENCES creation_shadow_bindings(shadow_workflow_id) ON DELETE CASCADE,
    oracle_generation INTEGER NOT NULL,
    oracle_attempt INTEGER NOT NULL,
    projection_status TEXT NOT NULL,
    completion TEXT NOT NULL,
    compensation TEXT NOT NULL,
    hidden INTEGER NOT NULL,
    can_read INTEGER NOT NULL,
    can_write INTEGER NOT NULL,
    can_runtime INTEGER NOT NULL,
    can_cancel INTEGER NOT NULL,
    can_start_over INTEGER NOT NULL,
    can_delete INTEGER NOT NULL,
    projected_at TEXT NOT NULL,
    CHECK (oracle_generation >= 0 AND oracle_attempt >= 0),
    CHECK (projection_status IN ('provisioning', 'failed', 'cancelled', 'deletion_pending', 'ready')),
    CHECK (completion IN ('pending', 'complete', 'failed', 'cancelled', 'deletion_pending')),
    CHECK (compensation IN ('none', 'required_for_cancellation', 'required_for_deletion')),
    CHECK (hidden IN (0, 1)),
    CHECK (can_read IN (0, 1) AND can_write IN (0, 1) AND can_runtime IN (0, 1)
       AND can_cancel IN (0, 1) AND can_start_over IN (0, 1) AND can_delete IN (0, 1))
);

CREATE TABLE IF NOT EXISTS creation_shadow_readiness_effects (
    shadow_workflow_id TEXT NOT NULL REFERENCES creation_shadow_bindings(shadow_workflow_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    effect_number INTEGER NOT NULL,
    PRIMARY KEY (shadow_workflow_id, ordinal),
    UNIQUE (shadow_workflow_id, effect_number)
);

CREATE TABLE IF NOT EXISTS creation_shadow_effect_predictions (
    shadow_workflow_id TEXT NOT NULL REFERENCES creation_shadow_bindings(shadow_workflow_id) ON DELETE CASCADE,
    effect_number INTEGER NOT NULL,
    prediction TEXT NOT NULL,
    PRIMARY KEY (shadow_workflow_id, effect_number),
    CHECK (prediction IN ('completed', 'eligible', 'blocked', 'omitted'))
);

CREATE TABLE IF NOT EXISTS creation_shadow_divergences (
    shadow_workflow_id TEXT NOT NULL REFERENCES creation_shadow_bindings(shadow_workflow_id) ON DELETE CASCADE,
    evidence_identity TEXT NOT NULL,
    expected_value TEXT NOT NULL,
    actual_value TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'blocking' CHECK (severity IN ('blocking', 'actionable', 'informational')),
    required_action TEXT NOT NULL DEFAULT 'retain_authority_and_investigate'
        CHECK (required_action IN ('halt_acceptance', 'retain_authority_and_investigate', 'record_only')),
    recorded_at TEXT NOT NULL,
    resolved_at TEXT,
    PRIMARY KEY (shadow_workflow_id, evidence_identity, recorded_at)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_creation_shadow_one_active_divergence
    ON creation_shadow_divergences(shadow_workflow_id, evidence_identity)
    WHERE resolved_at IS NULL;

CREATE TRIGGER IF NOT EXISTS trg_creation_shadow_binding_delete_shadow
AFTER DELETE ON creation_shadow_bindings
BEGIN
    DELETE FROM workflows WHERE id = OLD.shadow_workflow_id;
END;

CREATE TRIGGER IF NOT EXISTS trg_creation_shadow_job_delete_anchor
BEFORE DELETE ON conversation_creation_jobs
WHEN EXISTS (SELECT 1 FROM creation_shadow_bindings WHERE creation_job_id = OLD.id)
BEGIN
    DELETE FROM workflows
    WHERE id = (
        SELECT authoritative_workflow_id
        FROM creation_shadow_bindings
        WHERE creation_job_id = OLD.id
    );
END;
";

const MIGRATION_048: &str = r"
ALTER TABLE conversation_creation_jobs
    ADD COLUMN shadow_projection_revision INTEGER NOT NULL DEFAULT 0
    CHECK (shadow_projection_revision >= 0);

ALTER TABLE creation_shadow_projections
    ADD COLUMN oracle_revision INTEGER NOT NULL DEFAULT 0
    CHECK (oracle_revision >= 0);

CREATE TRIGGER creation_job_shadow_revision_after_update
AFTER UPDATE OF status, stage, attempt, generation, intent_json, error
ON conversation_creation_jobs
FOR EACH ROW
WHEN NEW.shadow_projection_revision = OLD.shadow_projection_revision
BEGIN
    UPDATE conversation_creation_jobs
    SET shadow_projection_revision = OLD.shadow_projection_revision + 1
    WHERE id = OLD.id;
END;

CREATE TABLE creation_shadow_effect_intents (
    effect_id TEXT PRIMARY KEY REFERENCES workflow_effects(id) ON DELETE CASCADE,
    intent_kind TEXT NOT NULL,
    conversation_id TEXT,
    repository_path TEXT,
    worktree_path TEXT,
    branch_name TEXT,
    message_id TEXT,
    attachment_count INTEGER,
    CHECK (intent_kind IN (
        'resolve_repository', 'reserve_worktree', 'materialize_or_reconcile_worktree',
        'finalize_attachments', 'expand_initial_message', 'commit_metadata',
        'bootstrap_runtime', 'dispatch_initial_llm_request', 'revoke_runtime',
        'remove_owned_worktree', 'release_reservation', 'delete_staged_attachments',
        'finish_cancellation_or_deletion'
    )),
    CHECK (attachment_count IS NULL OR attachment_count >= 0)
);

CREATE TABLE creation_shadow_archives (
    creation_job_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    oracle_revision INTEGER NOT NULL,
    terminal_status TEXT NOT NULL,
    terminal_stage TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    projection_status TEXT,
    completion TEXT,
    compensation TEXT,
    projected_at TEXT,
    archived_at TEXT NOT NULL,
    CHECK (oracle_revision >= 0 AND attempt >= 0 AND generation >= 0),
    CHECK (terminal_status IN ('cancelled', 'deletion_pending', 'ready', 'failed')),
    CHECK (projection_status IS NULL OR projection_status IN ('provisioning', 'failed', 'cancelled', 'deletion_pending', 'ready')),
    CHECK (completion IS NULL OR completion IN ('pending', 'complete', 'failed', 'cancelled', 'deletion_pending')),
    CHECK (compensation IS NULL OR compensation IN ('none', 'required_for_cancellation', 'required_for_deletion'))
);
";

const MIGRATION_049: &str = r"
CREATE TABLE creation_shadow_creation_evidence (
    creation_job_id TEXT PRIMARY KEY REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    cwd TEXT NOT NULL,
    attachment_count INTEGER NOT NULL CHECK (attachment_count >= 0),
    creation_kind TEXT NOT NULL CHECK (creation_kind IN ('initial_turn', 'seeded_empty')),
    accepted_at TEXT NOT NULL
);

INSERT INTO creation_shadow_creation_evidence (creation_job_id, cwd, attachment_count, creation_kind, accepted_at)
SELECT j.id, COALESCE(NULLIF(json_extract(j.intent_json, '$.cwd'), ''), c.cwd),
       COALESCE(json_array_length(json_extract(j.intent_json, '$.files')),
                (SELECT COUNT(*) FROM conversation_creation_job_files f WHERE f.job_id = j.id), 0)
       + COALESCE(json_array_length(json_extract(j.intent_json, '$.images')),
                  (SELECT COUNT(*) FROM conversation_creation_job_images i WHERE i.job_id = j.id), 0),
       CASE WHEN (json_extract(j.intent_json, '$.seed_parent_id') IS NOT NULL
                       OR json_extract(j.intent_json, '$.seed_label') IS NOT NULL)
                      AND trim(COALESCE(json_extract(j.intent_json, '$.text'), '')) = ''
                      AND COALESCE(json_array_length(json_extract(j.intent_json, '$.files')), 0) = 0
                      AND COALESCE(json_array_length(json_extract(j.intent_json, '$.images')), 0) = 0
            THEN 'seeded_empty' ELSE 'initial_turn' END,
       j.accepted_at
FROM conversation_creation_jobs j
JOIN conversations c ON c.id = j.conversation_id;
";

const MIGRATION_050: &str = r"
CREATE TRIGGER creation_reservation_shadow_revision_after_status_update
AFTER UPDATE OF status ON conversation_creation_resource_reservations
FOR EACH ROW
WHEN OLD.status = 'cleanup_required' AND NEW.status = 'released'
BEGIN
    UPDATE conversation_creation_jobs
    SET shadow_projection_revision = shadow_projection_revision + 1
    WHERE id = NEW.job_id;
END;
";

const MIGRATION_051: &str = r"
ALTER TABLE creation_shadow_divergences RENAME TO creation_shadow_divergences_legacy;

CREATE TABLE creation_shadow_divergences (
    shadow_workflow_id TEXT NOT NULL REFERENCES creation_shadow_bindings(shadow_workflow_id) ON DELETE CASCADE,
    evidence_identity TEXT NOT NULL,
    expected_value TEXT NOT NULL,
    actual_value TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'blocking'
        CHECK (severity IN ('blocking', 'actionable', 'informational')),
    required_action TEXT NOT NULL DEFAULT 'retain_authority_and_investigate'
        CHECK (required_action IN ('halt_acceptance', 'retain_authority_and_investigate', 'record_only')),
    recorded_at TEXT NOT NULL,
    resolved_at TEXT,
    PRIMARY KEY (shadow_workflow_id, evidence_identity, recorded_at)
);

INSERT INTO creation_shadow_divergences
    (shadow_workflow_id, evidence_identity, expected_value, actual_value,
     severity, required_action, recorded_at, resolved_at)
SELECT shadow_workflow_id, evidence_identity, expected_value, actual_value,
       CASE severity WHEN 'warning' THEN 'actionable' ELSE severity END,
       CASE required_action
           WHEN 'reconcile_authoritative_projection' THEN 'retain_authority_and_investigate'
           ELSE required_action
       END,
       recorded_at, resolved_at
FROM creation_shadow_divergences_legacy;

DROP TABLE creation_shadow_divergences_legacy;

CREATE UNIQUE INDEX idx_creation_shadow_one_active_divergence
    ON creation_shadow_divergences(shadow_workflow_id, evidence_identity)
    WHERE resolved_at IS NULL;
";

const MIGRATION_052: &str = r"
ALTER TABLE creation_shadow_creation_evidence
    ADD COLUMN uses_worktree INTEGER CHECK (uses_worktree IS NULL OR uses_worktree IN (0, 1));
ALTER TABLE creation_shadow_creation_evidence
    ADD COLUMN branch_name TEXT;
ALTER TABLE creation_shadow_creation_evidence
    ADD COLUMN client_idempotency_key TEXT;
ALTER TABLE conversation_creation_resource_reservations
    ADD COLUMN materialized_at TEXT;
UPDATE conversation_creation_resource_reservations
SET materialized_at = updated_at
WHERE status = 'present';
ALTER TABLE creation_shadow_creation_evidence
    ADD COLUMN requested_mode TEXT
    CHECK (requested_mode IS NULL OR requested_mode IN ('direct', 'managed', 'branch', 'auto'));
";

/// Run all pending migrations against the database.
///
/// Returns the number of migrations applied.
///
/// # Errors
///
/// Returns a [`DbError`] if the underlying database operation fails.
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

    // Track applied migrations by membership, not by a high-water mark: a tool
    // that builds a pre-migrated schema (the dev seeder) stamps a *sparse* set
    // of versions and leaves gaps for the ones it does not reproduce, expecting
    // those to run on first startup. A `MAX(version)` check would treat every
    // gap below the highest stamp as already applied and skip it forever.
    let applied_versions: HashSet<u32> =
        sqlx::query_scalar::<_, u32>("SELECT version FROM _migrations")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let mut applied = 0u32;

    for migration in MIGRATIONS {
        if applied_versions.contains(&migration.version) {
            continue;
        }

        tracing::info!(
            version = migration.version,
            name = migration.name,
            "Applying database migration"
        );

        // Apply the migration body and its version record in one transaction so
        // a crash mid-migration leaves the database all-or-nothing: a partially
        // applied but unrecorded migration would fail to re-run (missing/duplicate
        // column) and abort startup.
        let mut tx = pool.begin().await?;

        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;

        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?, ?)")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        applied += 1;
    }

    let projection_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('creation_shadow_projections')")
            .fetch_all(pool)
            .await?;
    if !projection_columns.is_empty()
        && !projection_columns
            .iter()
            .any(|name| name == "oracle_revision")
    {
        sqlx::query(
            "ALTER TABLE creation_shadow_projections ADD COLUMN oracle_revision INTEGER NOT NULL DEFAULT 0 CHECK (oracle_revision >= 0)",
        )
        .execute(pool)
        .await?;
    }

    let divergence_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('creation_shadow_divergences')")
            .fetch_all(pool)
            .await?;
    if !divergence_columns.is_empty() && !divergence_columns.iter().any(|name| name == "severity") {
        sqlx::query(
            "ALTER TABLE creation_shadow_divergences ADD COLUMN severity TEXT NOT NULL DEFAULT 'blocking' CHECK (severity IN ('blocking', 'actionable', 'informational'))",
        )
        .execute(pool)
        .await?;
    }
    if !divergence_columns.is_empty()
        && !divergence_columns
            .iter()
            .any(|name| name == "required_action")
    {
        sqlx::query(
            "ALTER TABLE creation_shadow_divergences ADD COLUMN required_action TEXT NOT NULL DEFAULT 'retain_authority_and_investigate' CHECK (required_action IN ('halt_acceptance', 'retain_authority_and_investigate', 'record_only'))",
        )
        .execute(pool)
        .await?;
    }

    if applied > 0 {
        tracing::info!(applied, "Database migrations complete");
    }

    Ok(applied)
}

/// Persist first-byte timing metadata for token-bearing turns.
const MIGRATION_031: &str = r"
ALTER TABLE turn_usage ADD COLUMN first_byte_at TEXT;
";

/// Add conversation-level transcript/replica generation with a durable default.
const MIGRATION_033: &str = r"
ALTER TABLE conversations ADD COLUMN transcript_generation INTEGER NOT NULL DEFAULT 1;
";

const MIGRATION_034: &str = r"
CREATE TABLE IF NOT EXISTS global_recall_sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS global_recall_messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES global_recall_sessions(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(session_id, ordinal)
);

CREATE INDEX IF NOT EXISTS idx_global_recall_messages_session_ordinal
    ON global_recall_messages(session_id, ordinal);
";

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
                model TEXT, \
                steering_queue TEXT NOT NULL DEFAULT '[]', \
                state_updated_at TEXT NOT NULL DEFAULT '2025-01-01', \
                created_at TEXT NOT NULL DEFAULT '2025-01-01', \
                updated_at TEXT NOT NULL DEFAULT '2025-01-01'\
            )",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE messages (\
                id INTEGER PRIMARY KEY AUTOINCREMENT, \
                message_id TEXT UNIQUE, \
                conversation_id TEXT NOT NULL, \
                message_type TEXT NOT NULL, \
                content TEXT NOT NULL, \
                sequence_id INTEGER NOT NULL DEFAULT 1, \
                created_at TEXT NOT NULL DEFAULT '2025-01-01'\
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
        assert_eq!(first as usize, MIGRATIONS.len());

        let second = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(second, 0);
    }

    #[tokio::test]
    async fn migration_047_replays_creation_shadow_schema() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        sqlx::raw_sql(
            "DROP TRIGGER trg_creation_shadow_job_delete_anchor;
             DROP TRIGGER trg_creation_shadow_binding_delete_shadow;
             DROP TABLE creation_shadow_divergences;
             DROP TABLE creation_shadow_effect_predictions;
             DROP TABLE creation_shadow_readiness_effects;
             DROP TABLE creation_shadow_projections;
             DROP TABLE creation_shadow_bindings;
             DELETE FROM _migrations WHERE version = 47;",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 0);
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'creation_shadow_%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(tables, 8);
    }

    /// A pre-stamped *highest* version must not suppress lower, un-stamped
    /// migrations. The dev seeder builds a partial schema and stamps a sparse
    /// set of `_migrations` rows (including the latest version), leaving the
    /// migrations it does not reproduce — e.g. 005 (`chain_name` + `chain_qa`)
    /// — un-stamped so they run on first startup. A high-water-mark check would
    /// see `MAX(version)` and skip every gap below it, leaving the schema
    /// missing those columns and the conversation-list query failing with
    /// `no such column: chain_name`.
    #[tokio::test]
    async fn sparse_stamp_does_not_suppress_lower_migrations() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Create the ledger and stamp a single high version, mimicking a
        // seeder that left every other migration un-stamped.
        sqlx::raw_sql(
            "CREATE TABLE _migrations (\
                version INTEGER PRIMARY KEY, \
                name TEXT NOT NULL, \
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))\
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (29, 'drop_conv_mode_blob')")
            .execute(&pool)
            .await
            .unwrap();

        // Every version except the stamped 29 must run.
        let applied = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(applied as usize, MIGRATIONS.len() - 1);

        // Migration 005's effects must be present.
        let cols: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get::<String, _>("name"))
            .collect();
        assert!(
            cols.iter().any(|c| c == "chain_name"),
            "chain_name must be added by un-stamped migration 005, got: {cols:?}"
        );
        let chain_qa_exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='chain_qa'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(chain_qa_exists.as_deref(), Some("chain_qa"));

        // Re-running is a no-op now that the ledger is complete.
        let again = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(again, 0);
    }

    /// Migration 028: the `conv_mode` JSON blob is projected into the `cm_*`
    /// columns per variant; a malformed/absent blob leaves `cm_kind` NULL (read
    /// path defaults to explore).
    #[tokio::test]
    async fn migration_028_projects_conv_mode_into_columns() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        let rows: &[(&str, &str)] = &[
            (
                "c-explore",
                r#"{"mode":"Explore","worktree_path":"/wt/e","next_taskmd_id_hint":"42001"}"#,
            ),
            ("c-direct", r#"{"mode":"Direct"}"#),
            (
                "c-work",
                r#"{"mode":"Work","branch_name":"b","worktree_path":"/wt/w","base_branch":"main","task_id":"T1","task_title":"Fix it"}"#,
            ),
            (
                "c-branch",
                r#"{"mode":"Branch","branch_name":"fix","worktree_path":"/wt/b","base_branch":"main"}"#,
            ),
        ];
        for (id, cm) in rows {
            sqlx::query(
                "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
                 VALUES (?1, ?2, '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(cm)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_pending_migrations(&pool).await.unwrap();

        let fetch = |id: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "SELECT cm_kind, cm_branch_name, cm_worktree_path, cm_base_branch, \
                            cm_task_id, cm_task_title, cm_next_taskmd_id_hint \
                     FROM conversations WHERE id = ?1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let get = |row: &sqlx::sqlite::SqliteRow, c: &str| row.get::<Option<String>, _>(c);

        let e = fetch("c-explore").await;
        assert_eq!(get(&e, "cm_kind").as_deref(), Some("explore"));
        assert_eq!(get(&e, "cm_worktree_path").as_deref(), Some("/wt/e"));
        assert_eq!(get(&e, "cm_next_taskmd_id_hint").as_deref(), Some("42001"));
        assert!(get(&e, "cm_branch_name").is_none());

        let d = fetch("c-direct").await;
        assert_eq!(get(&d, "cm_kind").as_deref(), Some("direct"));
        assert!(get(&d, "cm_worktree_path").is_none());

        let w = fetch("c-work").await;
        assert_eq!(get(&w, "cm_kind").as_deref(), Some("work"));
        assert_eq!(get(&w, "cm_branch_name").as_deref(), Some("b"));
        assert_eq!(get(&w, "cm_base_branch").as_deref(), Some("main"));
        assert_eq!(get(&w, "cm_task_id").as_deref(), Some("T1"));
        assert_eq!(get(&w, "cm_task_title").as_deref(), Some("Fix it"));

        let b = fetch("c-branch").await;
        assert_eq!(get(&b, "cm_kind").as_deref(), Some("branch"));
        assert_eq!(get(&b, "cm_branch_name").as_deref(), Some("fix"));
        assert!(get(&b, "cm_task_id").is_none());
    }

    /// Migration 027: pending steering entries are extracted from the
    /// `steering_queue` blob (both envelope and bare-array shapes) into
    /// `steering_messages` + grandchild attachment tables, preserving FIFO
    /// order and the `skill_invocation` trio.
    #[tokio::test]
    async fn migration_027_backfills_steering_message_tables() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // c-env: versioned envelope, two entries — first with a file + image,
        // second a skill invocation.
        let env = r#"{"v":"v1","entries":[
            {"text":"first","llm_text":"first-expanded","message_id":"s1","user_agent":"UA",
             "images":[{"data":"IMG","media_type":"image/png"}],
             "files":[{"original_name":"a.txt","media_type":"text/plain","size_bytes":9,"stored_path":"/p/a"}],
             "skill_invocation":null},
            {"text":"second","llm_text":null,"message_id":"s2","user_agent":null,
             "images":[],"files":[],
             "skill_invocation":{"name":"build","body":"BODY","skill_dir":"/skills/build"}}
        ]}"#;
        // c-bare: pre-envelope bare array, one plain entry.
        let bare = r#"[{"text":"legacy","llm_text":null,"message_id":"s3","user_agent":null,"images":[],"files":[]}]"#;
        for (id, q) in [("c-env", env), ("c-bare", bare), ("c-empty", "[]")] {
            sqlx::query(
                "INSERT INTO conversations (id, steering_queue, state_updated_at, created_at, updated_at) \
                 VALUES (?1, ?2, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(q)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_pending_migrations(&pool).await.unwrap();

        // steering_messages: FIFO order (conversation_id, ordinal, message_id, text).
        let msgs: Vec<(String, i64, String, String)> = sqlx::query(
            "SELECT conversation_id, ordinal, message_id, text \
             FROM steering_messages ORDER BY conversation_id, ordinal",
        )
        .map(|r: sqlx::sqlite::SqliteRow| {
            (
                r.get::<String, _>("conversation_id"),
                r.get::<i64, _>("ordinal"),
                r.get::<String, _>("message_id"),
                r.get::<String, _>("text"),
            )
        })
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            msgs,
            vec![
                ("c-bare".into(), 0, "s3".into(), "legacy".into()),
                ("c-env".into(), 0, "s1".into(), "first".into()),
                ("c-env".into(), 1, "s2".into(), "second".into()),
            ]
        );

        // Scalar columns: llm_text preserved on s1, skill trio only on s2.
        let s1_llm: Option<String> =
            sqlx::query_scalar("SELECT llm_text FROM steering_messages WHERE message_id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(s1_llm.as_deref(), Some("first-expanded"));
        let s1_skill: Option<String> =
            sqlx::query_scalar("SELECT skill_name FROM steering_messages WHERE message_id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(s1_skill.is_none());
        let s2_skill: Option<String> =
            sqlx::query_scalar("SELECT skill_name FROM steering_messages WHERE message_id = 's2'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(s2_skill.as_deref(), Some("build"));

        // Skill trio fully populated for s2.
        let skill: (Option<String>, Option<String>) = sqlx::query(
            "SELECT skill_body, skill_dir FROM steering_messages WHERE message_id = 's2'",
        )
        .map(|r: sqlx::sqlite::SqliteRow| {
            (
                r.get::<Option<String>, _>("skill_body"),
                r.get::<Option<String>, _>("skill_dir"),
            )
        })
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(skill, (Some("BODY".into()), Some("/skills/build".into())));

        // Grandchild files/images for s1.
        let file: (String, i64, String) = sqlx::query(
            "SELECT original_name, size_bytes, stored_path FROM steering_message_files WHERE message_id = 's1'",
        )
        .map(|r: sqlx::sqlite::SqliteRow| {
            (r.get::<String, _>("original_name"), r.get::<i64, _>("size_bytes"), r.get::<String, _>("stored_path"))
        })
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(file, ("a.txt".into(), 9, "/p/a".into()));

        let img_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_message_images WHERE message_id = 's1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(img_count, 1);

        // Empty queue contributes nothing.
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total, 3);
    }

    /// Migration 027 must be at least as tolerant as the decoder it replaced: a
    /// corrupt/duplicate/invalid legacy `steering_queue` must not abort the
    /// migration (which would brick startup). Malformed-JSON rows are skipped,
    /// duplicate `message_id`s keep the first FIFO occurrence, and entries
    /// missing required fields are dropped — without raising.
    #[tokio::test]
    async fn migration_027_tolerates_corrupt_duplicate_and_invalid_queues() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // c-bad: not valid JSON at all.
        // c-scalar: valid array but a non-object element + an object whose
        //   `files` array holds a scalar element (both must be skipped, not raise).
        // c-dup: same message_id twice (first FIFO occurrence wins).
        // c-missing: an entry with no `text` (dropped) alongside a valid one.
        // c-partial-skill: skill_invocation missing `body` → skill_* left NULL.
        let scalar =
            r#"["bad",{"text":"ok2","message_id":"m-scalar","images":[],"files":["junk"]}]"#;
        let dup = r#"[{"text":"a","message_id":"d1","images":[],"files":[]},
                      {"text":"b","message_id":"d1","images":[],"files":[]}]"#;
        let missing = r#"[{"message_id":"m-notext","images":[],"files":[]},
                          {"text":"ok","message_id":"m-ok","images":[],"files":[]}]"#;
        let partial = r#"[{"text":"s","message_id":"p1","images":[],"files":[],
                           "skill_invocation":{"name":"build","skill_dir":"/d"}}]"#;
        for (id, q) in [
            ("c-bad", "{not json"),
            ("c-scalar", scalar),
            ("c-dup", dup),
            ("c-missing", missing),
            ("c-partial-skill", partial),
        ] {
            sqlx::query(
                "INSERT INTO conversations (id, steering_queue, state_updated_at, created_at, updated_at) \
                 VALUES (?1, ?2, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(q)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Must not raise despite the corrupt/scalar/duplicate/invalid rows.
        run_pending_migrations(&pool).await.unwrap();

        let ids = |conv: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT message_id FROM steering_messages WHERE conversation_id = ?1 ORDER BY ordinal",
                )
                .bind(conv)
                .fetch_all(&pool)
                .await
                .unwrap()
            }
        };

        // Malformed JSON → no rows (skipped, not aborted).
        assert!(ids("c-bad").await.is_empty());
        // Non-object array element skipped; the valid object survives with no
        // file rows (its scalar `files` element is dropped, not extracted).
        assert_eq!(ids("c-scalar").await, vec!["m-scalar".to_string()]);
        let scalar_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM steering_message_files WHERE message_id = 'm-scalar'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(scalar_files, 0);
        // Duplicate message_id → exactly one row kept.
        assert_eq!(ids("c-dup").await, vec!["d1".to_string()]);
        // Missing-text entry dropped, valid one kept.
        assert_eq!(ids("c-missing").await, vec!["m-ok".to_string()]);

        // Partial skill trio → all skill_* columns NULL (CHECK satisfied).
        let skill_name: Option<String> =
            sqlx::query_scalar("SELECT skill_name FROM steering_messages WHERE message_id = 'p1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(skill_name.is_none());

        // The legacy blob is cleared on every migrated row (no parallel rep).
        let dirty: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE steering_queue <> '[]'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(dirty, 0);
    }

    /// Migration 025: attachments embedded in `messages.content` are extracted
    /// into `message_files` / `message_images` preserving order (`ordinal`).
    /// Skill rows contribute files only (no images field); the blob is left
    /// intact at this phase (the strip lands with the read/write cutover).
    #[tokio::test]
    async fn migration_025_backfills_attachment_tables_from_content() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c', '{\"mode\":\"Explore\"}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // user message with two files (ordered) and one image.
        let user_content = r#"{"text":"hi","images":[{"data":"AAA","media_type":"image/png"}],"files":[{"original_name":"a.txt","media_type":"text/plain","size_bytes":1,"stored_path":"/p/a"},{"original_name":"b.txt","media_type":"text/plain","size_bytes":2,"stored_path":"/p/b"}]}"#;
        // skill message with one file, no images field.
        let skill_content = r#"{"name":"build","body":"b","trigger":"/build","files":[{"original_name":"s.txt","media_type":"text/plain","size_bytes":3,"stored_path":"/p/s"}]}"#;
        // user message with no attachments.
        let plain_content = r#"{"text":"plain","images":[],"files":[]}"#;
        for (id, mt, content) in [
            ("m-user", "user", user_content),
            ("m-skill", "skill", skill_content),
            ("m-plain", "user", plain_content),
        ] {
            sqlx::query(
                "INSERT INTO messages (message_id, conversation_id, message_type, content) \
                 VALUES (?1, 'c', ?2, ?3)",
            )
            .bind(id)
            .bind(mt)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_pending_migrations(&pool).await.unwrap();

        // Files: two for m-user (ordered), one for m-skill, none for m-plain.
        let files: Vec<(String, i64, String, String)> = sqlx::query(
            "SELECT message_id, ordinal, original_name, stored_path FROM message_files \
             ORDER BY message_id, ordinal",
        )
        .map(|row: sqlx::sqlite::SqliteRow| {
            (
                row.get::<String, _>("message_id"),
                row.get::<i64, _>("ordinal"),
                row.get::<String, _>("original_name"),
                row.get::<String, _>("stored_path"),
            )
        })
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            files,
            vec![
                ("m-skill".into(), 0, "s.txt".into(), "/p/s".into()),
                ("m-user".into(), 0, "a.txt".into(), "/p/a".into()),
                ("m-user".into(), 1, "b.txt".into(), "/p/b".into()),
            ]
        );

        // Images: one for m-user, none for skill/plain.
        let images: Vec<(String, i64, String, String)> = sqlx::query(
            "SELECT message_id, ordinal, media_type, data FROM message_images ORDER BY message_id, ordinal",
        )
        .map(|row: sqlx::sqlite::SqliteRow| {
            (
                row.get::<String, _>("message_id"),
                row.get::<i64, _>("ordinal"),
                row.get::<String, _>("media_type"),
                row.get::<String, _>("data"),
            )
        })
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            images,
            vec![("m-user".into(), 0, "image/png".into(), "AAA".into())]
        );

        // size_bytes round-trips as an integer.
        let size: i64 = sqlx::query_scalar(
            "SELECT size_bytes FROM message_files WHERE message_id = 'm-user' AND ordinal = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(size, 2);

        // Migration 026 (run in the same pass) strips the now-redundant blob
        // keys: content keeps its scalar fields but no longer carries
        // files/images.
        let content_for = |id: &'static str| {
            let pool = pool.clone();
            async move {
                let s: String =
                    sqlx::query_scalar("SELECT content FROM messages WHERE message_id = ?1")
                        .bind(id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                serde_json::from_str::<serde_json::Value>(&s).unwrap()
            }
        };
        let user = content_for("m-user").await;
        assert!(user.get("files").is_none() && user.get("images").is_none());
        assert_eq!(user["text"], "hi");
        let skill = content_for("m-skill").await;
        assert!(skill.get("files").is_none());
        assert_eq!(skill["name"], "build");
        let plain = content_for("m-plain").await;
        assert!(plain.get("files").is_none() && plain.get("images").is_none());
    }

    #[tokio::test]
    async fn migration_021_removes_null_explore_taskmd_id_hint() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('c-hint', '{\"mode\":\"Explore\",\"next_taskmd_id_hint\":null}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        // After normalization (migration 028), the null hint lands as a NULL
        // cm_next_taskmd_id_hint column.
        let hint: Option<String> = sqlx::query_scalar(
            "SELECT cm_next_taskmd_id_hint FROM conversations WHERE id = 'c-hint'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(hint.is_none());
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

        // 001 rewrites the blob Standalone -> Direct; 028 normalizes it to the
        // cm_kind discriminator.
        let kind: Option<String> =
            sqlx::query_scalar("SELECT cm_kind FROM conversations WHERE id = 'c1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind.as_deref(), Some("direct"));
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

        let row = sqlx::query(
            "SELECT cm_kind, cm_worktree_path, state FROM conversations WHERE id = 'c2'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let kind: Option<String> = row.get("cm_kind");
        let worktree: Option<String> = row.get("cm_worktree_path");
        let state: String = row.get("state");
        // Migration 002 reverts to Explore. Migration 007 only backfills
        // `worktree_path` from cwd when cwd matches the canonical managed-
        // worktree layout `.phoenix/worktrees/{id}`; cwd `/tmp` does not
        // match, so the row stays a bare Explore with no worktree. This is
        // load-bearing: backfilling `/tmp` here would let two demoted convs
        // share the same worktree-scoped tmux socket on cascade.
        assert_eq!(kind.as_deref(), Some("explore"));
        assert!(worktree.is_none());
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

        let kind: Option<String> =
            sqlx::query_scalar("SELECT cm_kind FROM conversations WHERE id = 'c3'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind.as_deref(), Some("direct"));
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

        // 002 reverts to Explore; 007 leaves it alone (cwd `/tmp` is not
        // the canonical managed-worktree layout `.phoenix/worktrees/{id}`).
        let kind: Option<String> =
            sqlx::query_scalar("SELECT cm_kind FROM conversations WHERE id = 'c4'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kind.as_deref(), Some("explore"));
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

        let row =
            sqlx::query("SELECT cm_kind, cm_task_title, state FROM conversations WHERE id = 'c5'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let kind: Option<String> = row.get("cm_kind");
        let task_title: Option<String> = row.get("cm_task_title");
        let state: String = row.get("state");
        assert_eq!(
            kind.as_deref(),
            Some("work"),
            "Valid Work should be preserved"
        );
        assert_eq!(task_title.as_deref(), Some("Fix it"));
        assert!(
            state.contains("tool_executing"),
            "State should be preserved: {state}"
        );
    }

    /// Migration 35: creates the durable async conversation-creation job table.
    #[tokio::test]
    async fn migration_035_creates_conversation_creation_jobs_table() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        run_pending_migrations(&pool).await.unwrap();

        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversation_creation_jobs)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(
            columns.iter().any(|c| c == "conversation_id"),
            "Expected conversation_creation_jobs table to exist after migration 35; got {columns:?}"
        );

        let due_index: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_creation_jobs_due'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(due_index, vec!["idx_creation_jobs_due".to_string()]);
    }

    #[tokio::test]
    async fn migration_036_creates_creation_job_files_table() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        run_pending_migrations(&pool).await.unwrap();

        let columns: Vec<String> =
            sqlx::query("PRAGMA table_info(conversation_creation_job_files)")
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("name"))
                .collect();
        assert!(
            columns.iter().any(|c| c == "stored_path"),
            "Expected conversation_creation_job_files table to exist after migration 36; got {columns:?}"
        );
    }

    #[tokio::test]
    async fn migration_037_creates_creation_job_images_table() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        run_pending_migrations(&pool).await.unwrap();

        let columns: Vec<String> =
            sqlx::query("PRAGMA table_info(conversation_creation_job_images)")
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("name"))
                .collect();
        assert!(
            columns.iter().any(|c| c == "data"),
            "Expected conversation_creation_job_images table to exist after migration 37; got {columns:?}"
        );
    }

    #[tokio::test]
    async fn migration_035_adds_fenced_creation_protocol_constraints() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        let columns: Vec<String> = sqlx::query("PRAGMA table_info(conversation_creation_jobs)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        for expected in [
            "status",
            "stage",
            "attempt",
            "generation",
            "claim_worker_id",
            "claim_token",
            "lease_until",
            "next_attempt_at",
        ] {
            assert!(columns.iter().any(|column| column == expected));
        }

        let invalid = sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                id, conversation_id, status, stage, attempt, generation,
                intent_json, accepted_at, created_at, updated_at
             ) VALUES ('job-invalid', 'missing-conversation', 'claimed',
                       'validate_intent', 1, 1, '{}', 'now', 'now', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(
            invalid.is_err(),
            "claimed row without authority must be rejected"
        );
    }

    #[tokio::test]
    async fn migration_036_adds_creation_resource_reservations() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        let columns: Vec<String> =
            sqlx::query("PRAGMA table_info(conversation_creation_resource_reservations)")
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("name"))
                .collect();
        for expected in [
            "job_id",
            "generation",
            "repository_identity",
            "resource_identity",
            "status",
        ] {
            assert!(columns.iter().any(|column| column == expected));
        }
    }

    /// Migration 003 (REQ-BED-030): adds a nullable `continued_in_conv_id`
    /// column on `conversations`. Existing rows default to NULL and the column
    /// should be queryable via `PRAGMA table_info` after migration.
    #[tokio::test]
    async fn migrations_041_to_044_create_expected_tables() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        for table in [
            "workflow_protocol_selections",
            "workflow_profile_codecs",
            "workflow_profile_executors",
            "external_acceptance_bindings",
            "workflows",
            "workflow_transitions",
            "workflow_effects",
            "workflow_effect_dependencies",
            "workflow_barriers",
            "workflow_barrier_members",
            "workflow_claims",
            "workflow_attempts",
            "workflow_observations",
            "workflow_stale_observations",
            "workflow_receipts",
            "workflow_reducer_inbox",
            "workflow_owed_acceptance",
            "workflow_manual_resolutions",
            "workflow_manual_resolution_choices",
            "workflow_shadow_divergences",
            "wake_registration_fences",
            "wake_workflow_bindings",
            "wake_terminal_receipts",
            "wake_terminal_receipt_bash_tail",
            "wake_terminal_receipt_tmux_tail",
            "wake_observation_inbox",
            "wake_runtime_obligations",
            "wake_runtime_obligation_items",
            "wake_shadow_parity",
            "creation_shadow_bindings",
            "creation_shadow_projections",
            "creation_shadow_readiness_effects",
            "creation_shadow_effect_predictions",
            "creation_shadow_divergences",
        ] {
            let exists: Option<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
            )
            .bind(table)
            .fetch_optional(&pool)
            .await
            .unwrap();
            assert_eq!(exists.as_deref(), Some(table), "missing table {table}");
        }
    }

    async fn seed_workflow_stack(pool: &SqlitePool) {
        seed_workflow_stack_at_version(pool, 1).await;
    }

    async fn seed_workflow_stack_at_version(pool: &SqlitePool, workflow_version: i64) {
        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at) \
             VALUES ('sel', 'wake', 'wake-selector', 1, 1, 'engine_protocol', 1, 1, 1, 'now')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES ('wf', 'wake', 1, 'engine_protocol', 'authoritative', NULL, 'sel', ?1, 0, 'active', 'snapshot', 1, '{}', 'now')",
        )
        .bind(workflow_version)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_transitions \
             (id, workflow_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) \
             VALUES ('tr', 'wf', 0, 1, 0, 'event', 1, '{}', 'now')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects \
             (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status) \
             VALUES ('eff', 'wf', 'tr', 1, 0, 'wake', 'observe_handle', 'effect', 1, 'required', 'observable_reconciliation', '{}', 'eligible')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_effects \
             (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status) \
             VALUES ('eff-other', 'wf', 'tr', 1, 0, 'wake', 'register', 'effect', 1, 'required', 'observable_reconciliation', '{}', 'eligible')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_attempts \
             (id, effect_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, ordinal, status, begun_at) \
             VALUES ('att', 'eff', 'wf', 1, 0, 'claim-1', 'worker-1', 'later', 'now', 0, 'begun', 'now')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_attempts \
             (id, effect_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, ordinal, status, begun_at) \
             VALUES ('att-other', 'eff-other', 'wf', 1, 0, 'claim-2', 'worker-2', 'later', 'now', 0, 'begun', 'now')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_receipts \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, origin, accepted_at) \
             VALUES ('rcpt', 'eff', 'att', 'wf', 1, 0, 'claim-1', 'worker-1', 'later', 'now', 'receipt', 1, '{}', 'execution', 'now')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_receipts \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, origin, accepted_at) \
             VALUES ('rcpt-other', 'eff-other', 'att-other', 'wf', 1, 0, 'claim-2', 'worker-2', 'later', 'now', 'receipt', 1, '{}', 'execution', 'now')",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn migration_041_enforces_selection_authority_tuple_and_accepting_uniqueness() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at) \
             VALUES ('sel-1', 'wake', 'wake-selector', 1, 1, 'engine_protocol', 1, 1, 0, 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let dup_accepting = sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at) \
             VALUES ('sel-2', 'wake', 'wake-selector-2', 1, 2, 'legacy_protocol', 1, 0, 0, 'now')",
        )
        .execute(&pool)
        .await;
        assert!(dup_accepting.is_err());

        let dup_authority_tuple = sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at, drained_at) \
             VALUES ('sel-3', 'wake', 'wake-selector', 1, 1, 'engine_protocol', 0, 1, 0, 'now', 'later')",
        )
        .execute(&pool)
        .await;
        assert!(dup_authority_tuple.is_err());
    }

    #[tokio::test]
    async fn migration_042_enforces_selection_workflow_and_external_binding_coherence() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at) \
             VALUES ('sel-a', 'creation', 'create', 1, 1, 'engine_protocol', 1, 1, 1, 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at, drained_at) \
             VALUES ('sel-b', 'creation', 'create', 2, 2, 'legacy_protocol', 0, 0, 0, 'now', 'later')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mismatched_workflow = sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES ('wf-bad', 'creation', 1, 'engine_protocol', 'authoritative', NULL, 'sel-b', 1, 0, 'active', 'snapshot', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(mismatched_workflow.is_err());

        sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES ('wf-ok', 'creation', 1, 'engine_protocol', 'authoritative', NULL, 'sel-a', 1, 0, 'active', 'snapshot', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO external_acceptance_bindings \
             (id, selection_id, profile_id, protocol_version, authority, authority_scope, idempotency_key, intent_fingerprint, workflow_id, receipt_codec_family, receipt_codec_version, receipt_payload, accepted_at) \
             VALUES ('bind-1', 'sel-a', 'creation', 1, 'engine_protocol', 'repo:1', 'idem-1', 'fp-a', 'wf-ok', 'handle', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let replay = sqlx::query(
            "INSERT INTO external_acceptance_bindings \
             (id, selection_id, profile_id, protocol_version, authority, authority_scope, idempotency_key, intent_fingerprint, workflow_id, receipt_codec_family, receipt_codec_version, receipt_payload, accepted_at) \
             VALUES ('bind-2', 'sel-a', 'creation', 1, 'engine_protocol', 'repo:1', 'idem-1', 'fp-b', 'wf-ok', 'handle', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(replay.is_err());
    }

    #[tokio::test]
    async fn migration_042_rejects_external_binding_to_wrong_workflow_tuple() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at) \
             VALUES ('sel-a', 'creation', 'create', 1, 1, 'engine_protocol', 1, 1, 1, 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at, drained_at) \
             VALUES ('sel-b', 'other-profile', 'create', 2, 1, 'engine_protocol', 0, 1, 1, 'now', 'later')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES ('wf-other', 'other-profile', 1, 'engine_protocol', 'authoritative', NULL, 'sel-b', 1, 0, 'active', 'snapshot', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let wrong_workflow = sqlx::query(
            "INSERT INTO external_acceptance_bindings \
             (id, selection_id, profile_id, protocol_version, authority, authority_scope, idempotency_key, intent_fingerprint, workflow_id, receipt_codec_family, receipt_codec_version, receipt_payload, accepted_at) \
             VALUES ('bind-wrong', 'sel-a', 'creation', 1, 'engine_protocol', 'repo:1', 'idem-wrong', 'fp-a', 'wf-other', 'handle', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(wrong_workflow.is_err());
    }

    #[tokio::test]
    async fn migration_042_enforces_attempt_observation_and_receipt_exact_effect_claim_context() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        let bad_attempt = sqlx::query(
            "INSERT INTO workflow_attempts \
             (id, effect_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, ordinal, status, begun_at) \
             VALUES ('att-bad', 'eff', 'wf', 0, 0, 'claim-2', 'worker-1', 'later', 'now', 1, 'begun', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(bad_attempt.is_err());

        sqlx::query(
            "INSERT INTO workflow_observations \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, observed_at, recorded_at, authoritative) \
             VALUES ('obs-1', 'eff', 'att', 'wf', 1, 0, 'claim-1', 'worker-1', 'later', 'now', 'obs', 1, '{}', 'now', 'now', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let bad_observation = sqlx::query(
            "INSERT INTO workflow_observations \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, observed_at, recorded_at, authoritative) \
             VALUES ('obs-bad', 'eff', 'att', 'wf', 1, 99, 'claim-1', 'worker-1', 'later', 'now', 'obs', 1, '{}', 'now', 'now', 1)",
        )
        .execute(&pool)
        .await;
        assert!(bad_observation.is_err());

        let bad_receipt = sqlx::query(
            "INSERT INTO workflow_receipts \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, origin, accepted_at) \
             VALUES ('rcpt-bad', 'eff', 'att', 'wf', 1, 0, 'other-claim', 'worker-1', 'later', 'now', 'receipt', 1, '{}', 'execution', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(bad_receipt.is_err());
    }

    #[tokio::test]
    async fn migration_042_retains_historical_effect_after_workflow_version_advances() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack_at_version(&pool, 2).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects WHERE id = 'eff' AND workflow_id = 'wf' AND declared_workflow_version = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn migration_042_workflow_effect_pending_reconciliation_defaults_false() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        let pending: i64 = sqlx::query_scalar(
            "SELECT pending_reconciliation FROM workflow_effects WHERE id = 'eff'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn migration_042_workflow_effect_pending_reconciliation_rejects_non_boolean_values() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        let bad_insert = sqlx::query(
            "INSERT INTO workflow_effects \
             (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, \
              family, kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, \
              status, pending_reconciliation, next_eligible_at, destructive_resource) \
             VALUES ('eff-bad-pending', 'wf', 'tr', 1, 0, 'wake', 'register', 'intent', 1, 'required', \
                     'observable_reconciliation', '{}', 'eligible', 2, NULL, NULL)",
        )
        .execute(&pool)
        .await;
        assert!(bad_insert.is_err());
    }

    #[tokio::test]
    async fn migration_042_accepts_manual_receipt_without_attempt_but_rejects_execution_without_attempt(
    ) {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        sqlx::query("DELETE FROM workflow_receipts WHERE id = 'rcpt'")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO workflow_receipts \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, origin, accepted_at) \
             VALUES ('rcpt-manual', 'eff', NULL, 'wf', 1, 0, NULL, NULL, NULL, NULL, 'receipt', 1, '{}', 'manual', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let execution_without_attempt = sqlx::query(
            "INSERT INTO workflow_receipts \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, origin, accepted_at) \
             VALUES ('rcpt-exec-bad', 'eff', NULL, 'wf', 1, 0, NULL, NULL, NULL, NULL, 'receipt', 1, '{}', 'execution', 'now')",
        )
        .execute(&pool)
        .await;
        assert!(execution_without_attempt.is_err());
    }

    #[tokio::test]
    async fn migration_042_enforces_observation_authoritative_true_only() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        let non_authoritative = sqlx::query(
            "INSERT INTO workflow_observations \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, observed_at, recorded_at, authoritative) \
             VALUES ('obs-bad-auth', 'eff', 'att', 'wf', 1, 0, 'claim-1', 'worker-1', 'later', 'now', 'obs', 1, '{}', 'now', 'now', 0)",
        )
        .execute(&pool)
        .await;
        assert!(non_authoritative.is_err());
    }

    #[tokio::test]
    async fn migration_042_persists_barrier_event_tuple() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        sqlx::query(
            "INSERT INTO workflow_barriers \
             (id, workflow_id, declaring_transition_id, declaring_workflow_version, status, satisfied_at, event_codec_family, event_codec_version, event_payload) \
             VALUES ('bar', 'wf', 'tr', 1, 'waiting', NULL, 'barrier-event', 1, '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT event_codec_family, event_codec_version, event_payload \
             FROM workflow_barriers WHERE id = 'bar'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("event_codec_family"), "barrier-event");
        assert_eq!(row.get::<i64, _>("event_codec_version"), 1);
        assert_eq!(row.get::<String, _>("event_payload"), "{}");

        let missing_event_tuple = sqlx::query(
            "INSERT INTO workflow_barriers \
             (id, workflow_id, declaring_transition_id, declaring_workflow_version, status, satisfied_at) \
             VALUES ('bar-missing', 'wf', 'tr', 1, 'waiting', NULL)",
        )
        .execute(&pool)
        .await;
        assert!(missing_event_tuple.is_err());
    }

    #[tokio::test]
    async fn migration_042_requires_owed_acceptance_event_tuple_and_nonempty_source_kind() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, event_payload, delivery_status, consumed_by_transition_id) \
             VALUES ('inbox-owed', 'wf', 'rcpt', NULL, 'event', 1, '{}', 'pending', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_owed_acceptance \
             (id, workflow_id, reducer_inbox_id, source_kind, event_codec_family, event_codec_version, event_payload, status, resolving_transition_id, suppression_reason) \
             VALUES ('owed-ok', 'wf', 'inbox-owed', 'receipt', 'owed-event', 1, '{}', 'owed', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let empty_source_kind = sqlx::query(
            "INSERT INTO workflow_owed_acceptance \
             (id, workflow_id, reducer_inbox_id, source_kind, event_codec_family, event_codec_version, event_payload, status, resolving_transition_id, suppression_reason) \
             VALUES ('owed-empty-source', 'wf', 'inbox-empty-source', '', 'owed-event', 1, '{}', 'owed', NULL, NULL)",
        )
        .execute(&pool)
        .await;
        assert!(empty_source_kind.is_err());

        let missing_event_tuple = sqlx::query(
            "INSERT INTO workflow_owed_acceptance \
             (id, workflow_id, reducer_inbox_id, source_kind, status, resolving_transition_id, suppression_reason) \
             VALUES ('owed-missing-event', 'wf', 'inbox-missing-event', 'receipt', 'owed', NULL, NULL)",
        )
        .execute(&pool)
        .await;
        assert!(missing_event_tuple.is_err());
    }

    #[tokio::test]
    async fn migration_042_shadow_divergence_codec_tuples_are_all_null_or_all_present() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO workflow_protocol_selections \
             (id, profile_id, selector_identity, selector_version, protocol_version, authority, accepting, runtime_acceptance_enabled, external_acceptance_enabled, registered_at) \
             VALUES ('sel-shadow', 'wake', 'wake-selector', 1, 1, 'engine_protocol', 1, 1, 1, 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES ('wf-auth', 'wake', 1, 'engine_protocol', 'authoritative', NULL, 'sel-shadow', 1, 0, 'active', 'snapshot', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflows \
             (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, protocol_selection_id, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, accepted_at) \
             VALUES ('wf-shadow', 'wake', 1, 'engine_protocol', 'shadow', 'wf-auth', 'sel-shadow', 1, 0, 'active', 'snapshot', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workflow_shadow_divergences \
             (id, shadow_workflow_id, authoritative_workflow_id, kind, profile_detail_kind, severity, required_action, evidence_identity, resolved_at, expected_codec_family, expected_codec_version, expected_payload, actual_codec_family, actual_codec_version, actual_payload, recorded_at) \
             VALUES ('div-null', 'wf-shadow', 'wf-auth', 'snapshot', 'wake_terminal', 'informational', 'record_only', 'ev-1', NULL, NULL, NULL, NULL, 'actual', 1, '{}', 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let partial_expected = sqlx::query(
            "INSERT INTO workflow_shadow_divergences \
             (id, shadow_workflow_id, authoritative_workflow_id, kind, profile_detail_kind, severity, required_action, evidence_identity, resolved_at, expected_codec_family, expected_codec_version, expected_payload, actual_codec_family, actual_codec_version, actual_payload, recorded_at) \
             VALUES ('div-bad-expected', 'wf-shadow', 'wf-auth', 'snapshot', 'wake_terminal', 'informational', 'record_only', 'ev-2', NULL, 'expected', NULL, NULL, NULL, NULL, NULL, 'now')",
        )
        .execute(&pool)
        .await;
        assert!(partial_expected.is_err());

        let partial_actual = sqlx::query(
            "INSERT INTO workflow_shadow_divergences \
             (id, shadow_workflow_id, authoritative_workflow_id, kind, profile_detail_kind, severity, required_action, evidence_identity, resolved_at, expected_codec_family, expected_codec_version, expected_payload, actual_codec_family, actual_codec_version, actual_payload, recorded_at) \
             VALUES ('div-bad-actual', 'wf-shadow', 'wf-auth', 'snapshot', 'wake_terminal', 'informational', 'record_only', 'ev-3', NULL, NULL, NULL, NULL, 'actual', 1, NULL, 'now')",
        )
        .execute(&pool)
        .await;
        assert!(partial_actual.is_err());
    }

    #[tokio::test]
    async fn migration_042_rejects_stale_observation_without_attempt() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        let missing_attempt = sqlx::query(
            "INSERT INTO workflow_stale_observations \
             (id, effect_id, attempt_id, workflow_id, declared_workflow_version, generation, claim_token, claim_worker_id, claim_lease_until, claim_issued_at, codec_family, codec_version, payload, observed_at, recorded_at, stale_reason) \
             VALUES ('stale-1', 'eff', NULL, 'wf', 1, 0, 'claim-1', 'worker-1', 'later', 'now', 'obs', 1, '{}', 'now', 'now', 'superseded')",
        )
        .execute(&pool)
        .await;
        assert!(missing_attempt.is_err());
    }

    #[tokio::test]
    async fn migration_042_enforces_manual_resolution_choice_ownership_and_owed_resolution_state() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        seed_workflow_stack(&pool).await;

        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, event_payload, delivery_status, consumed_by_transition_id) \
             VALUES ('inbox', 'wf', 'rcpt', NULL, 'event', 1, '{}', 'pending', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let bad_owed = sqlx::query(
            "INSERT INTO workflow_owed_acceptance \
             (id, workflow_id, reducer_inbox_id, source_kind, event_codec_family, event_codec_version, event_payload, status, resolving_transition_id, suppression_reason) \
             VALUES ('owed-bad', 'wf', 'inbox', 'receipt', 'owed-event', 1, '{}', 'suppressed', NULL, 'superseded')",
        )
        .execute(&pool)
        .await;
        assert!(bad_owed.is_err());

        sqlx::query(
            "INSERT INTO workflow_manual_resolutions \
             (id, workflow_id, effect_id, status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by) \
             VALUES ('res-a', 'wf', 'eff', 'required', 'evidence', 1, '{}', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_manual_resolutions \
             (id, workflow_id, effect_id, status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by) \
             VALUES ('res-b', 'wf', 'eff', 'required', 'evidence', 1, '{}', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_manual_resolution_choices \
             (id, resolution_id, workflow_id, kind, codec_family, codec_version, payload) \
             VALUES ('choice-a', 'res-a', 'wf', 'retry', 'choice', 1, '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let wrong_owner = sqlx::query(
            "UPDATE workflow_manual_resolutions \
             SET status = 'resolved', accepted_choice_id = 'choice-a', resolved_by = 'operator' \
             WHERE id = 'res-b'",
        )
        .execute(&pool)
        .await;
        assert!(wrong_owner.is_err());
    }

    #[tokio::test]
    async fn migration_043_rejects_wrong_or_non_observe_binding_effects() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, state_updated_at, created_at, updated_at) VALUES ('conv', '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        seed_workflow_stack(&pool).await;
        sqlx::query(
            "INSERT INTO wake_registration_fences (conversation_id, version, status) VALUES ('conv', 7, 'open')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let missing_registering_tool_use = sqlx::query(
            "INSERT INTO wake_workflow_bindings \
             (contract_id, workflow_id, conversation_id, registration_scope_kind, registration_scope_stable_key, resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, registering_tool_use_id, registered_at, expires_at, registration_fence_version, observe_effect_id, lifecycle_fence_status) \
             VALUES ('contract-missing-tool', 'wf', 'conv', 'conversation', 'conv', 'bash', 'conversation', 'conv', 'b-1', NULL, NULL, NULL, NULL, '', 'now', 'later', 7, 'eff', 'open')",
        )
        .execute(&pool)
        .await;
        assert!(missing_registering_tool_use.is_err());

        let wrong_effect_kind = sqlx::query(
            "INSERT INTO wake_workflow_bindings \
             (contract_id, workflow_id, conversation_id, registration_scope_kind, registration_scope_stable_key, resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, registering_tool_use_id, registered_at, expires_at, registration_fence_version, observe_effect_id, lifecycle_fence_status) \
             VALUES ('contract-wrong-effect', 'wf', 'conv', 'conversation', 'conv', 'bash', 'conversation', 'conv', 'b-1', NULL, NULL, NULL, NULL, 'tool-1', 'now', 'later', 7, 'eff-other', 'open')",
        )
        .execute(&pool)
        .await;
        assert!(wrong_effect_kind.is_err());

        let wrong_fence = sqlx::query(
            "INSERT INTO wake_workflow_bindings \
             (contract_id, workflow_id, conversation_id, registration_scope_kind, registration_scope_stable_key, resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, registering_tool_use_id, registered_at, expires_at, registration_fence_version, observe_effect_id, lifecycle_fence_status) \
             VALUES ('contract-bad-fence', 'wf', 'conv', 'conversation', 'conv', 'bash', 'conversation', 'conv', 'b-1', NULL, NULL, NULL, NULL, 'tool-1', 'now', 'later', 999, 'eff', 'open')",
        )
        .execute(&pool)
        .await;
        assert!(wrong_fence.is_err());
    }

    #[tokio::test]
    async fn migration_043_rejects_wrong_receipt_effect_and_invalid_terminal_branches() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, state_updated_at, created_at, updated_at) VALUES ('conv-term', '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        seed_workflow_stack(&pool).await;
        sqlx::query(
            "INSERT INTO wake_registration_fences (conversation_id, version, status) VALUES ('conv-term', 1, 'open')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_workflow_bindings \
             (contract_id, workflow_id, conversation_id, registration_scope_kind, registration_scope_stable_key, resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, registering_tool_use_id, registered_at, expires_at, registration_fence_version, observe_effect_id, lifecycle_fence_status) \
             VALUES ('contract', 'wf', 'conv-term', 'conversation', 'conv-term', 'bash', 'conversation', 'conv-term', 'b-1', NULL, NULL, NULL, NULL, 'tool-1', 'now', 'later', 1, 'eff', 'open')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let wrong_receipt_effect = sqlx::query(
            "INSERT INTO wake_terminal_receipts \
             (receipt_id, workflow_id, contract_id, observe_effect_id, resource_kind, status, resolved_at, bash_status, bash_occurred_at, bash_exit_code, bash_duration_ms) \
             VALUES ('rcpt-other', 'wf', 'contract', 'eff', 'bash', 'fired', 'now', 'exited', 'now', 0, 10)",
        )
        .execute(&pool)
        .await;
        assert!(wrong_receipt_effect.is_err());

        let invalid_cancelled_branch = sqlx::query(
            "INSERT INTO wake_terminal_receipts \
             (receipt_id, workflow_id, contract_id, observe_effect_id, resource_kind, status, resolved_at, bash_status, bash_occurred_at, cancellation_reason) \
             VALUES ('rcpt', 'wf', 'contract', 'eff', 'bash', 'cancelled', 'now', 'killed', 'now', 'explicit_cancel')",
        )
        .execute(&pool)
        .await;
        assert!(invalid_cancelled_branch.is_err());

        sqlx::query(
            "INSERT INTO wake_terminal_receipts \
             (receipt_id, workflow_id, contract_id, observe_effect_id, resource_kind, status, resolved_at, cancellation_reason) \
             VALUES ('rcpt', 'wf', 'contract', 'eff', 'bash', 'cancelled', 'now', 'explicit_cancel')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let wrong_conv = sqlx::query(
            "INSERT INTO wake_observation_inbox \
             (id, workflow_id, contract_id, terminal_receipt_id, conversation_id, sequence, committed_at) \
             VALUES ('inbox-bad', 'wf', 'contract', 'rcpt', 'other-conv', 1, 'now')",
        )
        .execute(&pool)
        .await;
        assert!(wrong_conv.is_err());
    }

    #[tokio::test]
    async fn migration_043_rejects_unresolved_resolver_and_supports_obligation_minimal_shape() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, state_updated_at, created_at, updated_at) VALUES ('conv-tail', '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        seed_workflow_stack(&pool).await;
        sqlx::query(
            "INSERT INTO wake_registration_fences (conversation_id, version, status) VALUES ('conv-tail', 1, 'open')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_workflow_bindings \
             (contract_id, workflow_id, conversation_id, registration_scope_kind, registration_scope_stable_key, resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, registering_tool_use_id, registered_at, expires_at, registration_fence_version, observe_effect_id, lifecycle_fence_status) \
             VALUES ('contract-tail', 'wf', 'conv-tail', 'conversation', 'conv-tail', 'bash', 'conversation', 'conv-tail', 'b-1', NULL, NULL, NULL, NULL, 'tool-1', 'now', 'later', 1, 'eff', 'open')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_terminal_receipts \
             (receipt_id, workflow_id, contract_id, observe_effect_id, resource_kind, status, resolved_at, bash_status, bash_occurred_at, bash_exit_code, bash_duration_ms, bash_tail_start_offset, bash_tail_end_offset, bash_tail_truncated_before) \
             VALUES ('rcpt', 'wf', 'contract-tail', 'eff', 'bash', 'fired', 'now', 'exited', 'now', 0, 10, 0, 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_observation_inbox \
             (id, workflow_id, contract_id, terminal_receipt_id, conversation_id, sequence, committed_at) \
             VALUES ('inbox', 'wf', 'contract-tail', 'rcpt', 'conv-tail', 1, 'now')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_runtime_obligations \
             (id, conversation_id, snapshot_upper_bound, status, created_at, resolved_at, terminal_reason) \
             VALUES ('obl', 'conv-tail', 1, 'accepted', 'now', 'now', 'accepted')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let unresolved_resolver = sqlx::query(
            "INSERT INTO workflow_manual_resolutions \
             (id, workflow_id, effect_id, status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by) \
             VALUES ('res-required-bad', 'wf', 'eff', 'required', 'evidence', 1, '{}', NULL, 'operator')",
        )
        .execute(&pool)
        .await;
        assert!(unresolved_resolver.is_err());

        let missing_resolved_by = sqlx::query(
            "INSERT INTO workflow_manual_resolutions \
             (id, workflow_id, effect_id, status, evidence_codec_family, evidence_codec_version, evidence_payload, accepted_choice_id, resolved_by) \
             VALUES ('res-resolved-bad', 'wf', 'eff', 'resolved', 'evidence', 1, '{}', 'missing-choice', '')",
        )
        .execute(&pool)
        .await;
        assert!(missing_resolved_by.is_err());

        sqlx::query(
            "INSERT INTO wake_runtime_obligation_items \
             (obligation_id, ordinal, inbox_item_id) \
             VALUES ('obl', 0, 'inbox')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_terminal_receipt_bash_tail (receipt_id, ordinal, stream, offset, line) VALUES ('rcpt', 0, 'stdout', 1, 'hello')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wake_terminal_receipt_bash_tail (receipt_id, ordinal, stream, offset, line) VALUES ('rcpt', 1, 'stderr', 2, 'bye')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wake_terminal_receipt_bash_tail WHERE receipt_id = 'rcpt'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
    }

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
            // Renamed from snapshot_* by migration 019 (run_pending_migrations
            // applies every migration, so the final column set is post-rename).
            "chain_members_at_answer",
            "chain_messages_at_answer",
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

    /// Migration 019 renames the `chain_qa` chain-size markers from `snapshot_*`
    /// to `*_at_answer` while preserving each row's recorded values.
    #[tokio::test]
    async fn migration_019_renames_chain_qa_snapshot_columns_preserving_data() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        sqlx::query(
            "INSERT INTO conversations (id, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
             VALUES ('root', '{\"mode\":\"Explore\"}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_pending_migrations(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO chain_qa (id, root_conv_id, question, model, status, \
             chain_members_at_answer, chain_messages_at_answer, created_at) \
             VALUES ('qa1', 'root', 'q?', 'm', 'completed', 3, 17, '2025-01-02')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let columns: Vec<String> = sqlx::query("PRAGMA table_info(chain_qa)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(
            !columns
                .iter()
                .any(|c| c == "snapshot_member_count" || c == "snapshot_total_messages"),
            "Old snapshot_* columns should be gone after migration 019, got: {columns:?}"
        );

        let row = sqlx::query(
            "SELECT chain_members_at_answer, chain_messages_at_answer FROM chain_qa WHERE id = 'qa1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<i64, _>("chain_members_at_answer"), 3);
        assert_eq!(row.get::<i64, _>("chain_messages_at_answer"), 17);
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
    // reason: exhaustive table-driven backfill test covering 7 distinct seed
    // scenarios; splitting would scatter the shared fixture setup.
    #[allow(clippy::too_many_lines)]
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

    #[tokio::test]
    async fn migration_009_rewrites_one_million_context_model_ids_to_base() {
        async fn model_for(pool: &SqlitePool, id: &str) -> Option<String> {
            sqlx::query("SELECT model FROM conversations WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap()
                .get::<Option<String>, _>("model")
        }

        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        // Seed three convs pinned to legacy `-1m` ids and one on a base id.
        // The migration must rewrite the three `-1m` rows and leave the
        // already-base row untouched.
        let seeds = [
            ("c-opus-7-1m", "claude-opus-4-7-1m"),
            ("c-opus-6-1m", "claude-opus-4-6-1m"),
            ("c-sonnet-6-1m", "claude-sonnet-4-6-1m"),
            ("c-base", "claude-sonnet-4-6"),
        ];
        for (id, model) in seeds {
            sqlx::query(
                "INSERT INTO conversations (id, model, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
                 VALUES (?, ?, '{\"mode\":\"Direct\"}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(model)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_pending_migrations(&pool).await.unwrap();

        assert_eq!(
            model_for(&pool, "c-opus-7-1m").await.as_deref(),
            Some("claude-opus-4-7")
        );
        assert_eq!(
            model_for(&pool, "c-opus-6-1m").await.as_deref(),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            model_for(&pool, "c-sonnet-6-1m").await.as_deref(),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(
            model_for(&pool, "c-base").await.as_deref(),
            Some("claude-sonnet-4-6"),
            "Non -1m rows must not be touched"
        );

        // turn_usage and chain_qa are created by earlier migrations (004, 005);
        // confirm migration 009's UPDATE statements against them did not error
        // by checking the tables are queryable.
        sqlx::query("SELECT COUNT(*) FROM turn_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("SELECT COUNT(*) FROM chain_qa")
            .fetch_one(&pool)
            .await
            .unwrap();
    }

    /// Migration 022: the MCP OAuth tables exist with the expected columns and
    /// per-key uniqueness (one registration per authorization server, one token
    /// per MCP server).
    #[tokio::test]
    async fn migration_022_creates_mcp_oauth_tables() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        run_pending_migrations(&pool).await.unwrap();

        let reg_columns: Vec<String> = sqlx::query("PRAGMA table_info(mcp_oauth_registrations)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        for expected in [
            "auth_server",
            "client_id",
            "client_secret",
            "token_endpoint_auth_method",
        ] {
            assert!(
                reg_columns.iter().any(|c| c == expected),
                "Expected mcp_oauth_registrations column {expected:?}, got: {reg_columns:?}"
            );
        }

        let token_columns: Vec<String> = sqlx::query("PRAGMA table_info(mcp_oauth_tokens)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        for expected in [
            "server_name",
            "resource_uri",
            "scopes",
            "access_token",
            "refresh_token",
            "expires_at",
        ] {
            assert!(
                token_columns.iter().any(|c| c == expected),
                "Expected mcp_oauth_tokens column {expected:?}, got: {token_columns:?}"
            );
        }

        // OneTokenPerServer: the primary key makes a second insert for the
        // same server a constraint violation, not a duplicate row.
        sqlx::query(
            "INSERT INTO mcp_oauth_tokens (server_name, resource_uri, scopes, access_token, expires_at) \
             VALUES ('s', 'https://example.com/mcp', 'a b', 'tok', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let dup = sqlx::query(
            "INSERT INTO mcp_oauth_tokens (server_name, resource_uri, scopes, access_token, expires_at) \
             VALUES ('s', 'https://other.com/mcp', '', 'tok2', 2)",
        )
        .execute(&pool)
        .await;
        assert!(
            dup.is_err(),
            "second token row for one server must violate the primary key"
        );
    }

    #[tokio::test]
    async fn migration_010_rewrites_opus_4_5_to_4_6() {
        async fn model_for(pool: &SqlitePool, id: &str) -> Option<String> {
            sqlx::query("SELECT model FROM conversations WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap()
                .get::<Option<String>, _>("model")
        }

        let pool = test_pool().await;
        setup_conversations_table(&pool).await;

        let seeds = [
            ("c-opus-5", "claude-opus-4-5"),
            ("c-opus-6", "claude-opus-4-6"),
            ("c-other", "claude-sonnet-4-6"),
        ];
        for (id, model) in seeds {
            sqlx::query(
                "INSERT INTO conversations (id, model, conv_mode, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
                 VALUES (?, ?, '{\"mode\":\"Direct\"}', '{\"type\":\"idle\"}', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(model)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_pending_migrations(&pool).await.unwrap();

        assert_eq!(
            model_for(&pool, "c-opus-5").await.as_deref(),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            model_for(&pool, "c-opus-6").await.as_deref(),
            Some("claude-opus-4-6"),
            "already-4-6 rows must not be touched"
        );
        assert_eq!(
            model_for(&pool, "c-other").await.as_deref(),
            Some("claude-sonnet-4-6"),
            "non-Opus rows must not be touched"
        );
    }
}
