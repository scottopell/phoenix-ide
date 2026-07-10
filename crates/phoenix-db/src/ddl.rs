//! Raw SQL applied at startup: the base schema plus the legacy idempotent
//! "migration" statements that predate the versioned [`crate::migrations`]
//! table. [`Database::run_migrations`] runs these before the sequential
//! migrations.
//!
//! These are pure persistence DDL, scoped to this crate — no other crate
//! references them.

/// SQL schema for initialization.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    slug TEXT UNIQUE,
    -- cwd is immutable post-creation. The only writers are the
    -- recovery/teardown fallbacks via update_conversation_cwd_recovery_only
    -- (task 13012); nothing else may mutate it.
    cwd TEXT NOT NULL,
    parent_conversation_id TEXT,
    user_initiated BOOLEAN NOT NULL,
    state TEXT NOT NULL DEFAULT '{"type":"idle"}',
    state_updated_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived BOOLEAN NOT NULL DEFAULT 0,
    model TEXT,
    -- llm_language column added by migration 010; not in SCHEMA so that
    -- migration 010's ALTER TABLE doesn't collide on a fresh DB.

    -- conv_mode is the legacy ConvMode JSON blob. It is normalized into the
    -- cm_* columns by migration 028 and DROPped by migration 029; it lives in
    -- the base schema (rather than the idempotent legacy ALTER) so that on a
    -- fresh DB the migrations 001/002/007/021/028 that read it still resolve
    -- during replay, and migration 029's DROP is not resurrected on next boot.
    conv_mode TEXT NOT NULL DEFAULT '{"mode":"Explore"}',

    FOREIGN KEY (parent_conversation_id)
        REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_conversations_slug ON conversations(slug);
CREATE INDEX IF NOT EXISTS idx_conversations_parent ON conversations(parent_conversation_id);
CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    message_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    sequence_id INTEGER NOT NULL,
    message_type TEXT NOT NULL,
    content TEXT NOT NULL,
    display_data TEXT,
    usage_data TEXT,
    created_at TEXT NOT NULL,

    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id, sequence_id);

-- Per-message attachments, normalized out of the messages.content blob.
-- Child collections never belong inside a JSON-TEXT aggregate: presence is row
-- existence, shape is NOT NULL columns, order is the explicit ordinal.
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

-- Pending steering messages, normalized out of the conversations.steering_queue
-- blob. A steering entry is a queued user message; its own attachments are
-- grandchild collections (never an earned blob). skill_* is an all-or-nothing
-- trio enforced by CHECK. `ordinal` is the FIFO position within a conversation.
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

CREATE TABLE IF NOT EXISTS conversation_creation_job_images (
    job_id TEXT NOT NULL REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (job_id, ordinal)
);

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

CREATE TABLE IF NOT EXISTS notification_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// Migration SQL to convert old state format to typed JSON
/// Runs at startup to ensure all state values are valid JSON
pub(crate) const MIGRATION_TYPED_STATE: &str = r#"
-- Migrate old string-based state to JSON format
-- Only runs if there are non-JSON state values

-- Convert 'idle' string to JSON
UPDATE conversations SET state = '{"type":"idle"}' WHERE state = 'idle';

-- Convert all other non-JSON states to idle (they would be reset on startup anyway)
-- This handles: awaiting_llm, llm_requesting, tool_executing, etc.
UPDATE conversations SET state = '{"type":"idle"}'
WHERE state NOT LIKE '{%}';
"#;

/// Migration: replace `"unknown"` `error_kind` with `"server_error"` in JSON state.
/// The `Unknown` variant was removed from `ErrorKind`; old rows need updating
/// so serde can deserialize them.
pub(crate) const MIGRATION_REMOVE_UNKNOWN_ERROR_KIND: &str = r#"
UPDATE conversations
SET state = REPLACE(state, '"error_kind":"unknown"', '"error_kind":"server_error"')
WHERE state LIKE '%"error_kind":"unknown"%';
"#;

/// Migration SQL to add model column
#[allow(dead_code)] // Will be used in future
pub(crate) const MIGRATION_ADD_MODEL: &str = r"
-- This is a no-op if the column already exists
-- SQLite will return an error which we'll ignore
";

/// Migration SQL to create projects table
pub(crate) const MIGRATION_CREATE_PROJECTS: &str = r"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    canonical_path TEXT UNIQUE NOT NULL,
    main_ref TEXT NOT NULL DEFAULT 'main',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_projects_path ON projects(canonical_path);
";

/// Migration SQL to create `mcp_disabled_servers` table.
pub(crate) const MIGRATION_CREATE_MCP_DISABLED_SERVERS: &str = r"
CREATE TABLE IF NOT EXISTS mcp_disabled_servers (
    server_name TEXT PRIMARY KEY
);
";

/// Migration SQL to create `share_tokens` table (REQ-AUTH-008).
pub(crate) const MIGRATION_CREATE_SHARE_TOKENS: &str = r"
CREATE TABLE IF NOT EXISTS share_tokens (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_share_tokens_token ON share_tokens(token);
CREATE INDEX IF NOT EXISTS idx_share_tokens_conversation ON share_tokens(conversation_id);
";

/// Migration SQL to add `local_id` column for idempotent message sends
/// Migration to rename `messages.id` to `messages.message_id`
/// `SQLite` 3.25+ supports ALTER TABLE RENAME COLUMN
/// For older versions or if column already renamed, this is a no-op
pub(crate) const MIGRATION_RENAME_MESSAGE_ID: &str = r"
-- Rename id to message_id for searchability
-- This will fail silently if already renamed or SQLite is too old
ALTER TABLE messages RENAME COLUMN id TO message_id;
";
