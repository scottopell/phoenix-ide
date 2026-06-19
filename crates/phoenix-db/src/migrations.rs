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
        assert_eq!(first, 29);

        let second = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(second, 0);
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
