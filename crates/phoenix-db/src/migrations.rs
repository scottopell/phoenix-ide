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
        name: "replace_global_recall_with_coordinator",
        sql: MIGRATION_044,
    },
    Migration {
        version: 45,
        name: "create_continuation_dispatch_intents",
        sql: MIGRATION_045,
    },
    Migration {
        version: 46,
        name: "create_workflow_foundation_tables",
        sql: MIGRATION_046,
    },
    Migration {
        version: 47,
        name: "create_wake_bindings",
        sql: MIGRATION_047,
    },
    Migration {
        version: 48,
        name: "create_wake_terminal_receipts",
        sql: MIGRATION_048,
    },
    Migration {
        version: 49,
        name: "create_wake_delivery_messages",
        sql: MIGRATION_049,
    },
    Migration {
        version: 50,
        name: "add_tmux_completion_policy",
        sql: MIGRATION_050,
    },
    Migration {
        version: 51,
        name: "index_message_fts_row_locations",
        sql: MIGRATION_051,
    },
    Migration {
        version: 52,
        name: "create_llm_request_metrics",
        sql: MIGRATION_052,
    },
    Migration {
        version: 53,
        name: "opaque_work_scope_identity",
        sql: MIGRATION_053,
    },
    Migration {
        version: 54,
        name: "work_scope_owns_environment",
        sql: MIGRATION_054,
    },
    Migration {
        version: 55,
        name: "create_authoritative_direct_turns",
        sql: MIGRATION_055,
    },
    Migration {
        version: 56,
        name: "seed_workflow_global_sequences",
        sql: MIGRATION_056,
    },
    Migration {
        version: 57,
        name: "normalize_direct_turn_attachments",
        sql: MIGRATION_057,
    },
];

const MIGRATION_057: &str = r"
CREATE TABLE IF NOT EXISTS durable_turn_submitted_images (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0), media_type TEXT NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);
CREATE TABLE IF NOT EXISTS durable_turn_submitted_files (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0), original_name TEXT NOT NULL,
    media_type TEXT NOT NULL, size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0), stored_path TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);
CREATE TABLE IF NOT EXISTS durable_turn_delivery_images (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0), media_type TEXT NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);
CREATE TABLE IF NOT EXISTS durable_turn_delivery_files (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0), original_name TEXT NOT NULL,
    media_type TEXT NOT NULL, size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0), stored_path TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);
";

const MIGRATION_056: &str = r"
INSERT OR IGNORE INTO workflow_global_sequences (sequence_name, next_value)
SELECT 'workflow', COALESCE(MAX(workflow_id), 0) + 1 FROM workflows;

UPDATE workflow_global_sequences
SET next_value = MAX(
    next_value,
    (SELECT COALESCE(MAX(workflow_id), 0) + 1 FROM workflows)
)
WHERE sequence_name = 'workflow';

INSERT OR IGNORE INTO workflow_global_sequences (sequence_name, next_value)
SELECT 'direct_turn', COALESCE(MAX(turn_id), 0) + 1 FROM durable_turns;

UPDATE workflow_global_sequences
SET next_value = MAX(
    next_value,
    (SELECT COALESCE(MAX(turn_id), 0) + 1 FROM durable_turns)
)
WHERE sequence_name = 'direct_turn';
";

const MIGRATION_055: &str = r"
CREATE TABLE durable_turns (
    turn_id INTEGER PRIMARY KEY,
    workflow_id INTEGER NOT NULL UNIQUE REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    client_turn_key TEXT NOT NULL CHECK (client_turn_key <> ''),
    prepared_fingerprint TEXT NOT NULL,
    prepared_payload BLOB NOT NULL,
    disposition TEXT NOT NULL CHECK (disposition IN ('Runtime', 'Steering')),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    terminal_kind TEXT CHECK (terminal_kind IN ('Completed', 'Cancelled', 'Failed')),
    terminal_reason TEXT,
    owns_conversation INTEGER NOT NULL CHECK (owns_conversation IN (0, 1)),
    canonical_message_id TEXT,
    UNIQUE (conversation_id, client_turn_key),
    CHECK (owns_conversation = (disposition = 'Runtime' AND terminal_kind IS NULL)),
    CHECK (
        (terminal_kind = 'Failed' AND terminal_reason IS NOT NULL)
        OR (terminal_kind IS NULL AND terminal_reason IS NULL)
        OR (terminal_kind IN ('Completed', 'Cancelled') AND terminal_reason IS NULL)
    ),
    FOREIGN KEY (conversation_id, canonical_message_id)
        REFERENCES messages(conversation_id, message_id) ON DELETE RESTRICT
);

CREATE TABLE durable_turn_submitted_images (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);

CREATE TABLE durable_turn_submitted_files (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    original_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    stored_path TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);

CREATE TABLE durable_turn_delivery_images (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);

CREATE TABLE durable_turn_delivery_files (
    turn_id INTEGER NOT NULL REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    original_name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    stored_path TEXT NOT NULL,
    PRIMARY KEY (turn_id, ordinal)
);

CREATE UNIQUE INDEX messages_conversation_message_id_unique
    ON messages(conversation_id, message_id);

CREATE TRIGGER durable_turns_delete_owned_workflow
AFTER DELETE ON durable_turns
BEGIN
    DELETE FROM workflows WHERE workflow_id = OLD.workflow_id;
END;

CREATE UNIQUE INDEX durable_turns_one_live_owner
    ON durable_turns(conversation_id)
    WHERE owns_conversation = 1;

CREATE INDEX durable_turns_discoverable_nonterminal
    ON durable_turns(turn_id, workflow_id)
    WHERE disposition = 'Runtime'
      AND terminal_kind IS NULL
      AND canonical_message_id IS NULL;
";

const MIGRATION_052: &str = r"
CREATE TABLE IF NOT EXISTS llm_request_metrics (
    request_id TEXT NOT NULL,
    retry_attempt INTEGER NOT NULL CHECK (retry_attempt >= 1),
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    root_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    transport TEXT NOT NULL CHECK (transport IN ('http_sse', 'websocket', 'in_process', 'http_json')),
    total_duration_ms INTEGER NOT NULL CHECK (total_duration_ms >= 0),
    dispatch_to_first_provider_event_ms INTEGER CHECK (dispatch_to_first_provider_event_ms IS NULL OR dispatch_to_first_provider_event_ms >= 0),
    dispatch_to_first_generation_event_ms INTEGER CHECK (dispatch_to_first_generation_event_ms IS NULL OR dispatch_to_first_generation_event_ms >= 0),
    dispatch_to_first_visible_text_ms INTEGER CHECK (dispatch_to_first_visible_text_ms IS NULL OR dispatch_to_first_visible_text_ms >= 0),
    provider_event_count INTEGER NOT NULL CHECK (provider_event_count >= 0),
    generation_event_count INTEGER NOT NULL CHECK (generation_event_count >= 0),
    visible_text_event_count INTEGER NOT NULL CHECK (visible_text_event_count >= 0),
    max_provider_gap_ms INTEGER CHECK (max_provider_gap_ms IS NULL OR max_provider_gap_ms >= 0),
    max_generation_gap_ms INTEGER CHECK (max_generation_gap_ms IS NULL OR max_generation_gap_ms >= 0),
    output_kind TEXT NOT NULL CHECK (output_kind IN ('none', 'text', 'reasoning', 'tool', 'structured', 'mixed')),
    stream_completed INTEGER NOT NULL CHECK (stream_completed IN (0, 1)),
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'rate_limited', 'usage_limit_reached', 'server_error', 'invalid_response', 'server_overloaded', 'network_error', 'token_budget_exceeded', 'auth_error', 'request_rejected', 'cancelled')),
    created_at TEXT NOT NULL,
    PRIMARY KEY (request_id, retry_attempt)
);

CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_created_at ON llm_request_metrics(created_at);
CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_provider_model_transport
    ON llm_request_metrics(provider, model, transport, created_at);
CREATE INDEX IF NOT EXISTS idx_llm_request_metrics_root ON llm_request_metrics(root_conversation_id, created_at);
";

const MIGRATION_054: &str = r"
ALTER TABLE work_scopes ADD COLUMN environment_kind TEXT NOT NULL DEFAULT 'none'
    CHECK (environment_kind IN ('allocated_worktree', 'unowned_cwd', 'none'));
ALTER TABLE work_scopes ADD COLUMN cwd TEXT;
ALTER TABLE work_scopes ADD COLUMN worktree_path TEXT;
ALTER TABLE work_scopes ADD COLUMN branch_name TEXT;
ALTER TABLE work_scopes ADD COLUMN base_branch TEXT;

UPDATE work_scopes
SET environment_kind = (SELECT environment_kind FROM work_scope_environments WHERE work_scope_id = work_scopes.id),
    cwd = (SELECT cwd FROM work_scope_environments WHERE work_scope_id = work_scopes.id),
    worktree_path = (SELECT worktree_path FROM work_scope_environments WHERE work_scope_id = work_scopes.id),
    branch_name = (SELECT branch_name FROM work_scope_environments WHERE work_scope_id = work_scopes.id),
    base_branch = (SELECT base_branch FROM work_scope_environments WHERE work_scope_id = work_scopes.id),
    updated_at = MAX(updated_at, COALESCE((SELECT updated_at FROM work_scope_environments WHERE work_scope_id = work_scopes.id), updated_at));

CREATE TEMP TABLE migration_054_guard (invalid_count INTEGER NOT NULL CHECK (invalid_count = 0));
INSERT INTO migration_054_guard
SELECT COUNT(*) FROM work_scopes
WHERE NOT (
    (environment_kind = 'allocated_worktree' AND cwd IS NOT NULL AND cwd <> '' AND worktree_path IS NOT NULL AND worktree_path <> '' AND (branch_name IS NULL OR branch_name <> '') AND (base_branch IS NULL OR base_branch <> ''))
    OR (environment_kind = 'unowned_cwd' AND cwd IS NOT NULL AND cwd <> '' AND worktree_path IS NULL AND branch_name IS NULL AND base_branch IS NULL)
    OR (environment_kind = 'none' AND cwd IS NULL AND worktree_path IS NULL AND branch_name IS NULL AND base_branch IS NULL)
);

DROP TABLE work_scope_environments;
CREATE VIEW work_scope_environments AS
SELECT id AS work_scope_id, environment_kind, cwd, worktree_path, branch_name, base_branch, updated_at
FROM work_scopes;
CREATE TRIGGER work_scope_environment_shape_insert
BEFORE INSERT ON work_scopes
WHEN NOT (
    (NEW.environment_kind = 'allocated_worktree' AND NEW.cwd IS NOT NULL AND NEW.cwd <> '' AND NEW.worktree_path IS NOT NULL AND NEW.worktree_path <> '' AND (NEW.branch_name IS NULL OR NEW.branch_name <> '') AND (NEW.base_branch IS NULL OR NEW.base_branch <> ''))
    OR (NEW.environment_kind = 'unowned_cwd' AND NEW.cwd IS NOT NULL AND NEW.cwd <> '' AND NEW.worktree_path IS NULL AND NEW.branch_name IS NULL AND NEW.base_branch IS NULL)
    OR (NEW.environment_kind = 'none' AND NEW.cwd IS NULL AND NEW.worktree_path IS NULL AND NEW.branch_name IS NULL AND NEW.base_branch IS NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'invalid work scope environment');
END;
CREATE TRIGGER work_scope_environment_shape_update
BEFORE UPDATE OF environment_kind, cwd, worktree_path, branch_name, base_branch ON work_scopes
WHEN NOT (
    (NEW.environment_kind = 'allocated_worktree' AND NEW.cwd IS NOT NULL AND NEW.cwd <> '' AND NEW.worktree_path IS NOT NULL AND NEW.worktree_path <> '' AND (NEW.branch_name IS NULL OR NEW.branch_name <> '') AND (NEW.base_branch IS NULL OR NEW.base_branch <> ''))
    OR (NEW.environment_kind = 'unowned_cwd' AND NEW.cwd IS NOT NULL AND NEW.cwd <> '' AND NEW.worktree_path IS NULL AND NEW.branch_name IS NULL AND NEW.base_branch IS NULL)
    OR (NEW.environment_kind = 'none' AND NEW.cwd IS NULL AND NEW.worktree_path IS NULL AND NEW.branch_name IS NULL AND NEW.base_branch IS NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'invalid work scope environment');
END;

ALTER TABLE conversations ADD COLUMN sub_agent_cwd_override TEXT;
UPDATE conversations
SET sub_agent_cwd_override = cwd
WHERE runtime_role = 'sub_agent'
  AND cwd <> COALESCE(
      (SELECT cwd FROM work_scopes WHERE id = conversations.work_scope_id),
      cwd
  );

ALTER TABLE conversations ADD COLUMN coordinator_head INTEGER NOT NULL DEFAULT 0 CHECK (coordinator_head IN (0, 1));
UPDATE conversations SET coordinator_head = 1
WHERE runtime_role = 'coordinator' AND continued_in_conv_id IS NULL;
DROP INDEX one_coordinator_conversation;
CREATE UNIQUE INDEX one_live_coordinator_conversation
ON conversations(coordinator_head)
WHERE coordinator_head = 1;
ALTER TABLE conversations DROP COLUMN cm_branch_name;
ALTER TABLE conversations DROP COLUMN cm_worktree_path;
ALTER TABLE conversations DROP COLUMN cm_base_branch;
ALTER TABLE conversations DROP COLUMN cwd;
DROP TRIGGER conversations_role_scope_insert;
DROP TRIGGER conversations_role_scope_update;
CREATE TRIGGER conversations_role_scope_insert
BEFORE INSERT ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR ((NEW.runtime_role = 'coordinator') != (NEW.work_scope_id IS NULL))
  OR (NEW.coordinator_head = 1 AND NEW.runtime_role <> 'coordinator')
  OR (NEW.coordinator_head = 1 AND NEW.continued_in_conv_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;
CREATE TRIGGER conversations_role_scope_update
BEFORE UPDATE OF runtime_role, work_scope_id, coordinator_head, continued_in_conv_id ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR ((NEW.runtime_role = 'coordinator') != (NEW.work_scope_id IS NULL))
  OR (NEW.coordinator_head = 1 AND NEW.runtime_role <> 'coordinator')
  OR (NEW.coordinator_head = 1 AND NEW.continued_in_conv_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;
DROP TABLE migration_054_guard;
";

const MIGRATION_053: &str = r"
ALTER TABLE work_scopes RENAME TO work_scopes_old;
CREATE TEMP TABLE migration_052_scope_map (
    old_id INTEGER PRIMARY KEY,
    new_id TEXT NOT NULL UNIQUE
);

INSERT INTO migration_052_scope_map (old_id, new_id)
SELECT id, lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' || '4' || substr(hex(randomblob(2)), 2) || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6)))
FROM work_scopes_old;

CREATE TABLE work_scopes_new (
    id TEXT PRIMARY KEY CHECK (id <> ''),
    authority_kind TEXT NOT NULL CHECK (authority_kind IN ('restricted_explore', 'work')),
    lifecycle TEXT NOT NULL DEFAULT 'active' CHECK (lifecycle IN ('active', 'retired')),
    retired_at TEXT,
    retired_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((lifecycle = 'retired') = (retired_at IS NOT NULL)),
    CHECK (retired_reason IS NULL OR retired_reason <> '')
);

INSERT INTO work_scopes_new (id, authority_kind, lifecycle, created_at, updated_at)
SELECT m.new_id,
       CASE WHEN old.scope_type = 'Conversation' THEN 'restricted_explore' ELSE 'work' END,
       'active', old.created_at, old.updated_at
FROM work_scopes_old old
JOIN migration_052_scope_map m ON m.old_id = old.id;

CREATE TABLE work_scope_environments (
    work_scope_id TEXT PRIMARY KEY REFERENCES work_scopes_new(id) ON DELETE CASCADE,
    environment_kind TEXT NOT NULL CHECK (environment_kind IN ('allocated_worktree', 'unowned_cwd', 'none')),
    cwd TEXT,
    worktree_path TEXT,
    branch_name TEXT,
    base_branch TEXT,
    updated_at TEXT NOT NULL,
    CHECK (
        (environment_kind = 'allocated_worktree' AND cwd IS NOT NULL AND cwd <> '' AND worktree_path IS NOT NULL AND worktree_path <> '' AND (branch_name IS NULL OR branch_name <> '') AND (base_branch IS NULL OR base_branch <> ''))
        OR (environment_kind = 'unowned_cwd' AND cwd IS NOT NULL AND cwd <> '' AND worktree_path IS NULL AND branch_name IS NULL AND base_branch IS NULL)
        OR (environment_kind = 'none' AND cwd IS NULL AND worktree_path IS NULL AND branch_name IS NULL AND base_branch IS NULL)
    )
);

ALTER TABLE conversations ADD COLUMN runtime_role TEXT NOT NULL DEFAULT 'user'
    CHECK (runtime_role IN ('user', 'sub_agent', 'coordinator'));
ALTER TABLE conversations ADD COLUMN work_scope_id TEXT REFERENCES work_scopes_new(id);

UPDATE conversations
SET runtime_role = 'sub_agent'
WHERE parent_conversation_id IS NOT NULL;

WITH RECURSIVE coordinator_chain(id) AS (
    SELECT conversation_id FROM coordinator WHERE singleton = 1
    UNION
    SELECT predecessor.id
    FROM coordinator_chain chain
    JOIN conversations predecessor ON predecessor.continued_in_conv_id = chain.id
)
UPDATE conversations
SET runtime_role = 'coordinator'
WHERE id IN (SELECT id FROM coordinator_chain);

-- Resolve ownership from true user roots before creating any missing scopes. A
-- continuation successor is not a root, even though it has no parent.
CREATE TEMP TABLE migration_052_lineage (
    root_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    PRIMARY KEY (root_id, conversation_id)
);
INSERT INTO migration_052_lineage
WITH RECURSIVE lineage(root_id, conversation_id) AS (
    SELECT c.id, c.id
    FROM conversations c
    WHERE c.runtime_role = 'user'
      AND c.parent_conversation_id IS NULL
      AND NOT EXISTS (
          SELECT 1 FROM conversations predecessor
          WHERE predecessor.continued_in_conv_id = c.id
      )
    UNION
    SELECT lineage.root_id, child.id
    FROM lineage
    JOIN conversations child ON child.parent_conversation_id = lineage.conversation_id
    WHERE child.runtime_role <> 'coordinator'
    UNION
    SELECT lineage.root_id, successor.id
    FROM lineage
    JOIN conversations owner ON owner.id = lineage.conversation_id
    JOIN conversations successor ON successor.id = owner.continued_in_conv_id
    WHERE successor.runtime_role <> 'coordinator'
)
SELECT root_id, conversation_id FROM lineage;

CREATE TEMP TABLE migration_052_guard (invalid_count INTEGER NOT NULL CHECK (invalid_count = 0));
INSERT INTO migration_052_guard
SELECT COUNT(*)
FROM conversations c
WHERE c.runtime_role <> 'coordinator'
  AND (SELECT COUNT(*) FROM migration_052_lineage l WHERE l.conversation_id = c.id) <> 1;

CREATE TEMP TABLE migration_052_generated_scope_map (
    root_id TEXT PRIMARY KEY,
    new_id TEXT NOT NULL UNIQUE,
    authority_kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO migration_052_generated_scope_map
SELECT root.id,
       lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' || '4' || substr(hex(randomblob(2)), 2) || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6))),
       CASE WHEN root.cm_kind IN ('work', 'branch') THEN 'work' ELSE 'restricted_explore' END,
       root.created_at,
       root.updated_at
FROM conversations root
WHERE EXISTS (SELECT 1 FROM migration_052_lineage l WHERE l.root_id = root.id)
  AND root.cm_kind <> 'direct'
  AND NOT EXISTS (
      SELECT 1 FROM work_scopes_old old
      WHERE old.scope_type = 'Worktree' AND old.scope_value = root.cm_worktree_path
  )
  AND NOT EXISTS (
      SELECT 1 FROM work_scopes_old old
      WHERE old.scope_type = 'Conversation' AND old.scope_value = root.id
  );

INSERT INTO work_scopes_new (id, authority_kind, lifecycle, created_at, updated_at)
SELECT new_id, authority_kind, 'active', created_at, updated_at
FROM migration_052_generated_scope_map;

CREATE TEMP TABLE migration_052_generated_direct_scope_map (
    conversation_id TEXT PRIMARY KEY,
    new_id TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO migration_052_generated_direct_scope_map
SELECT c.id,
       lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' || '4' || substr(hex(randomblob(2)), 2) || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(hex(randomblob(2)), 2) || '-' || hex(randomblob(6))),
       c.created_at,
       c.updated_at
FROM conversations c
WHERE c.runtime_role = 'user'
  AND c.cm_kind = 'direct'
  AND NOT EXISTS (
      SELECT 1 FROM work_scopes_old old
      WHERE old.scope_type = 'Conversation' AND old.scope_value = c.id
  );

INSERT INTO work_scopes_new (id, authority_kind, lifecycle, created_at, updated_at)
SELECT new_id, 'work', 'active', created_at, updated_at
FROM migration_052_generated_direct_scope_map;

CREATE TEMP TABLE migration_052_root_scope (
    root_id TEXT PRIMARY KEY,
    work_scope_id TEXT NOT NULL
);
INSERT INTO migration_052_root_scope
SELECT root.id,
       COALESCE(
           (SELECT m.new_id
            FROM work_scopes_old old
            JOIN migration_052_scope_map m ON m.old_id = old.id
            WHERE old.scope_type = 'Worktree' AND old.scope_value = root.cm_worktree_path),
           (SELECT m.new_id
            FROM work_scopes_old old
            JOIN migration_052_scope_map m ON m.old_id = old.id
            WHERE old.scope_type = 'Conversation' AND old.scope_value = root.id),
           (SELECT direct.new_id FROM migration_052_generated_direct_scope_map direct WHERE direct.conversation_id = root.id),
           (SELECT g.new_id FROM migration_052_generated_scope_map g WHERE g.root_id = root.id)
       )
FROM conversations root
WHERE EXISTS (SELECT 1 FROM migration_052_lineage l WHERE l.root_id = root.id);

CREATE TEMP TABLE migration_052_conversation_scope (
    conversation_id TEXT PRIMARY KEY,
    work_scope_id TEXT NOT NULL
);
INSERT INTO migration_052_conversation_scope
SELECT lineage.conversation_id,
       CASE
           WHEN c.runtime_role = 'user' AND c.cm_kind = 'direct' THEN COALESCE(
               (SELECT m.new_id
                FROM work_scopes_old old
                JOIN migration_052_scope_map m ON m.old_id = old.id
                WHERE old.scope_type = 'Conversation' AND old.scope_value = c.id),
               (SELECT generated.new_id
                FROM migration_052_generated_direct_scope_map generated
                WHERE generated.conversation_id = c.id)
           )
           WHEN c.runtime_role <> 'user' THEN COALESCE(
               (WITH RECURSIVE ancestors(id, parent_conversation_id, depth) AS (
                    SELECT parent.id, parent.parent_conversation_id, 1
                    FROM conversations parent
                    WHERE parent.id = c.parent_conversation_id
                    UNION ALL
                    SELECT parent.id, parent.parent_conversation_id, ancestors.depth + 1
                    FROM ancestors
                    JOIN conversations parent ON parent.id = ancestors.parent_conversation_id
                )
                SELECT COALESCE(
                    (SELECT m.new_id
                     FROM work_scopes_old old
                     JOIN migration_052_scope_map m ON m.old_id = old.id
                     WHERE old.scope_type = 'Conversation' AND old.scope_value = ancestor.id),
                    (SELECT generated.new_id
                     FROM migration_052_generated_direct_scope_map generated
                     WHERE generated.conversation_id = ancestor.id)
                )
                FROM ancestors ancestor
                JOIN conversations parent ON parent.id = ancestor.id
                WHERE parent.runtime_role = 'user' AND parent.cm_kind = 'direct'
                ORDER BY ancestor.depth
                LIMIT 1),
               root_scope.work_scope_id
           )
           ELSE root_scope.work_scope_id
       END
FROM migration_052_lineage lineage
JOIN conversations c ON c.id = lineage.conversation_id
JOIN migration_052_root_scope root_scope ON root_scope.root_id = lineage.root_id;

UPDATE conversations
SET work_scope_id = (
    SELECT map.work_scope_id
    FROM migration_052_conversation_scope map
    WHERE map.conversation_id = conversations.id
)
WHERE runtime_role <> 'coordinator';

-- Pick one complete representative row per scope. Ordering by timestamp and id
-- makes every environment field come from the same legacy row.
CREATE TEMP TABLE migration_052_environment_candidates (
    work_scope_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    source_priority INTEGER NOT NULL,
    PRIMARY KEY (work_scope_id, conversation_id)
);
INSERT INTO migration_052_environment_candidates
SELECT scoped.work_scope_id, scoped.conversation_id,
       CASE
           WHEN c.cm_kind IN ('work', 'branch') THEN 0
           WHEN lineage.root_id = scoped.conversation_id THEN 1
           ELSE 2
       END
FROM migration_052_conversation_scope scoped
JOIN migration_052_lineage lineage ON lineage.conversation_id = scoped.conversation_id
JOIN conversations c ON c.id = scoped.conversation_id;
INSERT OR IGNORE INTO migration_052_environment_candidates
SELECT m.new_id, c.id, 3
FROM work_scopes_old old
JOIN migration_052_scope_map m ON m.old_id = old.id
JOIN conversations c
  ON (old.scope_type = 'Worktree' AND c.cm_worktree_path = old.scope_value)
  OR (old.scope_type = 'Conversation' AND c.id = old.scope_value);

CREATE TEMP TABLE migration_052_environment_representative (
    work_scope_id TEXT PRIMARY KEY,
    conversation_id TEXT
);
INSERT INTO migration_052_environment_representative
SELECT scope.id,
       (SELECT candidate.conversation_id
        FROM migration_052_environment_candidates candidate
        JOIN conversations c ON c.id = candidate.conversation_id
        WHERE candidate.work_scope_id = scope.id
        ORDER BY candidate.source_priority, c.updated_at DESC, c.id
        LIMIT 1)
FROM work_scopes_new scope;

INSERT INTO work_scope_environments
    (work_scope_id, environment_kind, cwd, worktree_path, branch_name, base_branch, updated_at)
SELECT scope.id,
       CASE
           WHEN representative.cm_worktree_path IS NOT NULL AND representative.cm_worktree_path <> '' THEN 'allocated_worktree'
           WHEN old.scope_type = 'Worktree' AND old.scope_value <> '' THEN 'allocated_worktree'
           WHEN representative.cwd IS NOT NULL AND representative.cwd <> '' THEN 'unowned_cwd'
           ELSE 'none'
       END,
       CASE
           WHEN representative.cm_worktree_path IS NOT NULL AND representative.cm_worktree_path <> ''
               THEN COALESCE(NULLIF(representative.cwd, ''), representative.cm_worktree_path)
           WHEN old.scope_type = 'Worktree' AND old.scope_value <> ''
               THEN COALESCE(NULLIF(representative.cwd, ''), old.scope_value)
           WHEN representative.cwd IS NOT NULL AND representative.cwd <> '' THEN representative.cwd
           ELSE NULL
       END,
       CASE
           WHEN representative.cm_worktree_path IS NOT NULL AND representative.cm_worktree_path <> '' THEN representative.cm_worktree_path
           WHEN old.scope_type = 'Worktree' AND old.scope_value <> '' THEN old.scope_value
           ELSE NULL
       END,
       CASE
           WHEN COALESCE(NULLIF(representative.cm_worktree_path, ''), CASE WHEN old.scope_type = 'Worktree' THEN old.scope_value END) IS NOT NULL
               THEN representative.cm_branch_name
           ELSE NULL
       END,
       CASE
           WHEN COALESCE(NULLIF(representative.cm_worktree_path, ''), CASE WHEN old.scope_type = 'Worktree' THEN old.scope_value END) IS NOT NULL
               THEN representative.cm_base_branch
           ELSE NULL
       END,
       COALESCE(representative.updated_at, old.updated_at, scope.updated_at)
FROM work_scopes_new scope
LEFT JOIN migration_052_scope_map map ON map.new_id = scope.id
LEFT JOIN work_scopes_old old ON old.id = map.old_id
LEFT JOIN migration_052_environment_representative rep ON rep.work_scope_id = scope.id
LEFT JOIN conversations representative ON representative.id = rep.conversation_id;

DELETE FROM migration_052_guard;
INSERT INTO migration_052_guard
SELECT ABS(
    (SELECT COUNT(*) FROM work_scopes_new)
    - (SELECT COUNT(*) FROM work_scope_environments)
);

CREATE INDEX IF NOT EXISTS idx_conversations_work_scope ON conversations(work_scope_id);
CREATE INDEX one_coordinator_conversation ON conversations(runtime_role) WHERE runtime_role = 'coordinator';
CREATE TRIGGER conversations_role_scope_insert
BEFORE INSERT ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR ((NEW.runtime_role = 'coordinator') != (NEW.work_scope_id IS NULL))
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;
CREATE TRIGGER conversations_role_scope_update
BEFORE UPDATE OF runtime_role, work_scope_id ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR ((NEW.runtime_role = 'coordinator') != (NEW.work_scope_id IS NULL))
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;

ALTER TABLE work_scope_pr_associations RENAME TO work_scope_pr_associations_old;
CREATE TABLE work_scope_pr_associations (
    work_scope_id TEXT NOT NULL REFERENCES work_scopes_new(id) ON DELETE CASCADE,
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
    feedback_status TEXT NOT NULL DEFAULT 'open',
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (work_scope_id, repo_owner, repo_name, pr_number)
);
INSERT INTO work_scope_pr_associations
SELECT m.new_id, p.repo_owner, p.repo_name, p.pr_number, p.title, p.url, p.state, p.draft,
       p.display_state, p.base, p.head, p.github_updated_at, p.feedback_status, p.first_seen_at, p.last_seen_at
FROM work_scope_pr_associations_old p JOIN migration_052_scope_map m ON m.old_id = p.work_scope_id;
DROP TABLE work_scope_pr_associations_old;
CREATE INDEX IF NOT EXISTS idx_work_scope_pr_primary ON work_scope_pr_associations(work_scope_id, display_state, github_updated_at, last_seen_at);

ALTER TABLE work_scope_pr_feedback_baselines RENAME TO work_scope_pr_feedback_baselines_old;
CREATE TABLE work_scope_pr_feedback_baselines (
    work_scope_id TEXT NOT NULL REFERENCES work_scopes_new(id) ON DELETE CASCADE,
    repo_owner TEXT NOT NULL,
    repo_name TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    captured_at TEXT NOT NULL,
    github_updated_at TEXT,
    feedback_identities TEXT NOT NULL DEFAULT '[]',
    feedback_fingerprints TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (work_scope_id, repo_owner, repo_name, pr_number)
);
INSERT INTO work_scope_pr_feedback_baselines
SELECT m.new_id, b.repo_owner, b.repo_name, b.pr_number, b.captured_at, b.github_updated_at, b.feedback_identities, b.feedback_fingerprints
FROM work_scope_pr_feedback_baselines_old b JOIN migration_052_scope_map m ON m.old_id = b.work_scope_id;
DROP TABLE work_scope_pr_feedback_baselines_old;

ALTER TABLE work_scope_observed_branches RENAME TO work_scope_observed_branches_old;
CREATE TABLE work_scope_observed_branches (
    work_scope_id TEXT NOT NULL REFERENCES work_scopes_new(id) ON DELETE CASCADE,
    repository_identity TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    first_observed_head_oid TEXT NOT NULL,
    last_observed_head_oid TEXT NOT NULL,
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    PRIMARY KEY (work_scope_id, repository_identity, branch_name)
);
INSERT INTO work_scope_observed_branches
SELECT m.new_id, o.repository_identity, o.branch_name, o.first_observed_head_oid, o.last_observed_head_oid, o.first_observed_at, o.last_observed_at
FROM work_scope_observed_branches_old o JOIN migration_052_scope_map m ON m.old_id = o.work_scope_id;
DROP TABLE work_scope_observed_branches_old;
CREATE INDEX IF NOT EXISTS idx_work_scope_observed_branches_last_seen ON work_scope_observed_branches(work_scope_id, last_observed_at);

ALTER TABLE work_scope_active_pr_selection RENAME TO work_scope_active_pr_selection_old;
CREATE TABLE work_scope_active_pr_selection (
    work_scope_id TEXT PRIMARY KEY REFERENCES work_scopes_new(id) ON DELETE CASCADE,
    repo_owner TEXT,
    repo_name TEXT,
    pr_number INTEGER,
    provenance TEXT NOT NULL,
    latest_observed_repository_identity TEXT,
    latest_observed_branch_name TEXT,
    inference_generation INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    CHECK ((repo_owner IS NULL AND repo_name IS NULL AND pr_number IS NULL) OR (repo_owner IS NOT NULL AND repo_name IS NOT NULL AND pr_number IS NOT NULL))
);
INSERT INTO work_scope_active_pr_selection
SELECT m.new_id, a.repo_owner, a.repo_name, a.pr_number, a.provenance, a.latest_observed_repository_identity, a.latest_observed_branch_name, a.inference_generation, a.updated_at
FROM work_scope_active_pr_selection_old a JOIN migration_052_scope_map m ON m.old_id = a.work_scope_id;
DROP TABLE work_scope_active_pr_selection_old;

ALTER TABLE wake_terminal_receipt_tails RENAME TO wake_terminal_receipt_tails_old;
ALTER TABLE wake_terminal_receipts RENAME TO wake_terminal_receipts_old;
ALTER TABLE wake_delivery_messages RENAME TO wake_delivery_messages_old;
ALTER TABLE wake_bindings RENAME TO wake_bindings_old;

ALTER TABLE work_scopes_new RENAME TO work_scopes;

CREATE TABLE wake_bindings (
    workflow_id INTEGER PRIMARY KEY REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    contract_id TEXT NOT NULL CHECK (contract_id <> ''),
    profile_kind TEXT NOT NULL CHECK (profile_kind = 'wake'),
    profile_version INTEGER NOT NULL CHECK (profile_version >= 1),
    work_scope_id TEXT NOT NULL REFERENCES work_scopes(id),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('Bash', 'TmuxWindow')),
    bash_handle_id TEXT,
    tmux_server_token TEXT,
    tmux_window_id TEXT,
    registering_tool_use_id TEXT NOT NULL CHECK (registering_tool_use_id <> ''),
    expires_at INTEGER NOT NULL CHECK (expires_at >= 0),
    resolved_at INTEGER CHECK (resolved_at IS NULL OR resolved_at >= 0),
    prepared_fingerprint TEXT NOT NULL CHECK (prepared_fingerprint <> ''),
    fingerprint_needs_scope_upgrade INTEGER NOT NULL DEFAULT 0 CHECK (fingerprint_needs_scope_upgrade IN (0, 1)),
    observe_effect_id INTEGER NOT NULL CHECK (observe_effect_id >= 1),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    tmux_completion_policy TEXT NOT NULL DEFAULT 'KeepOpen' CHECK (tmux_completion_policy IN ('KeepOpen', 'CloseAfterCompletion')),
    FOREIGN KEY (workflow_id, observe_effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    CHECK ((resource_kind = 'Bash') = (bash_handle_id IS NOT NULL)),
    CHECK ((resource_kind = 'TmuxWindow') = (tmux_server_token IS NOT NULL AND tmux_window_id IS NOT NULL)),
    CHECK (NOT (resource_kind = 'Bash' AND (tmux_server_token IS NOT NULL OR tmux_window_id IS NOT NULL)))
) STRICT;
CREATE TEMP TABLE migration_052_wake_scope_map (
    workflow_id INTEGER PRIMARY KEY,
    work_scope_id TEXT
);
INSERT INTO migration_052_wake_scope_map
SELECT binding.workflow_id,
       COALESCE(
           (SELECT map.new_id
            FROM work_scopes_old old
            JOIN migration_052_scope_map map ON map.old_id = old.id
            WHERE old.scope_type = binding.scope_kind
              AND binding.scope_stable_key = lower(binding.scope_kind) || ':' || old.scope_value),
           (SELECT c.work_scope_id
            FROM conversations c
            WHERE c.id = binding.conversation_id)
       )
FROM wake_bindings_old binding;

DELETE FROM migration_052_guard;
INSERT INTO migration_052_guard
SELECT COUNT(*) FROM migration_052_wake_scope_map WHERE work_scope_id IS NULL;

INSERT INTO wake_bindings
SELECT old.workflow_id, old.conversation_id, old.contract_id, old.profile_kind, old.profile_version,
       mapped.work_scope_id,
       old.resource_kind, old.bash_handle_id, old.tmux_server_token, old.tmux_window_id, old.registering_tool_use_id,
       old.expires_at, old.resolved_at, old.prepared_fingerprint,
       CASE WHEN old.resolved_at IS NULL THEN 1 ELSE 0 END,
       old.observe_effect_id, old.created_at, old.tmux_completion_policy
FROM wake_bindings_old old
JOIN migration_052_wake_scope_map mapped ON mapped.workflow_id = old.workflow_id;

CREATE TABLE wake_terminal_receipts (
    workflow_id INTEGER NOT NULL,
    receipt_id INTEGER NOT NULL,
    delivery_id INTEGER NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    contract_id TEXT NOT NULL CHECK (contract_id <> ''),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('Bash', 'TmuxWindow')),
    terminal_kind TEXT NOT NULL CHECK (terminal_kind IN ('Fired', 'Cancelled', 'Expired', 'Forgotten')),
    resolved_at INTEGER NOT NULL CHECK (resolved_at >= 0),
    bash_handle_id TEXT,
    tmux_server_token TEXT,
    tmux_window_id TEXT,
    bash_status TEXT CHECK (bash_status IN ('Exited', 'Killed', 'KillPendingKernel')),
    tmux_status TEXT CHECK (tmux_status IN ('ExitMarkerObserved', 'WindowKilled')),
    occurred_at INTEGER CHECK (occurred_at IS NULL OR occurred_at >= 0),
    exit_code INTEGER,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    signal_number INTEGER,
    kill_signal_sent TEXT CHECK (kill_signal_sent IS NULL OR kill_signal_sent <> ''),
    forgotten_reason TEXT CHECK (forgotten_reason IN ('PhoenixRestart', 'CascadeDestroyedHandle', 'TmuxHandleMissing')),
    cancelled_reason TEXT CHECK (cancelled_reason IN ('ExplicitCancel')),
    cancelled_at INTEGER CHECK (cancelled_at IS NULL OR cancelled_at >= 0),
    PRIMARY KEY (workflow_id, receipt_id),
    FOREIGN KEY (workflow_id, receipt_id) REFERENCES workflow_receipts(workflow_id, receipt_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, delivery_id) REFERENCES workflow_deliveries(workflow_id, delivery_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id) REFERENCES wake_bindings(workflow_id) ON DELETE CASCADE,
    CHECK ((resource_kind = 'Bash') = (bash_handle_id IS NOT NULL)),
    CHECK ((resource_kind = 'TmuxWindow') = (tmux_server_token IS NOT NULL AND tmux_window_id IS NOT NULL)),
    CHECK (NOT (resource_kind = 'Bash' AND (tmux_server_token IS NOT NULL OR tmux_window_id IS NOT NULL))),
    CHECK (terminal_kind <> 'Fired' OR resource_kind <> 'Bash' OR bash_status IS NOT NULL),
    CHECK (terminal_kind <> 'Fired' OR resource_kind <> 'TmuxWindow' OR tmux_status IS NOT NULL),
    CHECK ((terminal_kind = 'Fired') = (occurred_at IS NOT NULL)),
    CHECK ((bash_status IS NOT NULL) = (resource_kind = 'Bash' AND terminal_kind = 'Fired')),
    CHECK ((tmux_status IS NOT NULL) = (resource_kind = 'TmuxWindow' AND terminal_kind = 'Fired')),
    CHECK ((kill_signal_sent IS NOT NULL) <= (bash_status IS NOT NULL)),
    CHECK ((forgotten_reason IS NOT NULL) = (terminal_kind = 'Forgotten')),
    CHECK ((cancelled_reason IS NOT NULL) = (terminal_kind = 'Cancelled')),
    CHECK ((cancelled_at IS NOT NULL) = (terminal_kind = 'Cancelled'))
) WITHOUT ROWID;
INSERT INTO wake_terminal_receipts SELECT * FROM wake_terminal_receipts_old;

CREATE TABLE wake_terminal_receipt_tails (
    workflow_id INTEGER NOT NULL,
    receipt_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    line TEXT NOT NULL,
    PRIMARY KEY (workflow_id, receipt_id, ordinal),
    FOREIGN KEY (workflow_id, receipt_id) REFERENCES wake_terminal_receipts(workflow_id, receipt_id) ON DELETE CASCADE
) WITHOUT ROWID;
INSERT INTO wake_terminal_receipt_tails SELECT * FROM wake_terminal_receipt_tails_old;

CREATE TABLE wake_delivery_messages (
    workflow_id INTEGER NOT NULL,
    delivery_id INTEGER NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(message_id) ON DELETE CASCADE,
    registering_tool_use_id TEXT NOT NULL CHECK (registering_tool_use_id <> ''),
    terminal_kind TEXT NOT NULL CHECK (terminal_kind IN ('Fired', 'Cancelled', 'Expired', 'Forgotten')),
    auto_resume INTEGER NOT NULL CHECK (auto_resume IN (0, 1)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (workflow_id, delivery_id),
    FOREIGN KEY (workflow_id, delivery_id) REFERENCES workflow_deliveries(workflow_id, delivery_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id) REFERENCES wake_bindings(workflow_id) ON DELETE CASCADE
) WITHOUT ROWID;
INSERT INTO wake_delivery_messages SELECT * FROM wake_delivery_messages_old;
DROP TABLE wake_terminal_receipt_tails_old;
DROP TABLE wake_terminal_receipts_old;
DROP TABLE wake_delivery_messages_old;
DROP TABLE wake_bindings_old;
CREATE UNIQUE INDEX wake_bindings_contract_identity ON wake_bindings(profile_kind, profile_version, conversation_id, contract_id, resource_kind, COALESCE(bash_handle_id, ''), COALESCE(tmux_server_token, ''), COALESCE(tmux_window_id, ''));
CREATE UNIQUE INDEX wake_bindings_resource_identity ON wake_bindings(profile_kind, profile_version, conversation_id, resource_kind, COALESCE(bash_handle_id, ''), COALESCE(tmux_server_token, ''), COALESCE(tmux_window_id, '')) WHERE resolved_at IS NULL;
CREATE INDEX wake_bindings_by_conversation ON wake_bindings(conversation_id);
CREATE INDEX wake_bindings_by_work_scope ON wake_bindings(work_scope_id);
CREATE INDEX wake_bindings_active_unresolved ON wake_bindings(expires_at, workflow_id);
CREATE INDEX wake_terminal_receipts_by_conversation ON wake_terminal_receipts(conversation_id, delivery_id);
CREATE INDEX wake_terminal_receipts_by_workflow_delivery ON wake_terminal_receipts(workflow_id, delivery_id);
CREATE INDEX wake_delivery_messages_by_conversation ON wake_delivery_messages(conversation_id, delivery_id);

DROP TABLE coordinator;
DROP TABLE work_scopes_old;
DROP TABLE migration_052_scope_map;
DROP TABLE migration_052_generated_scope_map;
DROP TABLE migration_052_lineage;
DROP TABLE migration_052_root_scope;
DROP TABLE migration_052_conversation_scope;
DROP TABLE migration_052_environment_candidates;
DROP TABLE migration_052_environment_representative;
DROP TABLE migration_052_wake_scope_map;
DROP TABLE migration_052_guard;
";

const MIGRATION_050: &str = r"
ALTER TABLE wake_bindings
ADD COLUMN tmux_completion_policy TEXT NOT NULL DEFAULT 'KeepOpen'
CHECK (tmux_completion_policy IN ('KeepOpen', 'CloseAfterCompletion'));
";

const MIGRATION_049: &str = r"
CREATE TABLE wake_delivery_messages (
    workflow_id INTEGER NOT NULL,
    delivery_id INTEGER NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(message_id) ON DELETE CASCADE,
    registering_tool_use_id TEXT NOT NULL CHECK (registering_tool_use_id <> ''),
    terminal_kind TEXT NOT NULL CHECK (terminal_kind IN ('Fired', 'Cancelled', 'Expired', 'Forgotten')),
    auto_resume INTEGER NOT NULL CHECK (auto_resume IN (0, 1)),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (workflow_id, delivery_id),
    FOREIGN KEY (workflow_id, delivery_id) REFERENCES workflow_deliveries(workflow_id, delivery_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id) REFERENCES wake_bindings(workflow_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX wake_delivery_messages_by_conversation
ON wake_delivery_messages(conversation_id, delivery_id);
";

const MIGRATION_048: &str = r"
CREATE TABLE wake_terminal_receipts (
    workflow_id INTEGER NOT NULL,
    receipt_id INTEGER NOT NULL,
    delivery_id INTEGER NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    contract_id TEXT NOT NULL CHECK (contract_id <> ''),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('Bash', 'TmuxWindow')),
    terminal_kind TEXT NOT NULL CHECK (terminal_kind IN ('Fired', 'Cancelled', 'Expired', 'Forgotten')),
    resolved_at INTEGER NOT NULL CHECK (resolved_at >= 0),
    bash_handle_id TEXT,
    tmux_server_token TEXT,
    tmux_window_id TEXT,
    bash_status TEXT CHECK (bash_status IN ('Exited', 'Killed', 'KillPendingKernel')),
    tmux_status TEXT CHECK (tmux_status IN ('ExitMarkerObserved', 'WindowKilled')),
    occurred_at INTEGER CHECK (occurred_at IS NULL OR occurred_at >= 0),
    exit_code INTEGER,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    signal_number INTEGER,
    kill_signal_sent TEXT CHECK (kill_signal_sent IS NULL OR kill_signal_sent <> ''),
    forgotten_reason TEXT CHECK (forgotten_reason IN ('PhoenixRestart', 'CascadeDestroyedHandle', 'TmuxHandleMissing')),
    cancelled_reason TEXT CHECK (cancelled_reason IN ('ExplicitCancel')),
    cancelled_at INTEGER CHECK (cancelled_at IS NULL OR cancelled_at >= 0),
    PRIMARY KEY (workflow_id, receipt_id),
    FOREIGN KEY (workflow_id, receipt_id) REFERENCES workflow_receipts(workflow_id, receipt_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, delivery_id) REFERENCES workflow_deliveries(workflow_id, delivery_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id) REFERENCES wake_bindings(workflow_id) ON DELETE CASCADE,
    CHECK ((resource_kind = 'Bash') = (bash_handle_id IS NOT NULL)),
    CHECK ((resource_kind = 'TmuxWindow') = (tmux_server_token IS NOT NULL AND tmux_window_id IS NOT NULL)),
    CHECK (NOT (resource_kind = 'Bash' AND (tmux_server_token IS NOT NULL OR tmux_window_id IS NOT NULL))),
    CHECK (terminal_kind <> 'Fired' OR resource_kind <> 'Bash' OR bash_status IS NOT NULL),
    CHECK (terminal_kind <> 'Fired' OR resource_kind <> 'TmuxWindow' OR tmux_status IS NOT NULL),
    CHECK ((terminal_kind = 'Fired') = (occurred_at IS NOT NULL)),
    CHECK ((bash_status IS NOT NULL) = (resource_kind = 'Bash' AND terminal_kind = 'Fired')),
    CHECK ((tmux_status IS NOT NULL) = (resource_kind = 'TmuxWindow' AND terminal_kind = 'Fired')),
    CHECK ((kill_signal_sent IS NOT NULL) <= (bash_status IS NOT NULL)),
    CHECK ((forgotten_reason IS NOT NULL) = (terminal_kind = 'Forgotten')),
    CHECK ((cancelled_reason IS NOT NULL) = (terminal_kind = 'Cancelled')),
    CHECK ((cancelled_at IS NOT NULL) = (terminal_kind = 'Cancelled'))
) WITHOUT ROWID;

CREATE TABLE wake_terminal_receipt_tails (
    workflow_id INTEGER NOT NULL,
    receipt_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    line TEXT NOT NULL,
    PRIMARY KEY (workflow_id, receipt_id, ordinal),
    FOREIGN KEY (workflow_id, receipt_id) REFERENCES wake_terminal_receipts(workflow_id, receipt_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX wake_terminal_receipts_by_conversation
ON wake_terminal_receipts(conversation_id, delivery_id);
CREATE INDEX wake_terminal_receipts_by_workflow_delivery
ON wake_terminal_receipts(workflow_id, delivery_id);
";

const MIGRATION_047: &str = r"
CREATE TABLE wake_bindings (
    workflow_id INTEGER PRIMARY KEY REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    contract_id TEXT NOT NULL CHECK (contract_id <> ''),
    profile_kind TEXT NOT NULL CHECK (profile_kind = 'wake'),
    profile_version INTEGER NOT NULL CHECK (profile_version >= 1),
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('Conversation', 'Worktree')),
    scope_stable_key TEXT NOT NULL CHECK (scope_stable_key <> ''),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('Bash', 'TmuxWindow')),
    bash_handle_id TEXT,
    tmux_server_token TEXT,
    tmux_window_id TEXT,
    registering_tool_use_id TEXT NOT NULL CHECK (registering_tool_use_id <> ''),
    expires_at INTEGER NOT NULL CHECK (expires_at >= 0),
    resolved_at INTEGER CHECK (resolved_at IS NULL OR resolved_at >= 0),
    prepared_fingerprint TEXT NOT NULL CHECK (prepared_fingerprint <> ''),
    observe_effect_id INTEGER NOT NULL CHECK (observe_effect_id >= 1),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    FOREIGN KEY (workflow_id, observe_effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    CHECK ((resource_kind = 'Bash') = (bash_handle_id IS NOT NULL)),
    CHECK ((resource_kind = 'TmuxWindow') = (tmux_server_token IS NOT NULL AND tmux_window_id IS NOT NULL)),
    CHECK (NOT (resource_kind = 'Bash' AND (tmux_server_token IS NOT NULL OR tmux_window_id IS NOT NULL)))
) STRICT;

CREATE UNIQUE INDEX wake_bindings_contract_identity
ON wake_bindings(
    profile_kind, profile_version, conversation_id, contract_id, resource_kind,
    COALESCE(bash_handle_id, ''), COALESCE(tmux_server_token, ''), COALESCE(tmux_window_id, '')
);
CREATE UNIQUE INDEX wake_bindings_resource_identity
ON wake_bindings(
    profile_kind, profile_version, conversation_id, resource_kind,
    COALESCE(bash_handle_id, ''), COALESCE(tmux_server_token, ''), COALESCE(tmux_window_id, '')
)
WHERE resolved_at IS NULL;
CREATE INDEX wake_bindings_by_conversation ON wake_bindings(conversation_id);
CREATE INDEX wake_bindings_active_unresolved
ON wake_bindings(expires_at, workflow_id);
";

const MIGRATION_046: &str = r"
CREATE TABLE workflows (
    workflow_id INTEGER PRIMARY KEY,
    profile_kind TEXT NOT NULL CHECK (profile_kind <> ''),
    profile_version INTEGER NOT NULL CHECK (profile_version >= 1),
    runtime_acceptance_enabled INTEGER NOT NULL CHECK (runtime_acceptance_enabled IN (0, 1)),
    external_acceptance_enabled INTEGER NOT NULL CHECK (external_acceptance_enabled IN (0, 1)),
    version INTEGER NOT NULL CHECK (version >= 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    status TEXT NOT NULL CHECK (status IN (
        'Active', 'Cancelling', 'ManualResolution', 'Incompatible',
        'Cancelled', 'DeletionPending', 'Deleted', 'Completed', 'Failed'
    )),
    snapshot_codec_family TEXT NOT NULL CHECK (snapshot_codec_family <> ''),
    snapshot_codec_version INTEGER NOT NULL CHECK (snapshot_codec_version >= 1),
    snapshot_payload BLOB NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
);

CREATE TABLE workflow_supported_codecs (
    workflow_id INTEGER NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    codec_family TEXT NOT NULL CHECK (codec_family <> ''),
    codec_version INTEGER NOT NULL CHECK (codec_version >= 1),
    PRIMARY KEY (workflow_id, codec_family, codec_version)
) WITHOUT ROWID;

CREATE TABLE workflow_transitions (
    workflow_id INTEGER NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    transition_id INTEGER NOT NULL CHECK (transition_id >= 1),
    from_version INTEGER NOT NULL CHECK (from_version >= 0),
    to_version INTEGER NOT NULL CHECK (to_version >= 1),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    event_codec_family TEXT NOT NULL CHECK (event_codec_family <> ''),
    event_codec_version INTEGER NOT NULL CHECK (event_codec_version >= 1),
    event_payload BLOB NOT NULL,
    committed_at INTEGER NOT NULL CHECK (committed_at >= 0),
    PRIMARY KEY (workflow_id, transition_id),
    UNIQUE (workflow_id, to_version),
    CHECK (to_version = from_version + 1)
) WITHOUT ROWID;

CREATE TABLE workflow_effects (
    workflow_id INTEGER NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    effect_id INTEGER NOT NULL CHECK (effect_id >= 1),
    declared_workflow_version INTEGER NOT NULL CHECK (declared_workflow_version >= 0),
    family TEXT NOT NULL CHECK (family <> ''),
    kind TEXT NOT NULL CHECK (kind <> ''),
    intent_codec_family TEXT NOT NULL CHECK (intent_codec_family <> ''),
    intent_codec_version INTEGER NOT NULL CHECK (intent_codec_version >= 1),
    intent_payload BLOB NOT NULL,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    role TEXT NOT NULL CHECK (role IN ('Required', 'Optional', 'Compensation')),
    capability_kind TEXT NOT NULL CHECK (capability_kind IN (
        'ReclaimableObservation', 'IdempotentSubmission', 'ObservableSubmission',
        'SafelyRepeatable', 'ManualOnAmbiguity'
    )),
    stable_command_id INTEGER,
    next_eligible_at INTEGER CHECK (next_eligible_at >= 0),
    destructive_resource TEXT CHECK (destructive_resource IS NULL OR destructive_resource <> ''),
    status TEXT NOT NULL CHECK (status IN (
        'Blocked', 'Eligible', 'Executing', 'RetryWait', 'AmbiguityWait',
        'Receipted', 'Invalidated'
    )),
    pending_reconciliation INTEGER NOT NULL DEFAULT 0 CHECK (pending_reconciliation IN (0, 1)),
    PRIMARY KEY (workflow_id, effect_id),
    CHECK ((capability_kind IN ('IdempotentSubmission', 'ObservableSubmission')) = (stable_command_id IS NOT NULL))
) WITHOUT ROWID;

CREATE TABLE workflow_effect_dependencies (
    workflow_id INTEGER NOT NULL,
    effect_id INTEGER NOT NULL,
    depends_on_effect_id INTEGER NOT NULL,
    PRIMARY KEY (workflow_id, effect_id, depends_on_effect_id),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, depends_on_effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    CHECK (effect_id <> depends_on_effect_id)
) WITHOUT ROWID;

CREATE TABLE workflow_barriers (
    workflow_id INTEGER NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    barrier_id INTEGER NOT NULL CHECK (barrier_id >= 1),
    status TEXT NOT NULL CHECK (status IN ('Waiting', 'Satisfied')),
    reducer_event_codec_family TEXT NOT NULL CHECK (reducer_event_codec_family <> ''),
    reducer_event_codec_version INTEGER NOT NULL CHECK (reducer_event_codec_version >= 1),
    reducer_event_payload BLOB NOT NULL,
    PRIMARY KEY (workflow_id, barrier_id)
) WITHOUT ROWID;

CREATE TABLE workflow_barrier_members (
    workflow_id INTEGER NOT NULL,
    barrier_id INTEGER NOT NULL,
    effect_id INTEGER NOT NULL,
    receipt_family TEXT NOT NULL CHECK (receipt_family IN ('CurrentGenerationEffect', 'CompensationEffect')),
    PRIMARY KEY (workflow_id, barrier_id, effect_id),
    FOREIGN KEY (workflow_id, barrier_id) REFERENCES workflow_barriers(workflow_id, barrier_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE workflow_sequences (
    workflow_id INTEGER NOT NULL,
    sequence_name TEXT NOT NULL CHECK (sequence_name <> ''),
    next_value INTEGER NOT NULL CHECK (next_value >= 1),
    PRIMARY KEY (workflow_id, sequence_name),
    FOREIGN KEY (workflow_id) REFERENCES workflows(workflow_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE workflow_global_sequences (
    sequence_name TEXT NOT NULL CHECK (sequence_name <> ''),
    next_value INTEGER NOT NULL CHECK (next_value >= 1),
    PRIMARY KEY (sequence_name)
) WITHOUT ROWID;

CREATE TABLE workflow_attempts (
    workflow_id INTEGER NOT NULL,
    effect_id INTEGER NOT NULL,
    attempt_id INTEGER NOT NULL CHECK (attempt_id >= 1),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    declared_workflow_version INTEGER NOT NULL CHECK (declared_workflow_version >= 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    process_incarnation INTEGER NOT NULL CHECK (process_incarnation >= 0),
    status TEXT NOT NULL CHECK (status IN ('Begun', 'ObservationRecorded', 'ReceiptAccepted', 'AuthorityLost')),
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (workflow_id, attempt_id),
    UNIQUE (workflow_id, effect_id, generation, ordinal),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE UNIQUE INDEX workflow_attempts_one_live_per_effect
ON workflow_attempts(workflow_id, effect_id)
WHERE status IN ('Begun', 'ObservationRecorded');

CREATE INDEX workflow_attempts_observation_restart
ON workflow_attempts(workflow_id, effect_id, status, created_at, attempt_id);

CREATE TABLE workflow_reclaimable_leases (
    workflow_id INTEGER NOT NULL,
    attempt_id INTEGER NOT NULL,
    lease_until INTEGER NOT NULL CHECK (lease_until >= 0),
    PRIMARY KEY (workflow_id, attempt_id),
    FOREIGN KEY (workflow_id, attempt_id) REFERENCES workflow_attempts(workflow_id, attempt_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX workflow_reclaimable_leases_by_expiry
ON workflow_reclaimable_leases(lease_until, workflow_id, attempt_id);

CREATE TABLE workflow_authoritative_observations (
    workflow_id INTEGER NOT NULL,
    observation_id INTEGER NOT NULL CHECK (observation_id >= 1),
    effect_id INTEGER NOT NULL,
    attempt_id INTEGER NOT NULL,
    declared_workflow_version INTEGER NOT NULL CHECK (declared_workflow_version >= 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    process_incarnation INTEGER NOT NULL CHECK (process_incarnation >= 0),
    observation_codec_family TEXT NOT NULL CHECK (observation_codec_family <> ''),
    observation_codec_version INTEGER NOT NULL CHECK (observation_codec_version >= 1),
    observation_payload BLOB NOT NULL,
    observed_at INTEGER NOT NULL CHECK (observed_at >= 0),
    recorded_at INTEGER NOT NULL CHECK (recorded_at >= 0),
    PRIMARY KEY (workflow_id, observation_id),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, attempt_id) REFERENCES workflow_attempts(workflow_id, attempt_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE workflow_stale_observations (
    workflow_id INTEGER NOT NULL,
    observation_id INTEGER NOT NULL CHECK (observation_id >= 1),
    effect_id INTEGER NOT NULL,
    attempt_id INTEGER NOT NULL,
    declared_workflow_version INTEGER NOT NULL CHECK (declared_workflow_version >= 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    process_incarnation INTEGER NOT NULL CHECK (process_incarnation >= 0),
    observation_codec_family TEXT NOT NULL CHECK (observation_codec_family <> ''),
    observation_codec_version INTEGER NOT NULL CHECK (observation_codec_version >= 1),
    observation_payload BLOB NOT NULL,
    observed_at INTEGER NOT NULL CHECK (observed_at >= 0),
    recorded_at INTEGER NOT NULL CHECK (recorded_at >= 0),
    PRIMARY KEY (workflow_id, observation_id),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE workflow_receipts (
    workflow_id INTEGER NOT NULL,
    receipt_id INTEGER NOT NULL CHECK (receipt_id >= 1),
    effect_id INTEGER NOT NULL,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    declared_workflow_version INTEGER NOT NULL CHECK (declared_workflow_version >= 0),
    process_incarnation INTEGER NOT NULL CHECK (process_incarnation >= 0),
    attempt_id INTEGER,
    origin TEXT NOT NULL CHECK (origin IN (
        'Execution', 'Adoption', 'Reconciliation', 'Manual',
        'CancellationArbitration', 'DeadlineExpiration', 'ForgottenInterruption', 'ScheduleCollapse'
    )),
    receipt_codec_family TEXT NOT NULL CHECK (receipt_codec_family <> ''),
    receipt_codec_version INTEGER NOT NULL CHECK (receipt_codec_version >= 1),
    receipt_payload BLOB NOT NULL,
    PRIMARY KEY (workflow_id, receipt_id),
    UNIQUE (workflow_id, effect_id, generation),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, attempt_id) REFERENCES workflow_attempts(workflow_id, attempt_id) ON DELETE RESTRICT
) WITHOUT ROWID;

CREATE TABLE workflow_deliveries (
    workflow_id INTEGER NOT NULL,
    delivery_id INTEGER NOT NULL CHECK (delivery_id >= 1),
    effect_id INTEGER,
    barrier_id INTEGER,
    consumer_kind TEXT NOT NULL CHECK (consumer_kind <> ''),
    event_codec_family TEXT NOT NULL CHECK (event_codec_family <> ''),
    event_codec_version INTEGER NOT NULL CHECK (event_codec_version >= 1),
    payload_kind TEXT NOT NULL CHECK (payload_kind IN ('Receipt', 'Barrier')),
    payload_blob BLOB NOT NULL,
    requires_runtime_acceptance INTEGER NOT NULL CHECK (requires_runtime_acceptance IN (0, 1)),
    status TEXT NOT NULL CHECK (status IN ('Pending', 'Deferred', 'Accepted', 'Suppressed')),
    runtime_acceptance_status TEXT CHECK (runtime_acceptance_status IN ('Owed', 'Accepted', 'Suppressed')),
    suppression_reason TEXT CHECK (suppression_reason IN ('Cancelled', 'Superseded', 'LifecycleTerminal', 'ReducerTerminal')),
    accepted_by_transition_id INTEGER,
    PRIMARY KEY (workflow_id, delivery_id),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, barrier_id) REFERENCES workflow_barriers(workflow_id, barrier_id) ON DELETE CASCADE,
    FOREIGN KEY (workflow_id, accepted_by_transition_id) REFERENCES workflow_transitions(workflow_id, transition_id) ON DELETE RESTRICT,
    CHECK ((effect_id IS NOT NULL) <> (barrier_id IS NOT NULL)),
    CHECK ((status = 'Accepted') = (accepted_by_transition_id IS NOT NULL)),
    CHECK ((status = 'Suppressed') = (suppression_reason IS NOT NULL)),
    CHECK ((requires_runtime_acceptance = 1) = (runtime_acceptance_status IS NOT NULL)),
    CHECK (NOT (requires_runtime_acceptance = 0 AND accepted_by_transition_id IS NOT NULL AND status = 'Pending')),
    CHECK (
        (status = 'Pending' AND (
            runtime_acceptance_status IS NULL OR runtime_acceptance_status = 'Owed'
        )) OR
        (status = 'Accepted' AND (
            runtime_acceptance_status IS NULL OR runtime_acceptance_status = 'Accepted'
        )) OR
        (status = 'Suppressed' AND (
            runtime_acceptance_status IS NULL OR runtime_acceptance_status = 'Suppressed'
        ))
    )
) WITHOUT ROWID;

CREATE INDEX workflow_deliveries_pending_global
ON workflow_deliveries(status, workflow_id, delivery_id);

CREATE TABLE workflow_manual_resolutions (
    workflow_id INTEGER NOT NULL,
    manual_resolution_id INTEGER NOT NULL CHECK (manual_resolution_id >= 1),
    workflow_version INTEGER NOT NULL CHECK (workflow_version >= 0),
    effect_id INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('Required', 'Resolved')),
    accepted_choice_ordinal INTEGER,
    resolved_by TEXT CHECK (resolved_by IS NULL OR resolved_by <> ''),
    PRIMARY KEY (workflow_id, manual_resolution_id),
    FOREIGN KEY (workflow_id, effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE workflow_manual_resolution_choices (
    workflow_id INTEGER NOT NULL,
    manual_resolution_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('Retry', 'Compensate', 'Suppress', 'AcceptAsTerminal')),
    payload_codec_family TEXT NOT NULL CHECK (payload_codec_family <> ''),
    payload_codec_version INTEGER NOT NULL CHECK (payload_codec_version >= 1),
    payload_blob BLOB NOT NULL,
    receipt_codec_family TEXT NOT NULL CHECK (receipt_codec_family <> ''),
    receipt_codec_version INTEGER NOT NULL CHECK (receipt_codec_version >= 1),
    receipt_blob BLOB NOT NULL,
    receipt_event_codec_family TEXT NOT NULL CHECK (receipt_event_codec_family <> ''),
    receipt_event_codec_version INTEGER NOT NULL CHECK (receipt_event_codec_version >= 1),
    receipt_event_blob BLOB NOT NULL,
    PRIMARY KEY (workflow_id, manual_resolution_id, ordinal),
    FOREIGN KEY (workflow_id, manual_resolution_id) REFERENCES workflow_manual_resolutions(workflow_id, manual_resolution_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE workflow_external_acceptance_bindings (
    profile_kind TEXT NOT NULL CHECK (profile_kind <> ''),
    profile_version INTEGER NOT NULL CHECK (profile_version >= 1),
    target_scope TEXT NOT NULL CHECK (target_scope <> ''),
    idempotency_key TEXT NOT NULL CHECK (idempotency_key <> ''),
    intent_fingerprint TEXT NOT NULL CHECK (intent_fingerprint <> ''),
    workflow_id INTEGER NOT NULL UNIQUE REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    receipt_handle BLOB NOT NULL,
    disposition_handle BLOB NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    PRIMARY KEY (profile_kind, profile_version, target_scope, idempotency_key)
) WITHOUT ROWID;

CREATE TABLE workflow_schedules (
    workflow_id INTEGER NOT NULL REFERENCES workflows(workflow_id) ON DELETE CASCADE,
    schedule_id INTEGER NOT NULL CHECK (schedule_id >= 1),
    policy TEXT NOT NULL CHECK (policy IN ('CoalesceLatest')),
    schedule_key TEXT NOT NULL CHECK (schedule_key <> ''),
    status TEXT NOT NULL CHECK (status IN ('Idle', 'Due', 'Active')),
    next_eligible_at INTEGER NOT NULL CHECK (next_eligible_at >= 0),
    active_effect_id INTEGER,
    due_occurrence_id INTEGER,
    due_generation INTEGER CHECK (due_generation IS NULL OR due_generation >= 0),
    due_at INTEGER CHECK (due_at IS NULL OR due_at >= 0),
    active_occurrence_id INTEGER,
    active_generation INTEGER CHECK (active_generation IS NULL OR active_generation >= 0),
    active_due_at INTEGER CHECK (active_due_at IS NULL OR active_due_at >= 0),
    PRIMARY KEY (workflow_id, schedule_id),
    UNIQUE (workflow_id, schedule_key),
    FOREIGN KEY (workflow_id, active_effect_id) REFERENCES workflow_effects(workflow_id, effect_id) ON DELETE SET NULL,
    CHECK ((due_occurrence_id IS NULL) = (due_generation IS NULL AND due_at IS NULL)),
    CHECK ((active_occurrence_id IS NULL) = (active_generation IS NULL AND active_due_at IS NULL))
) WITHOUT ROWID;

CREATE TRIGGER workflow_receipts_origin_attempt_shape
BEFORE INSERT ON workflow_receipts
FOR EACH ROW
WHEN (NEW.origin = 'Execution' AND NEW.attempt_id IS NULL)
   OR (NEW.origin NOT IN ('Execution', 'CancellationArbitration') AND NEW.attempt_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'workflow_receipts origin/attempt mismatch');
END;

CREATE TRIGGER workflow_receipts_attempt_capability
BEFORE INSERT ON workflow_receipts
FOR EACH ROW
WHEN NEW.attempt_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM workflow_effects effects
    JOIN workflow_attempts attempts
      ON attempts.workflow_id = NEW.workflow_id
     AND attempts.attempt_id = NEW.attempt_id
     AND attempts.effect_id = effects.effect_id
    WHERE effects.workflow_id = NEW.workflow_id
      AND effects.effect_id = NEW.effect_id
      AND effects.capability_kind IN (
          'ReclaimableObservation', 'IdempotentSubmission',
          'ObservableSubmission', 'SafelyRepeatable', 'ManualOnAmbiguity'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_receipts attempt capability mismatch');
END;

CREATE TRIGGER workflow_reclaimable_leases_capability
BEFORE INSERT ON workflow_reclaimable_leases
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM workflow_attempts attempts
    JOIN workflow_effects effects
      ON effects.workflow_id = attempts.workflow_id
     AND effects.effect_id = attempts.effect_id
    WHERE attempts.workflow_id = NEW.workflow_id
      AND attempts.attempt_id = NEW.attempt_id
      AND effects.capability_kind = 'ReclaimableObservation'
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_reclaimable_leases requires reclaimable capability');
END;

CREATE TRIGGER workflow_authoritative_observations_attempt_shape
BEFORE INSERT ON workflow_authoritative_observations
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM workflow_attempts attempts
    WHERE attempts.workflow_id = NEW.workflow_id
      AND attempts.attempt_id = NEW.attempt_id
      AND attempts.effect_id = NEW.effect_id
      AND attempts.generation = NEW.generation
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_authoritative_observations attempt mismatch');
END;

CREATE TRIGGER workflow_stale_observations_attempt_shape
BEFORE INSERT ON workflow_stale_observations
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM workflow_attempts attempts
    WHERE attempts.workflow_id = NEW.workflow_id
      AND attempts.attempt_id = NEW.attempt_id
      AND attempts.effect_id = NEW.effect_id
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_stale_observations attempt mismatch');
END;

CREATE TRIGGER workflow_manual_resolutions_choice_fk
BEFORE UPDATE OF accepted_choice_ordinal ON workflow_manual_resolutions
FOR EACH ROW
WHEN NEW.accepted_choice_ordinal IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM workflow_manual_resolution_choices choices
    WHERE choices.workflow_id = NEW.workflow_id
      AND choices.manual_resolution_id = NEW.manual_resolution_id
      AND choices.ordinal = NEW.accepted_choice_ordinal
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_manual_resolutions accepted choice missing');
END;

CREATE TRIGGER workflow_schedules_occurrence_shape
BEFORE INSERT ON workflow_schedules
FOR EACH ROW
WHEN NOT (
    (NEW.status = 'Idle' AND NEW.due_occurrence_id IS NULL AND NEW.active_occurrence_id IS NULL) OR
    (NEW.status = 'Due' AND NEW.due_occurrence_id IS NOT NULL AND NEW.active_occurrence_id IS NULL) OR
    (NEW.status = 'Active' AND NEW.due_occurrence_id IS NULL AND NEW.active_occurrence_id IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_schedules occurrence shape mismatch');
END;

CREATE TRIGGER workflow_schedules_occurrence_shape_update
BEFORE UPDATE ON workflow_schedules
FOR EACH ROW
WHEN NOT (
    (NEW.status = 'Idle' AND NEW.due_occurrence_id IS NULL AND NEW.active_occurrence_id IS NULL) OR
    (NEW.status = 'Due' AND NEW.due_occurrence_id IS NOT NULL AND NEW.active_occurrence_id IS NULL) OR
    (NEW.status = 'Active' AND NEW.due_occurrence_id IS NULL AND NEW.active_occurrence_id IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'workflow_schedules occurrence shape mismatch');
END;
";

const MIGRATION_051: &str = r"
CREATE TABLE message_fts_rows (
    fts_rowid INTEGER PRIMARY KEY,
    message_id TEXT NOT NULL,
    chunk_ordinal INTEGER NOT NULL,
    conversation_id TEXT NOT NULL,
    message_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    content_hash TEXT NOT NULL
);
CREATE INDEX idx_message_fts_rows_message_id
    ON message_fts_rows(message_id);
CREATE INDEX idx_message_fts_rows_conversation_id
    ON message_fts_rows(conversation_id);
INSERT INTO message_fts_rows
    (fts_rowid, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash)
SELECT rowid, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash
FROM message_fts;

CREATE VIRTUAL TABLE message_fts_text USING fts5(
    text,
    tokenize = 'porter unicode61 remove_diacritics 2'
);
INSERT INTO message_fts_text (rowid, text)
SELECT rowid, text FROM message_fts;
DROP TABLE message_fts;
ALTER TABLE message_fts_text RENAME TO message_fts;
";

const MIGRATION_044: &str = r"
CREATE TABLE coordinator (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    conversation_id TEXT NOT NULL UNIQUE
        REFERENCES conversations(id) ON DELETE RESTRICT
);
DROP TABLE IF EXISTS global_recall_messages;
DROP TABLE IF EXISTS global_recall_sessions;
";

const MIGRATION_045: &str = r"
CREATE TABLE continuation_dispatch_intents (
    parent_conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    successor_conversation_id TEXT UNIQUE NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT UNIQUE NOT NULL,
    handoff TEXT NOT NULL CHECK (length(trim(handoff)) > 0),
    user_agent TEXT,
    created_at TEXT NOT NULL
);
CREATE TRIGGER consume_continuation_dispatch_intent
AFTER INSERT ON messages
WHEN EXISTS (
    SELECT 1 FROM continuation_dispatch_intents
    WHERE message_id = NEW.message_id
      AND successor_conversation_id = NEW.conversation_id
)
BEGIN
    DELETE FROM continuation_dispatch_intents WHERE message_id = NEW.message_id;
END;
";

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
/// Migration 18's original shape carries provenance as `UNINDEXED` columns;
/// migration 51 normalizes that metadata into an indexed row-locator table and
/// rebuilds this virtual table with `text` as its sole column. The initial
/// migration creates the empty structure; the typed backfill from existing
/// `messages` is performed by the Rust startup
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
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap()
    }

    async fn setup_legacy_conversations_table(pool: &SqlitePool) {
        sqlx::raw_sql(crate::ddl::SCHEMA)
            .execute(pool)
            .await
            .unwrap();
        sqlx::raw_sql(crate::ddl::MIGRATION_TYPED_STATE)
            .execute(pool)
            .await
            .unwrap();
        let _ = sqlx::raw_sql("ALTER TABLE conversations DROP COLUMN state_data")
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN model TEXT")
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql(crate::ddl::MIGRATION_RENAME_MESSAGE_ID)
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql(crate::ddl::MIGRATION_REMOVE_UNKNOWN_ERROR_KIND)
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql(crate::ddl::MIGRATION_CREATE_PROJECTS)
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql(
            "ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id)",
        )
        .execute(pool)
        .await;
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN title TEXT")
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN desired_base_branch TEXT")
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN seed_parent_id TEXT")
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql("ALTER TABLE conversations ADD COLUMN seed_label TEXT")
            .execute(pool)
            .await;
        let _ = sqlx::raw_sql(
            "ALTER TABLE conversations ADD COLUMN steering_queue TEXT NOT NULL DEFAULT '[]'",
        )
        .execute(pool)
        .await;
    }

    #[allow(dead_code)]
    async fn setup_legacy_schema_through(pool: &SqlitePool, version: u32) {
        setup_legacy_conversations_table(pool).await;
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= version)
        {
            sqlx::raw_sql(migration.sql).execute(pool).await.unwrap();
        }
    }

    #[allow(dead_code)]
    async fn stamp_migrations_except(pool: &SqlitePool, excluded_version: u32) {
        sqlx::query("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(pool)
            .await
            .unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version != excluded_version)
        {
            sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
                .bind(migration.version)
                .bind(migration.name)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    async fn setup_workflow_only_schema(pool: &SqlitePool) {
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
        sqlx::query("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(pool)
            .await
            .unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version != 46)
        {
            sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
                .bind(migration.version)
                .bind(migration.name)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    async fn setup_conversations_table(pool: &SqlitePool) {
        setup_legacy_conversations_table(pool).await;
    }

    #[tokio::test]
    async fn migration_055_creates_direct_turn_attachment_tables() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE workflows (workflow_id INTEGER PRIMARY KEY);
             CREATE TABLE conversations (id TEXT PRIMARY KEY);
             CREATE TABLE messages (conversation_id TEXT NOT NULL, message_id TEXT NOT NULL, PRIMARY KEY (conversation_id, message_id));",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATION_055).execute(&pool).await.unwrap();

        for table in [
            "durable_turn_submitted_images",
            "durable_turn_submitted_files",
            "durable_turn_delivery_images",
            "durable_turn_delivery_files",
        ] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(exists, 1, "missing table {table}");
        }
    }

    #[tokio::test]
    async fn migration_056_advances_global_sequences_without_regression() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE workflows (workflow_id INTEGER PRIMARY KEY);
             CREATE TABLE durable_turns (turn_id INTEGER PRIMARY KEY);
             CREATE TABLE workflow_global_sequences (
                 sequence_name TEXT PRIMARY KEY,
                 next_value INTEGER NOT NULL
             );
             INSERT INTO workflows (workflow_id) VALUES (7);
             INSERT INTO durable_turns (turn_id) VALUES (11);
             INSERT INTO workflow_global_sequences (sequence_name, next_value)
             VALUES ('workflow', 3), ('direct_turn', 20);",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATION_056).execute(&pool).await.unwrap();

        let workflow: i64 = sqlx::query_scalar(
            "SELECT next_value FROM workflow_global_sequences WHERE sequence_name = 'workflow'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let direct_turn: i64 = sqlx::query_scalar(
            "SELECT next_value FROM workflow_global_sequences WHERE sequence_name = 'direct_turn'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workflow, 8);
        assert_eq!(direct_turn, 20);
    }

    #[tokio::test]
    async fn migration_051_backfills_fts_row_locators() {
        let pool = test_pool().await;
        sqlx::raw_sql(MIGRATION_018).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO message_fts
             (text, message_id, chunk_ordinal, conversation_id, message_type, created_at, content_hash)
             VALUES ('needle', 'm-1', 0, 'c-1', 'user', '2026-01-01T00:00:00Z', 'hash-1')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATION_051).execute(&pool).await.unwrap();

        let locator: (i64, String, i64, String, String, String, String) = sqlx::query_as(
            "SELECT fts_rowid, message_id, chunk_ordinal, conversation_id,
                    message_type, created_at, content_hash
             FROM message_fts_rows",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let fts_rowid: i64 = sqlx::query_scalar("SELECT rowid FROM message_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            locator,
            (
                fts_rowid,
                "m-1".into(),
                0,
                "c-1".into(),
                "user".into(),
                "2026-01-01T00:00:00Z".into(),
                "hash-1".into()
            )
        );
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

    async fn assert_workflow_foundation_tables(pool: &SqlitePool) {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'workflow_%' OR name='workflows' ORDER BY name",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        for expected in [
            "workflows",
            "workflow_transitions",
            "workflow_effects",
            "workflow_effect_dependencies",
            "workflow_barriers",
            "workflow_barrier_members",
            "workflow_attempts",
            "workflow_reclaimable_leases",
            "workflow_authoritative_observations",
            "workflow_stale_observations",
            "workflow_receipts",
            "workflow_deliveries",
            "workflow_manual_resolutions",
            "workflow_manual_resolution_choices",
            "workflow_external_acceptance_bindings",
            "workflow_schedules",
            "workflow_supported_codecs",
        ] {
            assert!(
                tables.iter().any(|table| table == expected),
                "missing {expected}: {tables:?}"
            );
        }
    }

    #[tokio::test]
    async fn migration_046_creates_workflow_foundation_tables_and_invariants() {
        let pool = test_pool().await;
        setup_workflow_only_schema(&pool).await;

        let applied = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(applied, 1);

        assert_workflow_foundation_tables(&pool).await;

        sqlx::query(
            "INSERT INTO workflows (
                workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
                external_acceptance_enabled, version, generation, status,
                snapshot_codec_family, snapshot_codec_version, snapshot_payload,
                created_at, updated_at
             ) VALUES (1, 'test', 1, 1, 1, 0, 0, 'Active', 'snapshot', 1, X'00', 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO workflow_effects (workflow_id, effect_id, declared_workflow_version, family, kind, intent_codec_family, intent_codec_version, intent_payload, generation, role, capability_kind, status) VALUES (1, 1, 0, 'f', 'k', 'intent', 1, X'01', 0, 'Required', 'ReclaimableObservation', 'Eligible')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_attempts (workflow_id, effect_id, attempt_id, ordinal, declared_workflow_version, generation, process_incarnation, status, created_at) VALUES (1, 1, 1, 0, 0, 0, 1, 'Begun', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_transitions (workflow_id, transition_id, from_version, to_version, generation, event_codec_family, event_codec_version, event_payload, committed_at) VALUES (1, 1, 0, 1, 0, 'event', 1, X'02', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_barriers (workflow_id, barrier_id, status, reducer_event_codec_family, reducer_event_codec_version, reducer_event_payload) VALUES (1, 1, 'Waiting', 'barrier', 1, X'03')")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("INSERT INTO workflow_reclaimable_leases (workflow_id, attempt_id, lease_until) VALUES (1, 1, 10)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_authoritative_observations (workflow_id, observation_id, effect_id, attempt_id, declared_workflow_version, generation, process_incarnation, observation_codec_family, observation_codec_version, observation_payload, observed_at, recorded_at) VALUES (1, 1, 1, 1, 0, 0, 1, 'obs', 1, X'04', 2, 3)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (1, 1, 1, 0, 0, 1, 1, 'Execution', 'receipt', 1, X'05')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (1, 1, 1, NULL, 'consumer', 'event', 1, 'Receipt', X'06', 1, 'Pending', 'Owed', NULL, NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_manual_resolutions (workflow_id, manual_resolution_id, workflow_version, effect_id, status, accepted_choice_ordinal, resolved_by) VALUES (1, 1, 1, 1, 'Required', NULL, NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_manual_resolution_choices (workflow_id, manual_resolution_id, ordinal, kind, payload_codec_family, payload_codec_version, payload_blob, receipt_codec_family, receipt_codec_version, receipt_blob, receipt_event_codec_family, receipt_event_codec_version, receipt_event_blob) VALUES (1, 1, 0, 'Retry', 'manual', 1, X'07', 'receipt', 1, X'08', 'event', 1, X'09')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workflow_manual_resolutions SET accepted_choice_ordinal = 0 WHERE workflow_id = 1 AND manual_resolution_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_schedules (workflow_id, schedule_id, policy, schedule_key, status, next_eligible_at, active_effect_id, due_occurrence_id, due_generation, due_at, active_occurrence_id, active_generation, active_due_at) VALUES (1, 1, 'CoalesceLatest', 'sched', 'Due', 10, NULL, 1, 0, 10, NULL, NULL, NULL)")
            .execute(&pool)
            .await
            .unwrap();

        assert!(sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (1, 2, 1, 1, 1, 1, NULL, 'Execution', 'receipt', 1, X'05')")
            .execute(&pool)
            .await
            .is_err());
        assert!(sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, barrier_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status, suppression_reason, accepted_by_transition_id) VALUES (1, 2, 1, NULL, 'consumer', 'event', 1, 'Receipt', X'06', 1, 'Pending', NULL, NULL, NULL)")
            .execute(&pool)
            .await
            .is_err());
        assert!(sqlx::query("INSERT INTO workflow_schedules (workflow_id, schedule_id, policy, schedule_key, status, next_eligible_at, active_effect_id, due_occurrence_id, due_generation, due_at, active_occurrence_id, active_generation, active_due_at) VALUES (1, 2, 'CoalesceLatest', 'sched2', 'Idle', 10, NULL, 1, 0, 10, NULL, NULL, NULL)")
            .execute(&pool)
            .await
            .is_err());
    }

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
                    "SELECT c.cm_kind, e.branch_name AS cm_branch_name,
                            e.worktree_path AS cm_worktree_path, e.base_branch AS cm_base_branch,
                            c.cm_task_id, c.cm_task_title, c.cm_next_taskmd_id_hint
                     FROM conversations c
                     LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
                     WHERE c.id = ?1",
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
                "INSERT INTO conversations (id, cwd, user_initiated, steering_queue, state_updated_at, created_at, updated_at) \
                 VALUES (?1, '/tmp', 1, ?2, '2025-01-01', '2025-01-01', '2025-01-01')",
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
                "INSERT INTO conversations (id, cwd, user_initiated, steering_queue, state_updated_at, created_at, updated_at) \
                 VALUES (?1, '/tmp', 1, ?2, '2025-01-01', '2025-01-01', '2025-01-01')",
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
    #[allow(clippy::too_many_lines)]
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
        for (sequence_id, (id, mt, content)) in [
            ("m-user", "user", user_content),
            ("m-skill", "skill", skill_content),
            ("m-plain", "user", plain_content),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query(
                "INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at) \
                 VALUES (?1, 'c', ?2, ?3, ?4, '2025-01-01')",
            )
            .bind(id)
            .bind(i64::try_from(sequence_id + 1).unwrap())
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
            "SELECT c.cm_kind, e.worktree_path AS cm_worktree_path, c.state
             FROM conversations c
             LEFT JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
             WHERE c.id = 'c2'",
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
    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    async fn migration_052_normalizes_authority_environment_and_preserves_wakes() {
        let pool = test_pool().await;
        setup_legacy_schema_through(&pool, 51).await;
        sqlx::query("CREATE TABLE _migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool)
            .await
            .unwrap();
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 51)
        {
            sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
                .bind(migration.version)
                .bind(migration.name)
                .execute(&pool)
                .await
                .unwrap();
        }

        sqlx::query(
            "INSERT INTO conversations
             (id, slug, cwd, user_initiated, state, state_updated_at, created_at, updated_at,
              cm_kind, cm_worktree_path, cm_branch_name, cm_base_branch)
             VALUES ('migration-wake-conv', 'migration-wake-conv', '/tmp/migration-wake', 1,
                     '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-01',
                     'work', '/tmp/migration-wake', 'topic', 'main')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (scope_type, scope_value, created_at, updated_at)
             VALUES ('Worktree', '/tmp/migration-wake', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations
             (id, slug, cwd, parent_conversation_id, user_initiated, state, state_updated_at,
              created_at, updated_at, cm_kind, cm_worktree_path, cm_branch_name, cm_base_branch,
              continued_in_conv_id)
             VALUES
              ('work-root', 'work-root', '/wt/work', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'work', '/wt/work', NULL, NULL, NULL),
              ('branch-root', 'branch-root', '/wt/branch', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'branch', '/wt/branch', 'feature', NULL, NULL),
              ('explore-root', 'explore-root', '/wt/explore', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'explore', '/wt/explore', NULL, NULL, NULL),
              ('direct-cwd', 'direct-cwd', '/repo/direct', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'direct', NULL, NULL, NULL, NULL),
              ('direct-none', 'direct-none', '', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'direct', NULL, NULL, NULL, NULL),
              ('inherit-root', 'inherit-root', '/root', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'direct', NULL, NULL, NULL, 'inherit-successor'),
              ('inherit-child', 'inherit-child', '/wrong-child', 'inherit-root', 0, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-03', 'work', '/wrong-child', 'wrong', 'wrong-base', NULL),
              ('inherit-grandchild', 'inherit-grandchild', '/wrong-grandchild', 'inherit-child', 0, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-04', 'explore', '/wrong-grandchild', NULL, NULL, NULL),
              ('inherit-successor', 'inherit-successor', '/wrong-successor', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-05', 'branch', '/wrong-successor', 'wrong', NULL, NULL),
              ('successor-child', 'successor-child', '/wrong-successor-child', 'inherit-successor', 0, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-06', 'work', '/wrong-successor-child', 'wrong', NULL, NULL),
              ('direct-chain-root', 'direct-chain-root', '/direct/root', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'direct', NULL, NULL, NULL, 'direct-chain-successor'),
              ('direct-chain-successor', 'direct-chain-successor', '/direct/successor', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-03', 'direct', NULL, NULL, NULL, NULL),
              ('direct-successor-child', 'direct-successor-child', '/direct/successor/subdir', 'direct-chain-successor', 0, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-04', 'direct', NULL, NULL, NULL, NULL),
              ('coordinator-root', 'coordinator-root', '', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-02', 'direct', NULL, NULL, NULL, 'coordinator-leaf'),
              ('coordinator-leaf', 'coordinator-leaf', '', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-03', 'direct', NULL, NULL, NULL, NULL),
              ('handoff-explore', 'handoff-explore', '/handoff/worktree', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-07', 'explore', '/handoff/worktree', NULL, NULL, 'handoff-work'),
              ('handoff-work', 'handoff-work', '/handoff/worktree', NULL, 1, '{\"type\":\"idle\"}', '2025-01-01', '2025-01-01', '2025-01-08', 'work', '/handoff/worktree', 'task-handoff', 'main', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO coordinator (singleton, conversation_id)
             VALUES (1, 'coordinator-leaf')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at) VALUES ('migration-message', 'branch-root', 1, 'user', 'preserved', '2025-01-01')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflows (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled, external_acceptance_enabled, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at) VALUES (52, 'wake', 1, 1, 0, 1, 0, 'Active', 'wake', 1, X'00', 1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_effects (workflow_id, effect_id, declared_workflow_version, family, kind, intent_codec_family, intent_codec_version, intent_payload, generation, role, capability_kind, status) VALUES (52, 1, 0, 'wake', 'observe', 'wake', 1, X'01', 0, 'Required', 'ReclaimableObservation', 'Receipted')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_attempts (workflow_id, effect_id, attempt_id, ordinal, declared_workflow_version, generation, process_incarnation, status, created_at) VALUES (52, 1, 1, 0, 0, 0, 1, 'ReceiptAccepted', 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_receipts (workflow_id, receipt_id, effect_id, generation, declared_workflow_version, process_incarnation, attempt_id, origin, receipt_codec_family, receipt_codec_version, receipt_payload) VALUES (52, 1, 1, 0, 0, 1, 1, 'Execution', 'wake', 1, X'02')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflow_deliveries (workflow_id, delivery_id, effect_id, consumer_kind, event_codec_family, event_codec_version, payload_kind, payload_blob, requires_runtime_acceptance, status, runtime_acceptance_status) VALUES (52, 1, 1, 'conversation', 'wake', 1, 'Receipt', X'03', 1, 'Pending', 'Owed')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wake_bindings (workflow_id, conversation_id, contract_id, profile_kind, profile_version, scope_kind, scope_stable_key, resource_kind, bash_handle_id, registering_tool_use_id, expires_at, resolved_at, prepared_fingerprint, observe_effect_id, created_at) VALUES (52, 'branch-root', 'contract', 'wake', 1, 'Worktree', 'worktree:/tmp/migration-wake', 'Bash', 'b-52', 'tool-52', 100, NULL, 'fingerprint', 1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wake_terminal_receipts (workflow_id, receipt_id, delivery_id, conversation_id, contract_id, resource_kind, terminal_kind, resolved_at, bash_handle_id, bash_status, occurred_at, exit_code, duration_ms) VALUES (52, 1, 1, 'branch-root', 'contract', 'Bash', 'Fired', 2, 'b-52', 'Exited', 2, 0, 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wake_terminal_receipt_tails (workflow_id, receipt_id, ordinal, line) VALUES (52, 1, 0, 'preserved tail')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wake_delivery_messages (workflow_id, delivery_id, conversation_id, message_id, registering_tool_use_id, terminal_kind, auto_resume, created_at) VALUES (52, 1, 'branch-root', 'migration-message', 'tool-52', 'Fired', 1, 2)")
            .execute(&pool)
            .await
            .unwrap();

        let expected_pending = u32::try_from(
            MIGRATIONS
                .iter()
                .filter(|migration| migration.version > 51)
                .count(),
        )
        .unwrap();
        assert_eq!(
            run_pending_migrations(&pool).await.unwrap(),
            expected_pending
        );

        let columns: Vec<String> = sqlx::query("PRAGMA table_info(work_scopes)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();
        assert!(!columns.iter().any(|c| c == "scope_type"));
        assert!(!columns.iter().any(|c| c == "scope_value"));

        let bad_ids: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scopes WHERE id LIKE 'legacy-%' OR id LIKE '%:%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(bad_ids, 0);

        let invalid_authority: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scopes WHERE authority_kind NOT IN ('restricted_explore', 'work')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(invalid_authority, 0);

        let invalid_role: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations WHERE runtime_role NOT IN ('user', 'sub_agent', 'coordinator')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(invalid_role, 0);

        let invalid_scope_nullability: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations WHERE (runtime_role = 'coordinator') != (work_scope_id IS NULL)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(invalid_scope_nullability, 0);

        let direct_scope_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT work_scope_id) FROM conversations
             WHERE id IN ('direct-cwd', 'direct-none', 'inherit-root')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(direct_scope_count, 3);

        let direct_continuation_scopes: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, work_scope_id FROM conversations
             WHERE id IN ('direct-chain-root', 'direct-chain-successor', 'direct-successor-child')
             ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let direct_root_scope = &direct_continuation_scopes[0].1;
        let direct_successor_scope = &direct_continuation_scopes[1].1;
        let direct_child_scope = &direct_continuation_scopes[2].1;
        assert_ne!(direct_root_scope, direct_successor_scope);
        assert_eq!(direct_child_scope, direct_successor_scope);

        let coordinator_chain: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT runtime_role, work_scope_id FROM conversations
             WHERE id IN ('coordinator-root', 'coordinator-leaf') ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            coordinator_chain,
            vec![
                ("coordinator".to_string(), None),
                ("coordinator".to_string(), None)
            ]
        );

        let handoff_environment: (String, String, String) = sqlx::query_as(
            "SELECT environment.branch_name, environment.base_branch, environment.worktree_path
             FROM conversations successor
             JOIN work_scope_environments environment
               ON environment.work_scope_id = successor.work_scope_id
             WHERE successor.id = 'handoff-work'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            handoff_environment,
            (
                "task-handoff".to_string(),
                "main".to_string(),
                "/handoff/worktree".to_string(),
            )
        );

        let invalid_lifecycle: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scopes
             WHERE lifecycle <> 'active' OR retired_at IS NOT NULL OR retired_reason IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(invalid_lifecycle, 0);

        let conversation_columns: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("name"))
            .collect();
        assert!(!conversation_columns.iter().any(|column| column == "cwd"));
        assert!(!conversation_columns
            .iter()
            .any(|column| column == "coordinator_cwd"));

        let environment_columns: Vec<String> = sqlx::query("PRAGMA table_info(work_scopes)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("name"))
            .collect();
        for column in [
            "environment_kind",
            "cwd",
            "worktree_path",
            "branch_name",
            "base_branch",
        ] {
            assert!(environment_columns.iter().any(|actual| actual == column));
        }

        let invalid_environment = sqlx::query(
            "UPDATE work_scopes
             SET environment_kind = 'allocated_worktree', cwd = NULL, worktree_path = NULL
             LIMIT 1",
        )
        .execute(&pool)
        .await;
        assert!(invalid_environment.is_err());

        let invalid_role_scope = sqlx::query(
            "UPDATE conversations SET runtime_role = 'coordinator'
             WHERE id = 'work-root'",
        )
        .execute(&pool)
        .await;
        assert!(invalid_role_scope.is_err());

        let invalid_fk = sqlx::query(
            "UPDATE conversations SET work_scope_id = 'missing-scope'
             WHERE id = 'work-root'",
        )
        .execute(&pool)
        .await;
        assert!(invalid_fk.is_err());

        let generated_environments: Vec<(String, String, Option<String>, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT c.id, e.environment_kind, e.cwd, e.worktree_path, e.branch_name, e.base_branch
                 FROM conversations c
                 JOIN work_scope_environments e ON e.work_scope_id = c.work_scope_id
                 WHERE c.id IN ('work-root', 'branch-root', 'explore-root', 'direct-cwd', 'direct-none')
                 ORDER BY c.id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            generated_environments,
            vec![
                (
                    "branch-root".into(),
                    "allocated_worktree".into(),
                    Some("/wt/branch".into()),
                    Some("/wt/branch".into()),
                    Some("feature".into()),
                    None
                ),
                (
                    "direct-cwd".into(),
                    "unowned_cwd".into(),
                    Some("/repo/direct".into()),
                    None,
                    None,
                    None
                ),
                ("direct-none".into(), "none".into(), None, None, None, None),
                (
                    "explore-root".into(),
                    "allocated_worktree".into(),
                    Some("/wt/explore".into()),
                    Some("/wt/explore".into()),
                    None,
                    None
                ),
                (
                    "work-root".into(),
                    "allocated_worktree".into(),
                    Some("/wt/work".into()),
                    Some("/wt/work".into()),
                    None,
                    None
                ),
            ]
        );
        let scope_environment_counts: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM work_scopes),
                    (SELECT COUNT(*) FROM work_scope_environments)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(scope_environment_counts.0, scope_environment_counts.1);
        let inherited_scopes: Vec<String> = sqlx::query_scalar(
            "SELECT work_scope_id FROM conversations
             WHERE id IN ('inherit-root', 'inherit-child', 'inherit-grandchild', 'inherit-successor', 'successor-child')
             ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(inherited_scopes.len(), 5);
        assert!(inherited_scopes
            .iter()
            .all(|scope| scope == &inherited_scopes[0]));
        let fabricated_unknowns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scope_environments
             WHERE branch_name = 'unknown' OR base_branch = 'unknown'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fabricated_unknowns, 0);

        let binding_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wake_bindings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(binding_count, 1);
        let wake_ownership: (String, String, String) = sqlx::query_as(
            "SELECT binding.work_scope_id, owner.work_scope_id, binding.conversation_id
             FROM wake_bindings binding
             JOIN conversations owner ON owner.id = binding.conversation_id
             WHERE binding.workflow_id = 52",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let legacy_worktree_scope: String = sqlx::query_scalar(
            "SELECT c.work_scope_id FROM conversations c WHERE c.id = 'migration-wake-conv'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(wake_ownership.0, legacy_worktree_scope);
        assert_ne!(wake_ownership.0, wake_ownership.1);
        assert_eq!(wake_ownership.2, "branch-root");
        let migrated_fingerprint: (String, i64) = sqlx::query_as(
            "SELECT prepared_fingerprint, fingerprint_needs_scope_upgrade
             FROM wake_bindings WHERE workflow_id = 52",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(migrated_fingerprint, ("fingerprint".to_string(), 1));
        let replay_intent = phoenix_workflow::wake_profile::WakeRegistrationIntent {
            contract_id: "contract".to_string(),
            conversation_id: "branch-root".to_string(),
            root_conversation_id: "branch-root".to_string(),
            registration_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(
                wake_ownership.0.clone(),
            ),
            resource: phoenix_workflow::wake_profile::WakeResourceIdentity::Bash(
                phoenix_workflow::wake_profile::BashResourceIdentity {
                    work_scope: phoenix_workflow::wake_profile::WorkScopeIdentity(
                        wake_ownership.0.clone(),
                    ),
                    handle_id: "b-52".to_string(),
                },
            ),
            registering_tool_use_id: "tool-52".to_string(),
            registered_at: phoenix_workflow::Timestamp(3),
            expires_at: phoenix_workflow::Timestamp(100),
        };
        let replay = crate::workflow::wake::WakeRepository::new(pool.clone())
            .register(
                &replay_intent,
                "opaque-fingerprint",
                phoenix_workflow::Timestamp(3),
            )
            .await
            .unwrap();
        assert!(matches!(
            replay,
            crate::workflow::wake::WakeRegistrationOutcome::Replayed {
                workflow_id: phoenix_workflow::WorkflowId(52),
                ..
            }
        ));
        let upgraded_fingerprint: (String, i64) = sqlx::query_as(
            "SELECT prepared_fingerprint, fingerprint_needs_scope_upgrade
             FROM wake_bindings WHERE workflow_id = 52",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(upgraded_fingerprint, ("opaque-fingerprint".to_string(), 0));
        let preserved_tail: String = sqlx::query_scalar(
            "SELECT line FROM wake_terminal_receipt_tails WHERE workflow_id = 52 AND receipt_id = 1 AND ordinal = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preserved_tail, "preserved tail");
        let preserved_message: (String, String, i64) = sqlx::query_as(
            "SELECT message_id, registering_tool_use_id, auto_resume
             FROM wake_delivery_messages WHERE workflow_id = 52 AND delivery_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            preserved_message,
            ("migration-message".into(), "tool-52".into(), 1)
        );

        for (table, foreign_key_sql) in [
            (
                "wake_terminal_receipts",
                "PRAGMA foreign_key_list(wake_terminal_receipts)",
            ),
            (
                "wake_delivery_messages",
                "PRAGMA foreign_key_list(wake_delivery_messages)",
            ),
        ] {
            let referenced_tables: Vec<String> = sqlx::query(foreign_key_sql)
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("table"))
                .collect();
            assert!(
                referenced_tables.iter().any(|name| name == "wake_bindings"),
                "{table} must reference reconstructed wake_bindings: {referenced_tables:?}"
            );
            assert!(
                referenced_tables
                    .iter()
                    .all(|name| name != "wake_bindings_old"),
                "{table} retains rewritten FK to wake_bindings_old: {referenced_tables:?}"
            );
        }

        let tail_references: Vec<String> =
            sqlx::query("PRAGMA foreign_key_list(wake_terminal_receipt_tails)")
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("table"))
                .collect();
        assert!(tail_references
            .iter()
            .any(|name| name == "wake_terminal_receipts"));
        assert!(tail_references
            .iter()
            .all(|name| name != "wake_terminal_receipts_old"));

        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master
             WHERE type = 'index' AND name IN (
                 'wake_terminal_receipts_by_conversation',
                 'wake_terminal_receipts_by_workflow_delivery',
                 'wake_delivery_messages_by_conversation'
             )",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(indexes.len(), 3, "dependent wake indexes must survive");
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
