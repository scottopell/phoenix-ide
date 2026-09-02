//! Sequential database migrations.
//!
//! Each migration runs exactly once, tracked by the `_migrations` table.
//! Migrations run at startup before any conversation is loaded.

use std::collections::HashSet;

use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use phoenix_core::work_scope::WorkScopeId;

use super::{DbError, DbResult, ProjectSeedId};

mod retire_commission_review;

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
    Migration {
        version: 58,
        name: "add_model_effort",
        sql: MIGRATION_058,
    },
    Migration {
        version: 59,
        name: "add_conversation_state_kind_discriminator",
        sql: MIGRATION_059,
    },
    Migration {
        version: 60,
        name: "enforce_conversation_state_kind_consistency",
        sql: MIGRATION_060,
    },
    Migration {
        version: 61,
        name: "project_conversation_work_scope_attachments",
        sql: MIGRATION_061,
    },
    Migration {
        version: 62,
        name: "create_steering_acceptance_receipts",
        sql: MIGRATION_062,
    },
    Migration {
        version: 63,
        name: "add_conversation_service_tier",
        sql: MIGRATION_063,
    },
    Migration {
        version: 64,
        name: "create_close_retirement_tables",
        sql: MIGRATION_064,
    },
    Migration {
        version: 65,
        name: "create_git_repository_shadow_tables",
        sql: MIGRATION_065,
    },
    Migration {
        version: 66,
        name: "create_direct_turn_terminal_obligations",
        sql: MIGRATION_066,
    },
    Migration {
        version: 67,
        name: "create_startup_parent_actions",
        sql: MIGRATION_067,
    },
    Migration {
        version: 68,
        name: "retire_commission_review_approvals",
        sql: MIGRATION_068,
    },
    Migration {
        version: 69,
        name: "settle_retired_tool_recovery",
        sql: MIGRATION_069,
    },
    Migration {
        version: 70,
        name: "persist_product_conversation_identity",
        sql: MIGRATION_070,
    },
    Migration {
        version: 71,
        name: "correct_product_conversation_lifecycle_seed",
        sql: MIGRATION_071,
    },
    Migration {
        version: 72,
        name: "retain_dormant_product_lifecycle_projection",
        sql: MIGRATION_072,
    },
    Migration {
        version: 73,
        name: "forbid_recursive_subordinate_parents",
        sql: MIGRATION_073,
    },
    Migration {
        version: 74,
        name: "persist_completed_continuation_handoffs",
        sql: MIGRATION_074,
    },
    Migration {
        version: 75,
        name: "capture_close_direct_turn_settlements",
        sql: MIGRATION_075,
    },
    Migration {
        version: 76,
        name: "add_work_scope_close_retirement_resource_kind",
        sql: MIGRATION_076,
    },
    Migration {
        version: 77,
        name: "record_close_retirement_resource_dispatches",
        sql: MIGRATION_077,
    },
    Migration {
        version: 78,
        name: "persist_runtime_resource_instances",
        sql: MIGRATION_078,
    },
    Migration {
        version: 79,
        name: "bind_close_resources_to_runtime_instances",
        sql: MIGRATION_079,
    },
    Migration {
        version: 80,
        name: "simplify_close_runtime_authority_and_scope_owners",
        sql: MIGRATION_080,
    },
    Migration {
        version: 81,
        name: "repair_scope_owner_foreign_keys_and_absence_proof",
        sql: MIGRATION_081,
    },
    Migration {
        version: 82,
        name: "return_changed_close_worktree_to_reinspection",
        sql: MIGRATION_082,
    },
    Migration {
        version: 83,
        name: "retry_preinspection_close_repair_via_reinspection",
        sql: MIGRATION_083,
    },
    Migration {
        version: 84,
        name: "persist_close_worktree_cleanup_plans",
        sql: MIGRATION_084,
    },
    Migration {
        version: 85,
        name: "preserve_close_reinspection_authority",
        sql: MIGRATION_085,
    },
    Migration {
        version: 86,
        name: "rotate_worktree_less_close_retry_inventory",
        sql: MIGRATION_086,
    },
    Migration {
        version: 87,
        name: "require_worktree_cleanup_plan_for_adopted_absence",
        sql: MIGRATION_087,
    },
    Migration {
        version: 88,
        name: "bind_worktree_cleanup_plan_to_admin_incarnation",
        sql: MIGRATION_088,
    },
    Migration {
        version: 89,
        name: "require_residual_for_all_close_repair_transitions",
        sql: MIGRATION_089,
    },
    Migration {
        version: 90,
        name: "bind_close_worktree_final_tombstone",
        sql: MIGRATION_090,
    },
    Migration {
        version: 91,
        name: "persist_product_creation_jobs",
        sql: MIGRATION_091,
    },
    Migration {
        version: 92,
        name: "persist_conversation_creation_exact_checkout_oid",
        sql: MIGRATION_092,
    },
    Migration {
        version: 93,
        name: "persist_product_creation_resource_ownership",
        sql: MIGRATION_093,
    },
    Migration {
        version: 94,
        name: "enforce_monotonic_message_sequences",
        sql: MIGRATION_094,
    },
    Migration {
        version: 95,
        name: "reconcile_product_lifecycle_cutover",
        sql: MIGRATION_095,
    },
];

pub(crate) fn compiled_migration_ledger() -> Vec<(i64, &'static str)> {
    MIGRATIONS
        .iter()
        .map(|migration| (i64::from(migration.version), migration.name))
        .collect()
}

/// A length-delimited digest makes the exact compiled migration source part of
/// readiness identity: adjacent values cannot be confused with a different
/// version/name/body partition.
pub(crate) fn compiled_migration_digest() -> String {
    migration_digest_from_parts(MIGRATIONS.iter().map(|migration| {
        (
            migration.version,
            migration.name.as_bytes(),
            migration.sql.as_bytes(),
        )
    }))
}

fn migration_digest_from_parts<'a>(
    parts: impl IntoIterator<Item = (u32, &'a [u8], &'a [u8])>,
) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    let mut digest = Sha256::new();
    for (version, name, sql) in parts {
        digest.update(b"phoenix-db-migration-v1\\0");
        digest.update(version.to_be_bytes());
        digest.update(
            u64::try_from(name.len())
                .expect("name length fits u64")
                .to_be_bytes(),
        );
        digest.update(name);
        digest.update(
            u64::try_from(sql.len())
                .expect("SQL length fits u64")
                .to_be_bytes(),
        );
        digest.update(sql);
    }
    digest
        .finalize()
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("writing to String cannot fail");
            hex
        })
}

pub(crate) fn r1_expected_table_definitions() -> std::collections::BTreeMap<&'static str, String> {
    const TABLES: [&str; 4] = [
        "git_repositories",
        "git_repository_locator_observations",
        "git_repository_default_branch_observations",
        "work_scope_git_repositories",
    ];

    MIGRATION_065
        .split(';')
        .filter_map(|statement| {
            let canonical = normalize_sql(statement);
            let table_name_match = canonical.to_ascii_lowercase();
            TABLES.into_iter().find_map(|table| {
                table_name_match
                    .starts_with(&format!("create table {table} "))
                    .then(|| (table, canonical.clone()))
            })
        })
        .collect()
}

pub(crate) fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

const MIGRATION_070: &str = r"
CREATE TABLE product_conversations (
    id TEXT PRIMARY KEY CHECK (
        typeof(id) = 'text'
        AND trim(id, char(9) || char(10) || char(11) || char(12) || char(13) || ' ') <> ''
    ),
    kind TEXT NOT NULL CHECK (kind IN ('ordinary', 'coordinator')),
    ordinary_lifecycle TEXT CHECK (
        (kind = 'ordinary' AND ordinary_lifecycle IS NOT NULL
         AND ordinary_lifecycle IN ('open', 'history'))
        OR (kind = 'coordinator' AND ordinary_lifecycle IS NULL)
    )
);

CREATE TRIGGER product_conversations_kind_is_immutable
BEFORE UPDATE OF kind ON product_conversations
FOR EACH ROW WHEN OLD.kind <> NEW.kind
BEGIN
    SELECT RAISE(ABORT, 'ProductConversation kind is immutable');
END;

ALTER TABLE conversations ADD COLUMN product_conversation_id TEXT REFERENCES product_conversations(id) ON DELETE CASCADE;

CREATE TEMP TABLE migration_070_orphan_subordinates (
    conversation_id TEXT PRIMARY KEY,
    source_parent_id TEXT,
    group_key TEXT NOT NULL,
    tombstone_id TEXT NOT NULL,
    tombstone_scope_id TEXT NOT NULL
);
INSERT INTO migration_070_orphan_subordinates
SELECT orphan.id,
       orphan.parent_conversation_id,
       COALESCE(orphan.parent_conversation_id, orphan.id),
       'legacy-orphan-parent-' || hex(CAST(COALESCE(orphan.parent_conversation_id, orphan.id) AS BLOB)),
       'legacy-orphan-scope-' || hex(CAST(COALESCE(orphan.parent_conversation_id, orphan.id) AS BLOB))
FROM conversations orphan
WHERE orphan.runtime_role = 'sub_agent'
  AND (
      orphan.parent_conversation_id IS NULL
      OR NOT EXISTS (
          SELECT 1 FROM conversations parent
          WHERE parent.id = orphan.parent_conversation_id
      )
  );

CREATE TEMP TABLE migration_070_orphan_tombstone_collision_guard (
    collision_count INTEGER NOT NULL CHECK (collision_count = 0)
);
INSERT INTO migration_070_orphan_tombstone_collision_guard
SELECT
    (SELECT COUNT(*)
     FROM migration_070_orphan_subordinates orphan
     JOIN conversations existing ON existing.id = orphan.tombstone_id)
  + (SELECT COUNT(*)
     FROM migration_070_orphan_subordinates orphan
     JOIN work_scopes existing ON existing.id = orphan.tombstone_scope_id);
DROP TABLE migration_070_orphan_tombstone_collision_guard;

INSERT INTO work_scopes (
    id, authority_kind, lifecycle, retired_at, retired_reason, created_at, updated_at,
    environment_kind, cwd, worktree_path, branch_name, base_branch
)
SELECT DISTINCT tombstone_scope_id, 'restricted_explore', 'retired',
       '1970-01-01T00:00:00Z', 'legacy orphan subordinate tombstone',
       '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z',
       'none', NULL, NULL, NULL, NULL
FROM migration_070_orphan_subordinates;

INSERT INTO conversations (
    id, title, user_initiated, state, state_kind, state_updated_at,
    created_at, updated_at, archived, runtime_role, work_scope_id, cm_kind
)
SELECT tombstone_id, 'Retired legacy subordinate parent', 0,
       json_object('type', 'terminal'), 'terminal', '1970-01-01T00:00:00Z',
       '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z', 1, 'user',
       tombstone_scope_id, 'explore'
FROM migration_070_orphan_subordinates
GROUP BY group_key, tombstone_id, tombstone_scope_id;

CREATE TABLE legacy_orphan_subordinate_tombstones (
    root_conversation_id TEXT PRIMARY KEY
        REFERENCES conversations(id) ON DELETE CASCADE,
    source_parent_id TEXT,
    retired_at_us INTEGER NOT NULL DEFAULT 0
        CHECK (typeof(retired_at_us) = 'integer' AND retired_at_us >= 0)
);
INSERT INTO legacy_orphan_subordinate_tombstones (
    root_conversation_id, source_parent_id, retired_at_us
)
SELECT tombstone_id, MAX(source_parent_id), 0
FROM migration_070_orphan_subordinates
GROUP BY tombstone_id;

UPDATE conversations
SET parent_conversation_id = (
        SELECT orphan.tombstone_id
        FROM migration_070_orphan_subordinates orphan
        WHERE orphan.conversation_id = conversations.id
    ),
    continued_in_conv_id = NULL,
    work_scope_id = COALESCE(
        work_scope_id,
        (SELECT orphan.tombstone_scope_id
         FROM migration_070_orphan_subordinates orphan
         WHERE orphan.conversation_id = conversations.id)
    )
WHERE id IN (SELECT conversation_id FROM migration_070_orphan_subordinates);

UPDATE conversations
SET continued_in_conv_id = NULL
WHERE runtime_role = 'sub_agent'
  AND continued_in_conv_id IS NOT NULL;

DROP TABLE migration_070_orphan_subordinates;

CREATE TEMP TABLE migration_070_continuation_fork_guard (
    ambiguous_successor_count INTEGER NOT NULL CHECK (ambiguous_successor_count = 0)
);
INSERT INTO migration_070_continuation_fork_guard
SELECT COUNT(*)
FROM (
    SELECT continued_in_conv_id
    FROM conversations
    WHERE continued_in_conv_id IS NOT NULL
    GROUP BY continued_in_conv_id
    HAVING COUNT(*) > 1
);
DROP TABLE migration_070_continuation_fork_guard;

CREATE TEMP TABLE migration_070_continuation_cycle_guard (
    cyclic_member_count INTEGER NOT NULL CHECK (cyclic_member_count = 0)
);
INSERT INTO migration_070_continuation_cycle_guard
WITH RECURSIVE reachable(origin_id, conversation_id) AS (
    SELECT id, continued_in_conv_id
    FROM conversations
    WHERE continued_in_conv_id IS NOT NULL
    UNION
    SELECT reachable.origin_id, current.continued_in_conv_id
    FROM reachable
    JOIN conversations current ON current.id = reachable.conversation_id
    WHERE current.continued_in_conv_id IS NOT NULL
)
SELECT COUNT(*) FROM reachable WHERE origin_id = conversation_id;
DROP TABLE migration_070_continuation_cycle_guard;

CREATE TEMP TABLE migration_070_membership (
    conversation_id TEXT PRIMARY KEY,
    product_conversation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('ordinary', 'coordinator'))
);

WITH RECURSIVE roots(conversation_id, product_conversation_id, kind) AS (
    SELECT root.id, root.id,
           CASE root.runtime_role WHEN 'coordinator' THEN 'coordinator' ELSE 'ordinary' END
    FROM conversations root
    WHERE root.parent_conversation_id IS NULL
      AND root.runtime_role IN ('user', 'coordinator')
      AND NOT EXISTS (
          SELECT 1 FROM conversations predecessor
          WHERE predecessor.continued_in_conv_id = root.id
      )
), members(conversation_id, product_conversation_id, kind) AS (
    SELECT conversation_id, product_conversation_id, kind FROM roots
    UNION
    SELECT successor.id, members.product_conversation_id, members.kind
    FROM members
    JOIN conversations current ON current.id = members.conversation_id
    JOIN conversations successor ON successor.id = current.continued_in_conv_id
    WHERE successor.parent_conversation_id IS NULL
      AND successor.runtime_role = current.runtime_role
    UNION
    SELECT participant.id, members.product_conversation_id, members.kind
    FROM members
    JOIN conversations participant ON participant.parent_conversation_id = members.conversation_id
    WHERE participant.runtime_role = 'sub_agent'
)
INSERT INTO migration_070_membership
SELECT conversation_id, product_conversation_id, kind FROM members;

CREATE TEMP TABLE migration_070_membership_guard (
    unresolved_count INTEGER NOT NULL CHECK (unresolved_count = 0)
);
INSERT INTO migration_070_membership_guard
SELECT COUNT(*) FROM conversations c
WHERE NOT EXISTS (
    SELECT 1 FROM migration_070_membership m WHERE m.conversation_id = c.id
);
DROP TABLE migration_070_membership_guard;

INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
SELECT DISTINCT m.product_conversation_id, m.kind,
       CASE m.kind
           WHEN 'ordinary' THEN CASE WHEN EXISTS (
               SELECT 1 FROM migration_070_membership current
               JOIN conversations latest ON latest.id = current.conversation_id
               WHERE current.product_conversation_id = m.product_conversation_id
                 AND latest.runtime_role = 'user'
                 AND latest.parent_conversation_id IS NULL
                 AND latest.continued_in_conv_id IS NULL
                 AND latest.archived = 0
           ) THEN 'open' ELSE 'history' END
       END
FROM migration_070_membership m
JOIN conversations root ON root.id = m.product_conversation_id;

UPDATE conversations
SET product_conversation_id = (
    SELECT m.product_conversation_id FROM migration_070_membership m
    WHERE m.conversation_id = conversations.id
);
DROP TABLE migration_070_membership;


CREATE TEMP TABLE migration_070_continuation_shape_guard (
    invalid_edge_count INTEGER NOT NULL CHECK (invalid_edge_count = 0)
);
INSERT INTO migration_070_continuation_shape_guard
SELECT COUNT(*)
FROM conversations predecessor
LEFT JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
WHERE predecessor.continued_in_conv_id IS NOT NULL
  AND (
      predecessor.runtime_role NOT IN ('user', 'coordinator')
      OR predecessor.parent_conversation_id IS NOT NULL
      OR successor.id IS NULL
      OR successor.runtime_role <> predecessor.runtime_role
      OR successor.parent_conversation_id IS NOT NULL
      OR successor.product_conversation_id <> predecessor.product_conversation_id
  );
DROP TABLE migration_070_continuation_shape_guard;

CREATE UNIQUE INDEX conversations_one_predecessor_per_successor
ON conversations(continued_in_conv_id)
WHERE continued_in_conv_id IS NOT NULL;

CREATE TABLE product_continuation_reservations (
    predecessor_conversation_id TEXT PRIMARY KEY
        REFERENCES conversations(id) ON DELETE CASCADE,
    successor_conversation_id TEXT NOT NULL UNIQUE,
    product_conversation_id TEXT NOT NULL
        REFERENCES product_conversations(id) ON DELETE CASCADE,
    CHECK (predecessor_conversation_id <> successor_conversation_id)
);

CREATE TRIGGER conversations_require_single_product_root_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW WHEN NEW.runtime_role IN ('user', 'coordinator')
  AND NEW.parent_conversation_id IS NULL
  AND NOT EXISTS (
      SELECT 1 FROM conversations existing WHERE existing.id = NEW.id
  )
  AND NOT EXISTS (
      SELECT 1 FROM conversations predecessor
      WHERE predecessor.continued_in_conv_id = NEW.id
  )
  AND EXISTS (
      SELECT 1 FROM conversations member
      WHERE member.product_conversation_id = NEW.product_conversation_id
        AND member.runtime_role = NEW.runtime_role
        AND member.parent_conversation_id IS NULL
        AND NOT EXISTS (
            SELECT 1 FROM conversations predecessor
            WHERE predecessor.continued_in_conv_id = member.id
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'ProductConversation requires one parent-transcript root');
END;

CREATE TRIGGER conversations_require_product_membership_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW
WHEN NEW.product_conversation_id IS NULL
  OR NOT EXISTS (SELECT 1 FROM product_conversations p WHERE p.id = NEW.product_conversation_id)
BEGIN
    SELECT RAISE(ABORT, 'conversation requires ProductConversation membership');
END;

CREATE TRIGGER conversations_product_membership_is_immutable
BEFORE UPDATE OF product_conversation_id ON conversations
FOR EACH ROW WHEN OLD.product_conversation_id IS NOT NEW.product_conversation_id
BEGIN
    SELECT RAISE(ABORT, 'conversation ProductConversation membership is immutable');
END;

CREATE TRIGGER conversations_validate_product_kind_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW WHEN NOT EXISTS (
    SELECT 1 FROM product_conversations p
    WHERE p.id = NEW.product_conversation_id
      AND ((NEW.runtime_role = 'coordinator' AND p.kind = 'coordinator')
        OR (NEW.runtime_role = 'user' AND p.kind = 'ordinary')
        OR NEW.runtime_role = 'sub_agent')
)
BEGIN
    SELECT RAISE(ABORT, 'conversation runtime role does not match ProductConversation kind');
END;

CREATE TRIGGER conversations_validate_product_kind_on_update
BEFORE UPDATE OF runtime_role ON conversations
FOR EACH ROW WHEN NOT EXISTS (
    SELECT 1 FROM product_conversations p
    WHERE p.id = NEW.product_conversation_id
      AND ((NEW.runtime_role = 'coordinator' AND p.kind = 'coordinator')
        OR (NEW.runtime_role = 'user' AND p.kind = 'ordinary')
        OR NEW.runtime_role = 'sub_agent')
)
BEGIN
    SELECT RAISE(ABORT, 'conversation runtime role does not match ProductConversation kind');
END;

CREATE TRIGGER conversations_validate_product_parent_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW WHEN
    (NEW.runtime_role = 'sub_agent' AND NOT EXISTS (
        SELECT 1 FROM conversations parent
        WHERE parent.id = NEW.parent_conversation_id
          AND parent.product_conversation_id = NEW.product_conversation_id
    ))
    OR (NEW.runtime_role <> 'sub_agent' AND NEW.parent_conversation_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'only subordinate executions may have a same-ProductConversation parent');
END;

CREATE TRIGGER conversations_validate_product_parent_on_update
BEFORE UPDATE OF parent_conversation_id, runtime_role ON conversations
FOR EACH ROW WHEN
    (NEW.runtime_role = 'sub_agent' AND NOT EXISTS (
        SELECT 1 FROM conversations parent
        WHERE parent.id = NEW.parent_conversation_id
          AND parent.product_conversation_id = NEW.product_conversation_id
    ))
    OR (NEW.runtime_role <> 'sub_agent' AND NEW.parent_conversation_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'only subordinate executions may have a same-ProductConversation parent');
END;

CREATE TRIGGER conversations_require_single_product_root_on_update
BEFORE UPDATE OF runtime_role, parent_conversation_id, product_conversation_id ON conversations
FOR EACH ROW WHEN NEW.runtime_role IN ('user', 'coordinator')
  AND NEW.parent_conversation_id IS NULL
  AND NOT EXISTS (
      SELECT 1 FROM conversations predecessor
      WHERE predecessor.continued_in_conv_id = NEW.id
        AND predecessor.id <> OLD.id
  )
  AND EXISTS (
      SELECT 1 FROM conversations member
      WHERE member.product_conversation_id = NEW.product_conversation_id
        AND member.runtime_role = NEW.runtime_role
        AND member.parent_conversation_id IS NULL
        AND member.id <> OLD.id
        AND NOT EXISTS (
            SELECT 1 FROM conversations predecessor
            WHERE predecessor.continued_in_conv_id = member.id
              AND predecessor.id <> OLD.id
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'ProductConversation requires one parent-transcript root');
END;

CREATE TRIGGER conversations_preserve_parent_subordinate_topology_on_update
BEFORE UPDATE OF runtime_role, parent_conversation_id ON conversations
FOR EACH ROW WHEN
    (NEW.runtime_role = 'sub_agent' AND (
        NEW.parent_conversation_id IS NULL
        OR NEW.continued_in_conv_id IS NOT NULL
        OR EXISTS (
            SELECT 1 FROM conversations predecessor
            WHERE predecessor.continued_in_conv_id = NEW.id
        )
    ))
    OR (NEW.runtime_role IN ('user', 'coordinator')
        AND NEW.parent_conversation_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'parent transcripts and subordinate executions have disjoint topology');
END;

CREATE TRIGGER conversations_validate_product_continuation_on_update
BEFORE UPDATE OF continued_in_conv_id ON conversations
FOR EACH ROW WHEN
    (OLD.continued_in_conv_id IS NOT NULL
     AND OLD.continued_in_conv_id IS NOT NEW.continued_in_conv_id)
    OR (NEW.continued_in_conv_id IS NOT NULL AND (
        NEW.runtime_role NOT IN ('user', 'coordinator')
        OR NEW.parent_conversation_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1 FROM conversations successor
            WHERE successor.id = NEW.continued_in_conv_id
              AND successor.product_conversation_id = NEW.product_conversation_id
              AND successor.parent_conversation_id IS NULL
              AND successor.runtime_role = NEW.runtime_role
        )
        AND NOT EXISTS (
            SELECT 1 FROM product_continuation_reservations reservation
            WHERE reservation.predecessor_conversation_id = NEW.id
              AND reservation.successor_conversation_id = NEW.continued_in_conv_id
              AND reservation.product_conversation_id = NEW.product_conversation_id
        )
    ))
BEGIN
    SELECT RAISE(ABORT, 'continuation must connect parent transcripts in one ProductConversation');
END;

CREATE TRIGGER conversations_validate_product_continuation_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW WHEN NEW.continued_in_conv_id IS NOT NULL AND (
    NEW.runtime_role NOT IN ('user', 'coordinator')
    OR NEW.parent_conversation_id IS NOT NULL
    OR NOT EXISTS (
        SELECT 1 FROM conversations successor
        WHERE successor.id = NEW.continued_in_conv_id
          AND successor.product_conversation_id = NEW.product_conversation_id
          AND successor.parent_conversation_id IS NULL
          AND successor.runtime_role = NEW.runtime_role
    )
)
BEGIN
    SELECT RAISE(ABORT, 'continuation must connect parent transcripts in one ProductConversation');
END;

CREATE TRIGGER conversations_reject_product_continuation_cycle_on_update
BEFORE UPDATE OF continued_in_conv_id ON conversations
FOR EACH ROW WHEN NEW.continued_in_conv_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE successors(conversation_id) AS (
        SELECT NEW.continued_in_conv_id
        UNION
        SELECT current.continued_in_conv_id
        FROM successors
        JOIN conversations current ON current.id = successors.conversation_id
        WHERE current.continued_in_conv_id IS NOT NULL
    )
    SELECT 1 FROM successors WHERE conversation_id = NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'continuation topology must be acyclic');
END;

CREATE TRIGGER conversations_reject_product_continuation_cycle_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW WHEN NEW.continued_in_conv_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE successors(conversation_id) AS (
        SELECT NEW.continued_in_conv_id
        UNION
        SELECT current.continued_in_conv_id
        FROM successors
        JOIN conversations current ON current.id = successors.conversation_id
        WHERE current.continued_in_conv_id IS NOT NULL
    )
    SELECT 1 FROM successors WHERE conversation_id = NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'continuation topology must be acyclic');
END;

CREATE TRIGGER product_continuation_reservations_validate_on_insert
BEFORE INSERT ON product_continuation_reservations
FOR EACH ROW WHEN NOT EXISTS (
    SELECT 1 FROM conversations predecessor
    WHERE predecessor.id = NEW.predecessor_conversation_id
      AND predecessor.product_conversation_id = NEW.product_conversation_id
      AND predecessor.runtime_role IN ('user', 'coordinator')
      AND predecessor.parent_conversation_id IS NULL
      AND predecessor.continued_in_conv_id IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'continuation reservation requires an uncontinued parent transcript');
END;

CREATE TRIGGER product_continuation_reservations_validate_on_delete
BEFORE DELETE ON product_continuation_reservations
FOR EACH ROW WHEN NOT EXISTS (
    SELECT 1 FROM conversations predecessor
    JOIN conversations successor ON successor.id = predecessor.continued_in_conv_id
    WHERE predecessor.id = OLD.predecessor_conversation_id
      AND predecessor.continued_in_conv_id = OLD.successor_conversation_id
      AND successor.product_conversation_id = OLD.product_conversation_id
      AND successor.runtime_role = predecessor.runtime_role
      AND successor.parent_conversation_id IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'continuation reservation requires a completed continuation edge');
END;

CREATE TABLE product_conversation_sources (
    target_product_conversation_id TEXT PRIMARY KEY
        REFERENCES product_conversations(id) ON DELETE CASCADE,
    source_product_conversation_id TEXT NOT NULL
        CHECK (typeof(source_product_conversation_id) = 'text'
               AND source_product_conversation_id <> ''),
    source_conversation_id TEXT NOT NULL
        CHECK (typeof(source_conversation_id) = 'text'
               AND source_conversation_id <> ''),
    relation_kind TEXT NOT NULL CHECK (relation_kind IN ('approved_task')),
    relation_key TEXT NOT NULL
        CHECK (typeof(relation_key) = 'text' AND relation_key <> ''),
    created_at_us INTEGER NOT NULL
        CHECK (typeof(created_at_us) = 'integer' AND created_at_us >= 0),
    CHECK (target_product_conversation_id <> source_product_conversation_id)
);

CREATE INDEX product_conversation_sources_by_source
ON product_conversation_sources(source_product_conversation_id);

CREATE UNIQUE INDEX product_conversation_sources_one_relation_key
ON product_conversation_sources(
    source_product_conversation_id, relation_kind, relation_key
);

DROP TRIGGER close_obligations_require_admission_phase_on_insert;

DROP TRIGGER close_obligations_reject_invalid_timestamps_on_insert;

DROP TRIGGER close_obligations_reject_invalid_timestamps_on_update;

DROP TRIGGER close_obligations_reject_active_standalone_delete;

DROP TRIGGER close_obligations_completed_outcome_is_immutable;

DROP TRIGGER close_obligations_require_archived_members_for_completion;

DROP TRIGGER close_obligations_require_open_members_for_cancelled_completion;

DROP TRIGGER close_obligations_require_complete_retirement_proof;

DROP TRIGGER close_obligations_require_topology_seal_before_phase_transition;

DROP TRIGGER close_obligations_transition_graph;

DROP TRIGGER close_obligations_root_is_immutable;

DROP TRIGGER close_obligations_created_at_is_immutable;

DROP TRIGGER close_obligations_chronology_ordinal_is_immutable;

DROP TRIGGER close_obligations_chronology_must_be_database_allocated;

DROP TRIGGER close_obligations_require_closed_timestamps;

DROP TRIGGER close_obligations_reject_inspection_pair_mismatch_on_update;

DROP TRIGGER close_obligations_reject_missing_inspection_on_update;

DROP TRIGGER close_obligations_require_complete_inspection_scope_coverage;

DROP TRIGGER close_obligations_require_loss_consistent_branch_from_inspection;

DROP TRIGGER close_obligations_invalidate_inspection_on_reentry;

DROP TRIGGER close_obligations_snapshot_matches_inspection_aggregate;

DROP TRIGGER close_obligations_touch_updated_at;

DROP TRIGGER close_obligations_validate_topology_before_seal;

DROP TRIGGER close_obligations_topology_seal_is_monotonic;

DROP TRIGGER close_obligations_require_member_cleanup_before_delete;

DROP TRIGGER close_obligations_require_residual_before_needs_repair;

DROP TRIGGER close_obligations_preserve_dependent_absence_on_delete;

DROP TRIGGER close_obligations_preserve_dependent_absence_on_snapshot_update;

DROP TRIGGER conversations_reject_close_root_identity_change;

DROP TRIGGER close_attempt_members_reject_delete_after_topology_seal;

DROP TRIGGER close_attempt_members_preserve_target_scope_on_delete;

DROP TRIGGER close_attempt_scopes_preserve_captured_target_on_delete;

DROP TRIGGER close_retirement_inspections_reject_sealed_delete;

DROP TRIGGER close_retirement_losses_require_open_inspection_on_delete;

DROP TRIGGER close_retirement_inventories_reject_distinct_owner_before_seal;

DROP TRIGGER close_retirement_inventories_reject_standalone_delete;

DROP TRIGGER close_expected_retirement_resources_reject_standalone_delete;

DROP TRIGGER close_retirement_resources_require_absence_proof_on_insert;

DROP TRIGGER close_retirement_resources_require_absence_proof_on_update;

DROP TRIGGER close_retirement_resources_reject_standalone_delete;

DROP TRIGGER close_retirement_resources_preserve_dependent_absence_on_delete;

DROP TRIGGER close_retirement_resources_preserve_dependent_absence_on_update;

DROP INDEX close_obligations_one_active_per_root;
ALTER TABLE close_obligations
    ADD COLUMN product_conversation_id TEXT REFERENCES product_conversations(id) ON DELETE CASCADE;
UPDATE close_obligations
SET product_conversation_id = root_conversation_id;
ALTER TABLE close_obligations DROP COLUMN root_conversation_id;

CREATE TRIGGER close_obligations_require_product_membership_on_insert
BEFORE INSERT ON close_obligations
FOR EACH ROW WHEN NEW.product_conversation_id IS NULL
BEGIN
    SELECT RAISE(ABORT, 'close obligation requires ProductConversation identity');
END;

CREATE TRIGGER close_obligations_require_admission_phase_on_insert
BEFORE INSERT ON close_obligations
FOR EACH ROW
WHEN NEW.phase <> 'awaiting_blocker_resolution'
  OR NEW.topology_sealed <> 0
  OR NEW.inspection_generation IS NOT NULL
  OR NEW.inspection_fingerprint IS NOT NULL
  OR NEW.completed_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'close obligation must begin at admission phase');
END;

CREATE TRIGGER close_obligations_reject_invalid_timestamps_on_insert
BEFORE INSERT ON close_obligations
FOR EACH ROW
WHEN (
      NEW.created_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.created_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 26) GLOB '*[^0-9]*')
  )
  OR (
      NEW.updated_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.updated_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.created_at, 1, 10), '+0 days') <> SUBSTR(NEW.created_at, 1, 10)
  OR CAST(SUBSTR(NEW.created_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.created_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.created_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.created_at) IS NULL
  OR date(SUBSTR(NEW.updated_at, 1, 10), '+0 days') <> SUBSTR(NEW.updated_at, 1, 10)
  OR CAST(SUBSTR(NEW.updated_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.updated_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.updated_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.updated_at) IS NULL
  OR (
      NEW.completed_at IS NOT NULL
      AND (
          (
              NEW.completed_at NOT GLOB '????-??-??T??:??:??Z'
              AND NEW.completed_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 21) GLOB '*[^0-9]*')
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 26) GLOB '*[^0-9]*')
          )
          OR date(SUBSTR(NEW.completed_at, 1, 10), '+0 days') <> SUBSTR(NEW.completed_at, 1, 10)
          OR CAST(SUBSTR(NEW.completed_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
          OR CAST(SUBSTR(NEW.completed_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR CAST(SUBSTR(NEW.completed_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR julianday(NEW.completed_at) IS NULL
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation timestamps must be valid RFC 3339');
END;

CREATE TRIGGER close_obligations_reject_invalid_timestamps_on_update
BEFORE UPDATE OF created_at, updated_at, completed_at ON close_obligations
FOR EACH ROW
WHEN (
      NEW.created_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.created_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 26) GLOB '*[^0-9]*')
  )
  OR (
      NEW.updated_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.updated_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.created_at, 1, 10), '+0 days') <> SUBSTR(NEW.created_at, 1, 10)
  OR CAST(SUBSTR(NEW.created_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.created_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.created_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.created_at) IS NULL
  OR date(SUBSTR(NEW.updated_at, 1, 10), '+0 days') <> SUBSTR(NEW.updated_at, 1, 10)
  OR CAST(SUBSTR(NEW.updated_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.updated_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.updated_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.updated_at) IS NULL
  OR (
      NEW.completed_at IS NOT NULL
      AND (
          (
              NEW.completed_at NOT GLOB '????-??-??T??:??:??Z'
              AND NEW.completed_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 21) GLOB '*[^0-9]*')
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 26) GLOB '*[^0-9]*')
          )
          OR date(SUBSTR(NEW.completed_at, 1, 10), '+0 days') <> SUBSTR(NEW.completed_at, 1, 10)
          OR CAST(SUBSTR(NEW.completed_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
          OR CAST(SUBSTR(NEW.completed_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR CAST(SUBSTR(NEW.completed_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR julianday(NEW.completed_at) IS NULL
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation timestamps must be valid RFC 3339');
END;

CREATE TRIGGER close_obligations_reject_active_standalone_delete
BEFORE DELETE ON close_obligations
FOR EACH ROW
WHEN OLD.phase <> 'completed' AND EXISTS (
    SELECT 1 FROM product_conversations WHERE id = OLD.product_conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'active close obligation can only be deleted with its root');
END;

CREATE TRIGGER close_obligations_completed_outcome_is_immutable
BEFORE UPDATE OF phase, close_outcome, completed_at, updated_at ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'completed'
  AND (
      NEW.phase IS NOT OLD.phase
      OR NEW.close_outcome IS NOT OLD.close_outcome
      OR NEW.completed_at IS NOT OLD.completed_at
      OR NEW.updated_at IS NOT OLD.updated_at
  )
BEGIN
    SELECT RAISE(ABORT, 'completed close outcome is immutable');
END;

CREATE TRIGGER close_obligations_require_archived_members_for_completion
BEFORE UPDATE OF phase, close_outcome ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'completed'
  AND NEW.close_outcome = 'archived'
  AND EXISTS (
      SELECT 1 FROM close_attempt_members member
      JOIN conversations conversation ON conversation.id = member.conversation_id
      WHERE member.attempt_id = OLD.attempt_id AND conversation.archived <> 1
  )
BEGIN
    SELECT RAISE(ABORT, 'close completion requires archived captured members');
END;

CREATE TRIGGER close_obligations_require_open_members_for_cancelled_completion
BEFORE UPDATE OF phase, close_outcome ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'completed'
  AND NEW.close_outcome = 'cancelled'
  AND EXISTS (
      SELECT 1 FROM close_attempt_members member
      JOIN conversations conversation ON conversation.id = member.conversation_id
      WHERE member.attempt_id = OLD.attempt_id AND conversation.archived <> 0
  )
BEGIN
    SELECT RAISE(ABORT, 'cancelled close completion requires open captured members');
END;

CREATE TRIGGER close_obligations_require_complete_retirement_proof
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase IN ('retirement_requested', 'needs_repair')
  AND NEW.phase = 'completed'
  AND (
      EXISTS (
          SELECT 1 FROM close_attempt_scopes target
          WHERE target.attempt_id = OLD.attempt_id
            AND NOT EXISTS (
                SELECT 1 FROM close_retirement_inventories inventory
                WHERE inventory.attempt_id = target.attempt_id
                  AND inventory.scope = target.scope
                  AND inventory.inspection_generation = OLD.inspection_generation
                  AND inventory.inspection_fingerprint = OLD.inspection_fingerprint
                  AND inventory.sealed = 1
            )
      )
      OR EXISTS (
          SELECT 1
          FROM close_attempt_scopes target
          JOIN work_scopes scope ON scope.id = target.scope
          WHERE target.attempt_id = OLD.attempt_id
            AND scope.environment_kind = 'allocated_worktree'
            AND NOT EXISTS (
                SELECT 1 FROM close_expected_retirement_resources expected
                WHERE expected.attempt_id = target.attempt_id
                  AND expected.scope = target.scope
                  AND expected.inspection_generation = OLD.inspection_generation
                  AND expected.inspection_fingerprint = OLD.inspection_fingerprint
                  AND expected.resource_kind = 'worktree'
            )
      )
      OR EXISTS (
          SELECT 1 FROM close_expected_retirement_resources expected
          WHERE expected.attempt_id = OLD.attempt_id
            AND expected.inspection_generation = OLD.inspection_generation
            AND expected.inspection_fingerprint = OLD.inspection_fingerprint
            AND NOT EXISTS (
                SELECT 1 FROM close_retirement_resources proof
                WHERE proof.attempt_id = expected.attempt_id
                  AND proof.scope = expected.scope
                  AND proof.inspection_generation = expected.inspection_generation
                  AND proof.inspection_fingerprint = expected.inspection_fingerprint
                  AND proof.resource_kind = expected.resource_kind
                  AND proof.identity_kind = expected.identity_kind
                  AND proof.identity_codec = expected.identity_codec
                  AND proof.identity_value = expected.identity_value
                  AND proof.proof_kind IN ('retired', 'absence_adopted')
            )
      )
      OR EXISTS (
          SELECT 1 FROM close_retirement_resources resource
          WHERE resource.attempt_id = OLD.attempt_id
            AND resource.inspection_generation = OLD.inspection_generation
            AND resource.inspection_fingerprint = OLD.inspection_fingerprint
            AND resource.proof_kind = 'residual'
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation lacks complete retirement proof');
END;

CREATE TRIGGER close_obligations_require_topology_seal_before_phase_transition
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase <> NEW.phase
  AND OLD.topology_sealed <> 1
BEGIN
    SELECT RAISE(ABORT, 'close obligation phase transition requires sealed topology');
END;

CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;

CREATE TRIGGER close_obligations_root_is_immutable
BEFORE UPDATE OF product_conversation_id ON close_obligations
FOR EACH ROW
WHEN OLD.product_conversation_id IS NOT NEW.product_conversation_id
BEGIN
    SELECT RAISE(ABORT, 'close obligation ProductConversation is immutable');
END;

CREATE TRIGGER close_obligations_created_at_is_immutable
BEFORE UPDATE OF created_at ON close_obligations
FOR EACH ROW
WHEN OLD.created_at <> NEW.created_at
BEGIN
    SELECT RAISE(ABORT, 'close obligation creation timestamp is immutable');
END;

CREATE TRIGGER close_obligations_chronology_ordinal_is_immutable
BEFORE UPDATE OF chronology_ordinal ON close_obligations
FOR EACH ROW
WHEN OLD.chronology_ordinal <> NEW.chronology_ordinal
BEGIN
    SELECT RAISE(ABORT, 'close obligation chronology ordinal is immutable');
END;

CREATE TRIGGER close_obligations_chronology_must_be_database_allocated
BEFORE INSERT ON close_obligations
FOR EACH ROW
WHEN NEW.chronology_ordinal <> -1
BEGIN
    SELECT RAISE(ABORT, 'close obligation chronology must be database allocated');
END;

CREATE TRIGGER close_obligations_require_closed_timestamps
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN ((NEW.phase = 'completed') <> (NEW.completed_at IS NOT NULL))
  OR ((NEW.phase = 'completed') <> (NEW.close_outcome IS NOT NULL))
  OR (OLD.phase <> 'completed' AND NEW.phase = 'completed'
      AND NEW.close_outcome = 'cancelled'
      AND OLD.phase IN ('retirement_requested', 'needs_repair'))
  OR (OLD.phase <> 'completed' AND NEW.phase = 'completed'
      AND NEW.close_outcome = 'archived'
      AND OLD.phase NOT IN ('retirement_requested', 'needs_repair'))
BEGIN
    SELECT RAISE(ABORT, 'close_obligations completion outcome must match legal source phase');
END;

CREATE TRIGGER close_obligations_reject_inspection_pair_mismatch_on_update
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN ((NEW.inspection_generation IS NULL) <> (NEW.inspection_fingerprint IS NULL))
BEGIN
    SELECT RAISE(ABORT, 'close_obligations inspection_generation/fingerprint must both be null or both nonnull');
END;

CREATE TRIGGER close_obligations_reject_missing_inspection_on_update
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN ((NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair')) AND NEW.inspection_generation IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'close_obligations inspection required for phase');
END;

CREATE TRIGGER close_obligations_require_complete_inspection_scope_coverage
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'awaiting_retirement_inspection'
  AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested')
  AND EXISTS (
      SELECT 1
      FROM close_attempt_scopes target
      JOIN work_scopes scope ON scope.id = target.scope
      WHERE target.attempt_id = NEW.attempt_id
        AND scope.environment_kind = 'allocated_worktree'
        AND NOT EXISTS (
            SELECT 1 FROM close_retirement_inspections inspection
            WHERE inspection.attempt_id = target.attempt_id
              AND inspection.scope = target.scope
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'close inspection must cover every targeted allocated worktree scope');
END;

CREATE TRIGGER close_obligations_require_loss_consistent_branch_from_inspection
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'awaiting_retirement_inspection'
  AND (
      (
          NEW.phase = 'retirement_requested'
          AND EXISTS (
              SELECT 1 FROM close_retirement_losses loss
              WHERE loss.attempt_id = NEW.attempt_id
          )
      )
      OR (
          NEW.phase = 'awaiting_loss_confirmation'
          AND NOT EXISTS (
              SELECT 1 FROM close_retirement_losses loss
              WHERE loss.attempt_id = NEW.attempt_id
          )
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation phase must match persisted inspection losses');
END;

CREATE TRIGGER close_obligations_invalidate_inspection_on_reentry
AFTER UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'awaiting_retirement_inspection'
  AND OLD.phase <> 'awaiting_retirement_inspection'
BEGIN
    DELETE FROM close_retirement_inspections WHERE attempt_id = NEW.attempt_id;
    UPDATE close_obligations
    SET inspection_generation = NULL, inspection_fingerprint = NULL
    WHERE attempt_id = NEW.attempt_id;
END;

CREATE TRIGGER close_obligations_snapshot_matches_inspection_aggregate
BEFORE UPDATE OF inspection_generation, inspection_fingerprint ON close_obligations
FOR EACH ROW
WHEN (
    OLD.inspection_generation IS NOT NEW.inspection_generation
    OR OLD.inspection_fingerprint IS NOT NEW.inspection_fingerprint
) AND NOT (
    NEW.phase = 'completed'
    AND NEW.close_outcome = 'cancelled'
    AND NEW.inspection_generation IS NULL
    AND NEW.inspection_fingerprint IS NULL
) AND (
    OLD.phase <> 'awaiting_retirement_inspection'
    OR NEW.inspection_generation <> CASE
        WHEN EXISTS (
            SELECT 1 FROM close_retirement_inspections
            WHERE attempt_id = NEW.attempt_id
        ) THEN (
            SELECT 'v1' || COALESCE(GROUP_CONCAT(component, ''), '')
            FROM (
                SELECT generation,
                       LENGTH(CAST(scope AS BLOB)) || ':' || scope ||
                       LENGTH(CAST(generation AS BLOB)) || ':' || generation AS component
                FROM close_retirement_inspections
                WHERE attempt_id = NEW.attempt_id
                ORDER BY scope
            )
        ) ELSE 'no-worktree'
    END
    OR NEW.inspection_fingerprint <> CASE
        WHEN EXISTS (
            SELECT 1 FROM close_retirement_inspections
            WHERE attempt_id = NEW.attempt_id
        ) THEN (
            SELECT 'v1' || COALESCE(GROUP_CONCAT(component, ''), '')
            FROM (
                SELECT fingerprint,
                       LENGTH(CAST(scope AS BLOB)) || ':' || scope ||
                       LENGTH(CAST(fingerprint AS BLOB)) || ':' || fingerprint AS component
                FROM close_retirement_inspections
                WHERE attempt_id = NEW.attempt_id
                ORDER BY scope
            )
        ) ELSE 'no-worktree'
    END
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation snapshot must match atomic inspection replacement');
END;

CREATE TRIGGER close_obligations_touch_updated_at
AFTER UPDATE ON close_obligations
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
  AND NEW.phase <> 'completed'
  AND julianday('now') > julianday(OLD.updated_at)
BEGIN
    UPDATE close_obligations
    SET updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE attempt_id = NEW.attempt_id;
END;

CREATE TRIGGER close_obligations_validate_topology_before_seal
BEFORE UPDATE OF topology_sealed ON close_obligations
FOR EACH ROW
WHEN OLD.topology_sealed = 0 AND NEW.topology_sealed = 1 AND (
    NOT EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.continuation_ordinal = 0
          AND EXISTS (SELECT 1 FROM conversations root_member WHERE root_member.id = member.conversation_id AND root_member.product_conversation_id = OLD.product_conversation_id)
          AND member.member_role IN ('root', 'root_latest')
    )
    OR NOT EXISTS (
        SELECT 1 FROM conversations root
        WHERE root.product_conversation_id = OLD.product_conversation_id
          AND root.runtime_role = 'user'
          AND root.parent_conversation_id IS NULL
          AND root.user_initiated = 1
          AND root.archived = 0
          AND NOT EXISTS (
              SELECT 1 FROM conversations predecessor
              WHERE predecessor.product_conversation_id = root.product_conversation_id
                AND predecessor.continued_in_conv_id = root.id
          )
    )
    OR EXISTS (
        SELECT 1
        FROM close_attempt_members latest
        JOIN conversations live ON live.id = latest.conversation_id
        WHERE latest.attempt_id = OLD.attempt_id
          AND latest.member_role IN ('latest', 'root_latest')
          AND live.state_kind = 'handed_off'
          AND live.continued_in_conv_id IS NULL
    )
    OR EXISTS (
        SELECT 1
        FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND live.state_kind IN ('awaiting_task_approval', 'awaiting_continuation')
    )
    OR NOT EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.member_role IN ('latest', 'root_latest')
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members latest
        WHERE latest.attempt_id = OLD.attempt_id
          AND latest.member_role IN ('latest', 'root_latest')
          AND (
              latest.captured_continued_in_conv_id IS NOT NULL
              OR EXISTS (
                  SELECT 1 FROM close_attempt_members later
                  WHERE later.attempt_id = latest.attempt_id
                    AND later.continuation_ordinal > latest.continuation_ordinal
              )
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.continuation_ordinal > 0
          AND NOT EXISTS (
              SELECT 1 FROM close_attempt_members predecessor
              WHERE predecessor.attempt_id = member.attempt_id
                AND predecessor.continuation_ordinal = member.continuation_ordinal - 1
                AND predecessor.captured_continued_in_conv_id = member.conversation_id
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND (
              member.captured_continued_in_conv_id IS NOT live.continued_in_conv_id
              OR member.captured_state_kind <> live.state_kind
              OR member.captured_runtime_role <> live.runtime_role
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND live.archived <> 0
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND (
              (
                  member.continuation_ordinal = 0
                  AND (
                      SELECT COUNT(*) FROM conversations predecessor
                      WHERE predecessor.continued_in_conv_id = member.conversation_id
                  ) <> 0
              )
              OR (
                  member.continuation_ordinal > 0
                  AND (
                      SELECT COUNT(*) FROM conversations predecessor
                      WHERE predecessor.continued_in_conv_id = member.conversation_id
                  ) <> 1
              )
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND member.captured_work_scope_id IS NOT live.work_scope_id
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.captured_work_scope_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM close_attempt_scopes scope
              WHERE scope.attempt_id = member.attempt_id
                AND scope.scope = member.captured_work_scope_id
          )
    )
    OR EXISTS (
        SELECT 1
        FROM close_attempt_scopes captured
        JOIN work_scopes live ON live.id = captured.scope
        WHERE captured.attempt_id = OLD.attempt_id
          AND (
              (live.environment_kind = 'allocated_worktree'
               AND (captured.captured_worktree_identity IS NOT live.worktree_id
                   OR captured.captured_worktree_fingerprint IS NOT live.worktree_fingerprint
                   OR captured.captured_worktree_locator IS NOT
                       'git_path_bytes_hex_v1:' || lower(hex(CAST(live.worktree_path AS BLOB)))))
              OR (live.environment_kind <> 'allocated_worktree'
                  AND (captured.captured_worktree_identity IS NOT NULL
                      OR captured.captured_worktree_fingerprint IS NOT NULL
                      OR captured.captured_worktree_locator IS NOT NULL))
          )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'captured close topology is incomplete');
END;

CREATE TRIGGER close_obligations_topology_seal_is_monotonic
BEFORE UPDATE OF topology_sealed ON close_obligations
FOR EACH ROW
WHEN OLD.topology_sealed = 1 AND NEW.topology_sealed <> 1
BEGIN
    SELECT RAISE(ABORT, 'captured close topology seal is immutable');
END;

CREATE TRIGGER close_obligations_require_member_cleanup_before_delete
BEFORE DELETE ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'completed'
  AND (
      EXISTS (
          SELECT 1 FROM close_attempt_members member
          WHERE member.attempt_id = OLD.attempt_id
      )
      OR EXISTS (
          SELECT 1 FROM close_attempt_scopes scope
          WHERE scope.attempt_id = OLD.attempt_id
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'completed Close history must remove member snapshots before obligation deletion');
END;

CREATE TRIGGER close_obligations_require_residual_before_needs_repair
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'retirement_requested'
  AND NEW.phase = 'needs_repair'
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_resources resource
      WHERE resource.attempt_id = OLD.attempt_id
        AND resource.inspection_generation = OLD.inspection_generation
        AND resource.inspection_fingerprint = OLD.inspection_fingerprint
        AND resource.proof_kind = 'residual'
  )
  AND NOT EXISTS (
      SELECT 1
      FROM close_attempt_scopes captured
      WHERE captured.attempt_id = OLD.attempt_id
        AND captured.captured_worktree_identity IS NULL
        AND captured.captured_worktree_fingerprint IS NULL
        AND captured.captured_worktree_locator IS NOT NULL
  )
BEGIN
    SELECT RAISE(ABORT, 'needs_repair requires current-snapshot residual evidence');
END;

CREATE TRIGGER close_obligations_preserve_dependent_absence_on_delete
BEFORE DELETE ON close_obligations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM conversations root
    JOIN close_retirement_resources proof ON proof.attempt_id = OLD.attempt_id
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.product_conversation_id = OLD.product_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE root.product_conversation_id = OLD.product_conversation_id
      AND root.runtime_role = 'user'
      AND root.parent_conversation_id IS NULL
      AND root.user_initiated = 1
      AND NOT EXISTS (
          SELECT 1 FROM conversations predecessor
          WHERE predecessor.product_conversation_id = root.product_conversation_id
            AND predecessor.continued_in_conv_id = root.id
      )
      AND proof.inspection_generation = OLD.inspection_generation
      AND proof.inspection_fingerprint = OLD.inspection_fingerprint
      AND proof.proof_kind IN ('retired', 'absence_adopted')
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = proof.scope
      AND dependent.resource_kind = proof.resource_kind
      AND dependent.identity_kind = proof.identity_kind
      AND dependent.identity_codec = proof.identity_codec
      AND dependent.identity_value = proof.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation retains proof for adopted absence');
END;

CREATE TRIGGER close_obligations_preserve_dependent_absence_on_snapshot_update
BEFORE UPDATE OF inspection_generation, inspection_fingerprint ON close_obligations
FOR EACH ROW
WHEN (
    OLD.inspection_generation IS NOT NEW.inspection_generation
    OR OLD.inspection_fingerprint IS NOT NEW.inspection_fingerprint
) AND EXISTS (
    SELECT 1
    FROM close_retirement_resources proof
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.product_conversation_id = OLD.product_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE proof.attempt_id = OLD.attempt_id
      AND proof.inspection_generation = OLD.inspection_generation
      AND proof.inspection_fingerprint = OLD.inspection_fingerprint
      AND proof.proof_kind IN ('retired', 'absence_adopted')
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = proof.scope
      AND dependent.resource_kind = proof.resource_kind
      AND dependent.identity_kind = proof.identity_kind
      AND dependent.identity_codec = proof.identity_codec
      AND dependent.identity_value = proof.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation retains proof for adopted absence');
END;

CREATE TRIGGER conversations_reject_close_product_membership_change
BEFORE UPDATE OF product_conversation_id ON conversations
FOR EACH ROW WHEN OLD.product_conversation_id IS NOT NEW.product_conversation_id
 AND EXISTS (SELECT 1 FROM close_obligations o WHERE o.product_conversation_id = OLD.product_conversation_id AND o.phase <> 'completed')
BEGIN
    SELECT RAISE(ABORT, 'active Close preserves ProductConversation membership');
END;

CREATE TRIGGER close_attempt_members_reject_delete_after_topology_seal
BEFORE DELETE ON close_attempt_members
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.topology_sealed = 1
      AND obligation.phase <> 'completed'
)
BEGIN
    SELECT RAISE(ABORT, 'captured close topology is sealed');
END;

CREATE TRIGGER close_attempt_members_preserve_target_scope_on_delete
BEFORE DELETE ON close_attempt_members
FOR EACH ROW
WHEN OLD.captured_work_scope_id IS NOT NULL AND EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'completed'
) AND EXISTS (
    SELECT 1 FROM close_attempt_scopes target
    WHERE target.attempt_id = OLD.attempt_id AND target.scope = OLD.captured_work_scope_id
) AND NOT EXISTS (
    SELECT 1 FROM close_attempt_members member
    WHERE member.attempt_id = OLD.attempt_id
      AND member.captured_work_scope_id = OLD.captured_work_scope_id
      AND member.conversation_id <> OLD.conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'captured member scope is targeted by close attempt');
END;

CREATE TRIGGER close_attempt_scopes_preserve_captured_target_on_delete
BEFORE DELETE ON close_attempt_scopes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'completed'
)
BEGIN
    SELECT RAISE(ABORT, 'captured close target is immutable');
END;

CREATE TRIGGER close_retirement_inspections_reject_sealed_delete
BEFORE DELETE ON close_retirement_inspections
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'persisted close inspection snapshot is sealed');
END;

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_delete
BEFORE DELETE ON close_retirement_losses
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TRIGGER close_retirement_inventories_reject_distinct_owner_before_seal
BEFORE UPDATE OF sealed ON close_retirement_inventories
FOR EACH ROW
WHEN OLD.sealed = 0
  AND NEW.sealed = 1
  AND EXISTS (
      WITH RECURSIVE open_candidates(id) AS (
          SELECT id FROM conversations
          WHERE work_scope_id = NEW.scope
            AND runtime_role = 'user'
            AND parent_conversation_id IS NULL
            AND archived = 0
      ), ancestry(candidate_id, id, path) AS (
          SELECT id, id, json_array(id) FROM open_candidates
          UNION ALL
          SELECT ancestry.candidate_id, predecessor.id,
                 json_insert(ancestry.path, '$[#]', predecessor.id)
          FROM ancestry
          JOIN conversations predecessor
            ON predecessor.continued_in_conv_id = ancestry.id
          WHERE NOT EXISTS (
              SELECT 1 FROM json_each(ancestry.path) visited
              WHERE visited.value = predecessor.id
          )
      ), resolved(candidate_id, root_id) AS (
          SELECT ancestry.candidate_id, root.product_conversation_id
          FROM ancestry
          JOIN conversations root ON root.id = ancestry.id
          WHERE NOT EXISTS (
              SELECT 1 FROM conversations predecessor
              WHERE predecessor.continued_in_conv_id = ancestry.id
          )
      ), captured_root(id) AS (
          SELECT product_conversation_id FROM close_obligations
          WHERE attempt_id = NEW.attempt_id
      )
      SELECT 1
      FROM open_candidates candidate
      LEFT JOIN resolved ON resolved.candidate_id = candidate.id
      CROSS JOIN captured_root
      GROUP BY candidate.id, captured_root.id
      HAVING COUNT(resolved.root_id) <> 1
          OR MAX(resolved.root_id) <> captured_root.id
  )
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory scope is retained by distinct open aggregate');
END;

CREATE TRIGGER close_retirement_inventories_reject_standalone_delete
BEFORE DELETE ON close_retirement_inventories
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'captured retirement inventory can only be deleted with its root');
END;

CREATE TRIGGER close_expected_retirement_resources_reject_standalone_delete
BEFORE DELETE ON close_expected_retirement_resources
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'expected retirement resource can only be deleted with its root');
END;

CREATE TRIGGER close_retirement_resources_require_absence_proof_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NEW.proof_kind = 'absence_adopted' AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    JOIN close_obligations proof_obligation ON proof_obligation.attempt_id = proof.attempt_id
    JOIN close_obligations current_obligation ON current_obligation.attempt_id = NEW.attempt_id
    WHERE proof.scope = NEW.scope
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND (
          (NEW.absence_basis = 'same_attempt_prior_retirement'
           AND proof.attempt_id = NEW.attempt_id
           AND proof.inspection_generation = NEW.inspection_generation
           AND proof.inspection_fingerprint = NEW.inspection_fingerprint
           AND proof.proof_kind = 'retired')
          OR
          (NEW.absence_basis = 'preexisting_exact_identity_evidence'
           AND proof.attempt_id <> NEW.attempt_id
           AND proof_obligation.product_conversation_id = current_obligation.product_conversation_id
           AND proof.inspection_generation = proof_obligation.inspection_generation
           AND proof.inspection_fingerprint = proof_obligation.inspection_fingerprint
           AND proof.proof_kind IN ('retired', 'absence_adopted'))
      )
)
BEGIN
    SELECT RAISE(ABORT, 'adopted absence requires exact retained proof');
END;

CREATE TRIGGER close_retirement_resources_require_absence_proof_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN NEW.proof_kind = 'absence_adopted'
 AND (
     OLD.proof_kind <> 'absence_adopted'
     OR OLD.absence_basis IS NOT NEW.absence_basis
     OR OLD.attempt_id <> NEW.attempt_id
     OR OLD.scope <> NEW.scope
     OR OLD.inspection_generation <> NEW.inspection_generation
     OR OLD.inspection_fingerprint <> NEW.inspection_fingerprint
     OR OLD.resource_kind <> NEW.resource_kind
     OR OLD.identity_kind <> NEW.identity_kind
     OR OLD.identity_codec <> NEW.identity_codec
     OR OLD.identity_value <> NEW.identity_value
 )
 AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    JOIN close_obligations proof_obligation ON proof_obligation.attempt_id = proof.attempt_id
    JOIN close_obligations current_obligation ON current_obligation.attempt_id = NEW.attempt_id
    WHERE proof.scope = NEW.scope
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND (
          (NEW.absence_basis = 'same_attempt_prior_retirement'
           AND proof.attempt_id = NEW.attempt_id
           AND proof.inspection_generation = NEW.inspection_generation
           AND proof.inspection_fingerprint = NEW.inspection_fingerprint
           AND proof.proof_kind = 'retired')
          OR
          (NEW.absence_basis = 'preexisting_exact_identity_evidence'
           AND proof.attempt_id <> NEW.attempt_id
           AND proof_obligation.product_conversation_id = current_obligation.product_conversation_id
           AND proof.inspection_generation = proof_obligation.inspection_generation
           AND proof.inspection_fingerprint = proof_obligation.inspection_fingerprint
           AND proof.proof_kind IN ('retired', 'absence_adopted'))
      )
)
BEGIN
    SELECT RAISE(ABORT, 'adopted absence requires exact retained proof');
END;

CREATE TRIGGER close_retirement_resources_reject_standalone_delete
BEFORE DELETE ON close_retirement_resources
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN product_conversations root ON root.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence can only be deleted with its root');
END;

CREATE TRIGGER close_retirement_resources_preserve_dependent_absence_on_delete
BEFORE DELETE ON close_retirement_resources
FOR EACH ROW
WHEN OLD.proof_kind IN ('retired', 'absence_adopted') AND EXISTS (
    SELECT 1
    FROM close_obligations proof_obligation
    JOIN product_conversations root ON root.id = proof_obligation.product_conversation_id
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.product_conversation_id = proof_obligation.product_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE proof_obligation.attempt_id = OLD.attempt_id
      AND OLD.inspection_generation = proof_obligation.inspection_generation
      AND OLD.inspection_fingerprint = proof_obligation.inspection_fingerprint
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = OLD.scope
      AND dependent.resource_kind = OLD.resource_kind
      AND dependent.identity_kind = OLD.identity_kind
      AND dependent.identity_codec = OLD.identity_codec
      AND dependent.identity_value = OLD.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'retained proof has dependent adopted absence');
END;

CREATE TRIGGER close_retirement_resources_preserve_dependent_absence_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN OLD.proof_kind IN ('retired', 'absence_adopted')
 AND (
     NEW.proof_kind NOT IN ('retired', 'absence_adopted')
     OR NEW.attempt_id <> OLD.attempt_id
     OR NEW.scope <> OLD.scope
     OR NEW.inspection_generation <> OLD.inspection_generation
     OR NEW.inspection_fingerprint <> OLD.inspection_fingerprint
     OR NEW.resource_kind <> OLD.resource_kind
     OR NEW.identity_kind <> OLD.identity_kind
     OR NEW.identity_codec <> OLD.identity_codec
     OR NEW.identity_value <> OLD.identity_value
 )
 AND EXISTS (
    SELECT 1
    FROM close_obligations proof_obligation
    JOIN product_conversations root ON root.id = proof_obligation.product_conversation_id
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.product_conversation_id = proof_obligation.product_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE proof_obligation.attempt_id = OLD.attempt_id
      AND OLD.inspection_generation = proof_obligation.inspection_generation
      AND OLD.inspection_fingerprint = proof_obligation.inspection_fingerprint
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = OLD.scope
      AND dependent.resource_kind = OLD.resource_kind
      AND dependent.identity_kind = OLD.identity_kind
      AND dependent.identity_codec = OLD.identity_codec
      AND dependent.identity_value = OLD.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'retained proof has dependent adopted absence');
END;

CREATE UNIQUE INDEX close_obligations_one_active_per_product ON close_obligations(product_conversation_id) WHERE phase <> 'completed';
";

const MIGRATION_075: &str = r"
CREATE TABLE close_attempt_direct_turn_settlement_captures (
    attempt_id TEXT PRIMARY KEY
        REFERENCES close_obligations(attempt_id) ON DELETE CASCADE,
    captured_at TEXT NOT NULL
);

CREATE TABLE close_attempt_direct_turn_settlements (
    attempt_id TEXT NOT NULL
        REFERENCES close_attempt_direct_turn_settlement_captures(attempt_id) ON DELETE CASCADE,
    turn_id INTEGER NOT NULL
        REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    expected_generation INTEGER NOT NULL
        CHECK (expected_generation >= 0),
    settled_at TEXT,
    PRIMARY KEY (attempt_id, turn_id)
);

CREATE INDEX idx_close_attempt_direct_turn_settlements_pending
    ON close_attempt_direct_turn_settlements(attempt_id, settled_at);

CREATE TRIGGER close_attempt_direct_turn_settlement_capture_requires_settlement_phase
BEFORE INSERT ON close_attempt_direct_turn_settlement_captures
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement')
)
BEGIN
    SELECT RAISE(ABORT, 'close direct-turn settlement capture requires settlement phase');
END;

CREATE TRIGGER close_attempt_direct_turn_settlement_target_requires_latest_member
BEFORE INSERT ON close_attempt_direct_turn_settlements
WHEN NOT EXISTS (
    SELECT 1
    FROM durable_turns turn
    JOIN close_attempt_members member ON member.conversation_id = turn.conversation_id
    WHERE member.attempt_id = NEW.attempt_id
      AND member.member_role IN ('latest', 'root_latest')
      AND turn.turn_id = NEW.turn_id
      AND turn.generation = NEW.expected_generation
      AND turn.owns_conversation = 1
      AND turn.terminal_kind IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'close direct-turn settlement target must be latest active authority');
END;

DROP TRIGGER close_obligations_transition_graph;

UPDATE close_obligations
SET phase = 'awaiting_stop_work_confirmation',
    updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE phase IN ('settling_active_work', 'cancel_requested_during_settlement');

CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;

CREATE TRIGGER close_attempt_direct_turn_settlements_immutable_identity
BEFORE UPDATE OF attempt_id, turn_id, expected_generation
ON close_attempt_direct_turn_settlements
BEGIN
    SELECT RAISE(ABORT, 'close direct-turn settlement target identity is immutable');
END;

CREATE TRIGGER close_attempt_direct_turn_settlements_settle_once
BEFORE UPDATE OF settled_at
ON close_attempt_direct_turn_settlements
WHEN OLD.settled_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'close direct-turn settlement receipt is immutable');
END;
";

const MIGRATION_074: &str = r"
CREATE TABLE completed_continuation_handoffs (
    predecessor_conversation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES conversations(id) ON DELETE CASCADE,
    successor_conversation_id TEXT UNIQUE NOT NULL
        REFERENCES conversations(id) ON DELETE CASCADE,
    continuation_message_id TEXT UNIQUE NOT NULL
        REFERENCES messages(message_id) ON DELETE RESTRICT,
    accepted_successor_message_id TEXT UNIQUE NOT NULL
        REFERENCES messages(message_id) ON DELETE RESTRICT
);

CREATE TRIGGER completed_continuation_handoffs_validate_insert
BEFORE INSERT ON completed_continuation_handoffs
FOR EACH ROW WHEN
    NEW.predecessor_conversation_id = NEW.successor_conversation_id
    OR NOT EXISTS (
        SELECT 1 FROM conversations predecessor
        WHERE predecessor.id = NEW.predecessor_conversation_id
          AND predecessor.continued_in_conv_id = NEW.successor_conversation_id
    )
    OR NOT EXISTS (
        SELECT 1 FROM messages continuation
        WHERE continuation.message_id = NEW.continuation_message_id
          AND continuation.conversation_id = NEW.predecessor_conversation_id
          AND continuation.message_type = 'continuation'
    )
    OR NOT EXISTS (
        SELECT 1 FROM messages accepted
        WHERE accepted.message_id = NEW.accepted_successor_message_id
          AND accepted.conversation_id = NEW.successor_conversation_id
    )
BEGIN
    SELECT RAISE(ABORT, 'completed continuation handoff relation mismatch');
END;

CREATE TRIGGER completed_continuation_handoffs_immutable
BEFORE UPDATE ON completed_continuation_handoffs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'completed continuation handoff is immutable');
END;

DROP TRIGGER consume_continuation_dispatch_intent;
CREATE TRIGGER consume_continuation_dispatch_intent
AFTER INSERT ON messages
WHEN EXISTS (
    SELECT 1 FROM continuation_dispatch_intents intent
    WHERE (NEW.message_id = intent.message_id
           OR NEW.message_id = intent.successor_conversation_id || ':' || intent.message_id)
      AND intent.successor_conversation_id = NEW.conversation_id
)
BEGIN
    INSERT INTO completed_continuation_handoffs (
        predecessor_conversation_id, successor_conversation_id,
        continuation_message_id, accepted_successor_message_id
    )
    SELECT intent.parent_conversation_id, intent.successor_conversation_id,
           continuation.message_id, NEW.message_id
    FROM continuation_dispatch_intents intent
    JOIN messages continuation
      ON continuation.conversation_id = intent.parent_conversation_id
     AND continuation.message_type = 'continuation'
    WHERE (NEW.message_id = intent.message_id
           OR NEW.message_id = intent.successor_conversation_id || ':' || intent.message_id)
      AND intent.successor_conversation_id = NEW.conversation_id
    ORDER BY continuation.sequence_id DESC, continuation.message_id DESC
    LIMIT 1;

    DELETE FROM continuation_dispatch_intents
    WHERE successor_conversation_id = NEW.conversation_id
      AND (message_id = NEW.message_id
           OR NEW.message_id = successor_conversation_id || ':' || message_id);
END;
";

const MIGRATION_073: &str = r"
DROP TRIGGER IF EXISTS conversations_validate_product_parent_on_insert;
DROP TRIGGER IF EXISTS conversations_validate_product_parent_on_update;
DROP TRIGGER IF EXISTS conversations_preserve_subordinate_parent_on_update;

CREATE TRIGGER conversations_validate_product_parent_on_insert
BEFORE INSERT ON conversations
FOR EACH ROW WHEN
    (NEW.runtime_role = 'sub_agent' AND (
        NEW.parent_conversation_id = NEW.id
        OR NOT EXISTS (
            SELECT 1 FROM conversations parent
            WHERE parent.id = NEW.parent_conversation_id
              AND parent.product_conversation_id = NEW.product_conversation_id
              AND parent.runtime_role IN ('user', 'coordinator')
        )
    ))
    OR (NEW.runtime_role <> 'sub_agent' AND NEW.parent_conversation_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'subordinate parent must be a same-ProductConversation user or coordinator');
END;

CREATE TRIGGER conversations_validate_product_parent_on_update
BEFORE UPDATE OF parent_conversation_id, product_conversation_id, runtime_role ON conversations
FOR EACH ROW WHEN
    (NEW.runtime_role = 'sub_agent' AND (
        NEW.parent_conversation_id = NEW.id
        OR NOT EXISTS (
            SELECT 1 FROM conversations parent
            WHERE parent.id = NEW.parent_conversation_id
              AND parent.product_conversation_id = NEW.product_conversation_id
              AND parent.runtime_role IN ('user', 'coordinator')
        )
    ))
    OR (NEW.runtime_role <> 'sub_agent' AND NEW.parent_conversation_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'subordinate parent must be a same-ProductConversation user or coordinator');
END;

CREATE TRIGGER conversations_preserve_subordinate_parent_on_update
BEFORE UPDATE OF product_conversation_id, runtime_role ON conversations
FOR EACH ROW WHEN EXISTS (
    SELECT 1 FROM conversations child
    WHERE child.parent_conversation_id = OLD.id
      AND child.runtime_role = 'sub_agent'
      AND (
          NEW.product_conversation_id <> child.product_conversation_id
          OR NEW.runtime_role NOT IN ('user', 'coordinator')
      )
)
BEGIN
    SELECT RAISE(ABORT, 'subordinate parent must remain a same-ProductConversation user or coordinator');
END;

WITH RECURSIVE ancestors(child_id, ancestor_id, depth, path) AS (
    SELECT child.id, child.parent_conversation_id, 1,
           ',' || hex(child.id) || ',' || hex(child.parent_conversation_id) || ','
    FROM conversations child
    WHERE child.runtime_role = 'sub_agent'
      AND child.parent_conversation_id IS NOT NULL
    UNION ALL
    SELECT ancestors.child_id, parent.parent_conversation_id, ancestors.depth + 1,
           ancestors.path || hex(parent.parent_conversation_id) || ','
    FROM ancestors
    JOIN conversations parent ON parent.id = ancestors.ancestor_id
    WHERE parent.runtime_role = 'sub_agent'
      AND parent.parent_conversation_id IS NOT NULL
      AND instr(ancestors.path, ',' || hex(parent.parent_conversation_id) || ',') = 0
), eligible_ancestors(child_id, ancestor_id, depth) AS (
    SELECT ancestors.child_id, ancestors.ancestor_id, ancestors.depth
    FROM ancestors
    JOIN conversations child ON child.id = ancestors.child_id
    JOIN conversations ancestor ON ancestor.id = ancestors.ancestor_id
    WHERE ancestor.product_conversation_id = child.product_conversation_id
      AND ancestor.runtime_role IN ('user', 'coordinator')
)
UPDATE conversations AS child
SET parent_conversation_id = (
    SELECT eligible_ancestors.ancestor_id
    FROM eligible_ancestors
    WHERE eligible_ancestors.child_id = child.id
    ORDER BY eligible_ancestors.depth
    LIMIT 1
)
WHERE child.runtime_role = 'sub_agent'
  AND NOT EXISTS (
      SELECT 1 FROM conversations parent
      WHERE parent.id = child.parent_conversation_id
        AND parent.product_conversation_id = child.product_conversation_id
        AND parent.runtime_role IN ('user', 'coordinator')
  )
  AND EXISTS (
      SELECT 1 FROM eligible_ancestors
      WHERE eligible_ancestors.child_id = child.id
  );

UPDATE conversations
SET runtime_role = runtime_role
WHERE runtime_role = 'sub_agent';
";

const MIGRATION_072: &str = r"
DROP TRIGGER IF EXISTS product_conversation_lifecycle_after_insert;
DROP TRIGGER IF EXISTS product_conversation_lifecycle_after_update;
DROP TRIGGER IF EXISTS product_conversation_lifecycle_after_delete;
";

const MIGRATION_071: &str = r"
UPDATE product_conversations
SET ordinary_lifecycle = CASE WHEN EXISTS (
    SELECT 1 FROM conversations latest
    WHERE latest.product_conversation_id = product_conversations.id
      AND latest.runtime_role = 'user'
      AND latest.parent_conversation_id IS NULL
      AND latest.continued_in_conv_id IS NULL
      AND latest.archived = 0
) THEN 'open' ELSE 'history' END
WHERE kind = 'ordinary';

CREATE TRIGGER product_conversation_id_before_insert
BEFORE INSERT ON product_conversations
WHEN trim(NEW.id, char(9) || char(10) || char(11) || char(12) || char(13) || ' ') = ''
BEGIN
    SELECT RAISE(ABORT, 'product conversation id must not be blank');
END;

CREATE TRIGGER product_conversation_id_before_update
BEFORE UPDATE OF id ON product_conversations
WHEN trim(NEW.id, char(9) || char(10) || char(11) || char(12) || char(13) || ' ') = ''
BEGIN
    SELECT RAISE(ABORT, 'product conversation id must not be blank');
END;

CREATE TRIGGER product_conversation_lifecycle_after_insert
AFTER INSERT ON conversations
WHEN NEW.product_conversation_id IS NOT NULL
BEGIN
    UPDATE product_conversations
    SET ordinary_lifecycle = CASE WHEN EXISTS (
        SELECT 1 FROM conversations latest
        WHERE latest.product_conversation_id = NEW.product_conversation_id
          AND latest.runtime_role = 'user'
          AND latest.parent_conversation_id IS NULL
          AND latest.continued_in_conv_id IS NULL
          AND latest.archived = 0
    ) THEN 'open' ELSE 'history' END
    WHERE id = NEW.product_conversation_id AND kind = 'ordinary';
END;

CREATE TRIGGER product_conversation_lifecycle_after_update
AFTER UPDATE OF archived, continued_in_conv_id, parent_conversation_id, runtime_role,
                product_conversation_id ON conversations
WHEN NEW.product_conversation_id IS NOT NULL OR OLD.product_conversation_id IS NOT NULL
BEGIN
    UPDATE product_conversations
    SET ordinary_lifecycle = CASE WHEN EXISTS (
        SELECT 1 FROM conversations latest
        WHERE latest.product_conversation_id = NEW.product_conversation_id
          AND latest.runtime_role = 'user'
          AND latest.parent_conversation_id IS NULL
          AND latest.continued_in_conv_id IS NULL
          AND latest.archived = 0
    ) THEN 'open' ELSE 'history' END
    WHERE id = NEW.product_conversation_id AND kind = 'ordinary';
    UPDATE product_conversations
    SET ordinary_lifecycle = CASE WHEN EXISTS (
        SELECT 1 FROM conversations latest
        WHERE latest.product_conversation_id = OLD.product_conversation_id
          AND latest.runtime_role = 'user'
          AND latest.parent_conversation_id IS NULL
          AND latest.continued_in_conv_id IS NULL
          AND latest.archived = 0
    ) THEN 'open' ELSE 'history' END
    WHERE id = OLD.product_conversation_id AND kind = 'ordinary';
END;

CREATE TRIGGER product_conversation_lifecycle_after_delete
AFTER DELETE ON conversations
WHEN OLD.product_conversation_id IS NOT NULL
BEGIN
    UPDATE product_conversations
    SET ordinary_lifecycle = CASE WHEN EXISTS (
        SELECT 1 FROM conversations latest
        WHERE latest.product_conversation_id = OLD.product_conversation_id
          AND latest.runtime_role = 'user'
          AND latest.parent_conversation_id IS NULL
          AND latest.continued_in_conv_id IS NULL
          AND latest.archived = 0
    ) THEN 'open' ELSE 'history' END
    WHERE id = OLD.product_conversation_id AND kind = 'ordinary';
END;
";

const MIGRATION_069: &str = "";

const MIGRATION_068: &str = "";

const MIGRATION_067: &str = r"
CREATE TABLE startup_parent_actions (
    action_id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL UNIQUE
        REFERENCES conversations(id) ON DELETE CASCADE,
    action TEXT NOT NULL
        CHECK (action IN ('Reconcile', 'Resume', 'Cancel')),
    transcript_generation INTEGER NOT NULL,
    turn_id INTEGER REFERENCES durable_turns(turn_id) ON DELETE SET NULL,
    turn_generation INTEGER,
    created_at TEXT NOT NULL
);
";

const MIGRATION_066: &str = r"
CREATE TABLE direct_turn_terminal_obligations (
    turn_id INTEGER NOT NULL PRIMARY KEY
        REFERENCES durable_turns(turn_id) ON DELETE CASCADE,
    expected_generation INTEGER NOT NULL
        CHECK (typeof(expected_generation) = 'integer' AND expected_generation >= 0),
    terminal_kind TEXT NOT NULL
        CHECK (typeof(terminal_kind) = 'text')
        CHECK (terminal_kind IN ('Completed', 'Cancelled', 'Failed')),
    terminal_reason TEXT,
    target_state TEXT NOT NULL
        CHECK (typeof(target_state) = 'text' AND json_valid(target_state)),
    target_state_updated_at_us INTEGER NOT NULL
        CHECK (typeof(target_state_updated_at_us) = 'integer' AND target_state_updated_at_us >= 0),
    response_message_id TEXT,
    CHECK (
        (terminal_kind = 'Failed' AND terminal_reason IS NOT NULL)
        OR (terminal_kind IN ('Completed', 'Cancelled') AND terminal_reason IS NULL)
    )
);

CREATE TABLE direct_turn_retirements (
    turn_id INTEGER NOT NULL PRIMARY KEY,
    conversation_id TEXT NOT NULL
);

INSERT INTO direct_turn_terminal_obligations (
    turn_id,
    expected_generation,
    terminal_kind,
    terminal_reason,
    target_state,
    target_state_updated_at_us,
    response_message_id
)
SELECT
    t.turn_id,
    t.generation,
    'Failed',
    'Phoenix restarted before this direct turn recorded an exact terminal result',
    json_object(
        'type', 'error',
        'message', 'Phoenix restarted before this direct turn recorded an exact terminal result',
        'error_kind', 'server_error'
    ),
    CAST(strftime(
        '%s',
        COALESCE(NULLIF(c.state_updated_at, ''), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) AS INTEGER) * 1000000,
    NULL
FROM durable_turns AS t
JOIN conversations AS c ON c.id = t.conversation_id
WHERE t.disposition = 'Runtime'
  AND t.terminal_kind IS NULL
  AND t.owns_conversation = 1
  AND t.canonical_message_id IS NOT NULL;
";

const MIGRATION_065: &str = r"
CREATE TABLE git_repositories (
    id TEXT NOT NULL PRIMARY KEY
        CHECK (typeof(id) = 'text' AND id <> '')
);

CREATE TABLE git_repository_locator_observations (
    repository_id TEXT NOT NULL REFERENCES git_repositories(id) ON DELETE CASCADE
        CHECK (typeof(repository_id) = 'text'),
    locator_kind TEXT NOT NULL
        CHECK (typeof(locator_kind) = 'text')
        CHECK (locator_kind IN ('common_dir', 'management_root')),
    status TEXT NOT NULL
        CHECK (typeof(status) = 'text')
        CHECK (status IN ('present', 'missing', 'inaccessible')),
    path TEXT NOT NULL
        CHECK (typeof(path) = 'text' AND path <> '' AND instr(path, char(0)) = 0),
    observed_at_unix_micros INTEGER NOT NULL
        CHECK (
            typeof(observed_at_unix_micros) = 'integer'
            AND observed_at_unix_micros >= 0
        ),
    PRIMARY KEY (repository_id, locator_kind)
);

CREATE TABLE git_repository_default_branch_observations (
    repository_id TEXT NOT NULL PRIMARY KEY REFERENCES git_repositories(id) ON DELETE CASCADE
        CHECK (typeof(repository_id) = 'text'),
    generation INTEGER NOT NULL
        CHECK (typeof(generation) = 'integer' AND generation > 0),
    status TEXT NOT NULL
        CHECK (typeof(status) = 'text')
        CHECK (status IN ('resolved', 'unresolved')),
    branch TEXT CHECK (
        branch IS NULL OR (typeof(branch) = 'text' AND instr(branch, char(0)) = 0)
    ),
    provenance TEXT CHECK (provenance IS NULL OR typeof(provenance) = 'text'),
    observed_at_unix_micros INTEGER NOT NULL
        CHECK (
            typeof(observed_at_unix_micros) = 'integer'
            AND observed_at_unix_micros >= 0
        ),
    CHECK (
        (status = 'resolved'
         AND branch IS NOT NULL AND branch <> ''
         AND provenance IS NOT NULL
         AND provenance IN ('remote_head_cache', 'local_checked_out_branch', 'user_selected'))
        OR (status = 'unresolved' AND branch IS NULL AND provenance IS NULL)
    )
);

CREATE TABLE work_scope_git_repositories (
    work_scope_id TEXT NOT NULL PRIMARY KEY REFERENCES work_scopes(id) ON DELETE CASCADE
        CONSTRAINT work_scope_git_repositories_work_scope_id_nonblank CHECK (
            typeof(work_scope_id) = 'text'
            AND length(trim(
                work_scope_id,
                char(9) || char(10) || char(11) || char(12) || char(13) || char(32)
                || char(133) || char(160) || char(5760)
                || char(8192) || char(8193) || char(8194) || char(8195) || char(8196)
                || char(8197) || char(8198) || char(8199) || char(8200) || char(8201)
                || char(8202) || char(8232) || char(8233) || char(8239) || char(8287)
                || char(12288)
            )) > 0
        ),
    repository_id TEXT NOT NULL REFERENCES git_repositories(id) ON DELETE RESTRICT
        CHECK (typeof(repository_id) = 'text')
);

INSERT INTO git_repositories (id)
SELECT id
FROM projects;

CREATE TEMP TABLE migration_065_scope_project_counts (
    work_scope_id TEXT NOT NULL PRIMARY KEY
        CHECK (typeof(work_scope_id) = 'text'),
    distinct_project_count INTEGER NOT NULL
        CHECK (typeof(distinct_project_count) = 'integer'),
    project_id TEXT CHECK (project_id IS NULL OR typeof(project_id) = 'text')
);

INSERT INTO migration_065_scope_project_counts (work_scope_id, distinct_project_count, project_id)
SELECT c.work_scope_id,
       COUNT(DISTINCT c.project_id) AS distinct_project_count,
       MIN(c.project_id) AS project_id
FROM conversations c
WHERE c.work_scope_id IS NOT NULL
  AND c.project_id IS NOT NULL
GROUP BY c.work_scope_id;

CREATE TEMP TABLE migration_065_scope_project_guard (
    invalid_count INTEGER NOT NULL
        CHECK (typeof(invalid_count) = 'integer' AND invalid_count = 0)
);

INSERT INTO migration_065_scope_project_guard (invalid_count)
SELECT COUNT(*)
FROM migration_065_scope_project_counts
WHERE distinct_project_count > 1;

INSERT INTO work_scope_git_repositories (work_scope_id, repository_id)
SELECT work_scope_id, project_id
FROM migration_065_scope_project_counts
WHERE distinct_project_count = 1;

DROP TABLE migration_065_scope_project_guard;
DROP TABLE migration_065_scope_project_counts;
";

const MIGRATION_063: &str = r"
ALTER TABLE conversations ADD COLUMN service_tier TEXT NOT NULL DEFAULT 'standard'
CHECK (service_tier IN ('standard', 'fast'));
";

const MIGRATION_062: &str = r"
CREATE TABLE IF NOT EXISTS steering_acceptance_receipts (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL,
    request_fingerprint TEXT,
    PRIMARY KEY (conversation_id, message_id),
    CHECK (request_fingerprint IS NULL OR request_fingerprint <> '')
);

INSERT OR IGNORE INTO steering_acceptance_receipts (
    conversation_id, message_id, request_fingerprint
)
SELECT conversation_id, message_id, NULL
FROM steering_messages;
";

const MIGRATION_064: &str = r"
ALTER TABLE work_scopes ADD COLUMN worktree_id TEXT;
ALTER TABLE work_scopes ADD COLUMN worktree_fingerprint TEXT;
CREATE UNIQUE INDEX work_scopes_unique_worktree_id
ON work_scopes(worktree_id) WHERE worktree_id IS NOT NULL;
CREATE UNIQUE INDEX work_scopes_unique_worktree_fingerprint
ON work_scopes(worktree_fingerprint) WHERE worktree_fingerprint IS NOT NULL;

DROP TRIGGER IF EXISTS conversations_role_scope_insert;
DROP TRIGGER IF EXISTS conversations_role_scope_update;
CREATE TRIGGER conversations_role_scope_insert
BEFORE INSERT ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR (NEW.runtime_role = 'user' AND NEW.work_scope_id IS NULL)
  OR (NEW.runtime_role = 'coordinator' AND NEW.work_scope_id IS NOT NULL)
  OR (NEW.coordinator_head = 1 AND NEW.runtime_role <> 'coordinator')
  OR (NEW.coordinator_head = 1 AND NEW.continued_in_conv_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;
CREATE TRIGGER conversations_role_scope_update
BEFORE UPDATE OF runtime_role, work_scope_id, coordinator_head, continued_in_conv_id ON conversations
WHEN NEW.runtime_role NOT IN ('user', 'sub_agent', 'coordinator')
  OR (NEW.runtime_role = 'user' AND NEW.work_scope_id IS NULL)
  OR (NEW.runtime_role = 'coordinator' AND NEW.work_scope_id IS NOT NULL)
  OR (NEW.coordinator_head = 1 AND NEW.runtime_role <> 'coordinator')
  OR (NEW.coordinator_head = 1 AND NEW.continued_in_conv_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'invalid conversation runtime role/work scope');
END;

CREATE TRIGGER work_scope_stable_worktree_shape_insert
BEFORE INSERT ON work_scopes
WHEN (NEW.environment_kind <> 'allocated_worktree'
      AND (NEW.worktree_id IS NOT NULL OR NEW.worktree_fingerprint IS NOT NULL))
  OR (NEW.environment_kind = 'allocated_worktree'
      AND NOT (
          (NEW.worktree_id IS NULL AND NEW.worktree_fingerprint IS NULL)
          OR (NEW.worktree_id IS NOT NULL AND NEW.worktree_id <> ''
              AND NEW.worktree_fingerprint IS NOT NULL AND NEW.worktree_fingerprint <> '')
      ))
BEGIN
    SELECT RAISE(ABORT, 'invalid stable worktree identity shape');
END;
CREATE TRIGGER work_scope_stable_worktree_shape_update
BEFORE UPDATE OF environment_kind, worktree_path, worktree_id, worktree_fingerprint ON work_scopes
WHEN (NEW.environment_kind <> 'allocated_worktree'
      AND (NEW.worktree_id IS NOT NULL OR NEW.worktree_fingerprint IS NOT NULL))
  OR (NEW.environment_kind = 'allocated_worktree'
      AND NOT (
          (NEW.worktree_id IS NULL AND NEW.worktree_fingerprint IS NULL)
          OR (NEW.worktree_id IS NOT NULL AND NEW.worktree_id <> ''
              AND NEW.worktree_fingerprint IS NOT NULL AND NEW.worktree_fingerprint <> '')
      ))
BEGIN
    SELECT RAISE(ABORT, 'invalid stable worktree identity shape');
END;

WITH RECURSIVE
chain_members(root_id, member_id, depth, path) AS (
     SELECT c.id, c.id, 0, json_array(c.id)
    FROM conversations c
    WHERE NOT EXISTS (
        SELECT 1 FROM conversations predecessor WHERE predecessor.continued_in_conv_id = c.id
    )
    UNION ALL
    SELECT chain_members.root_id,
           c.continued_in_conv_id,
           chain_members.depth + 1,
                   json_insert(chain_members.path, '$[#]', c.continued_in_conv_id)
    FROM chain_members
    JOIN conversations c ON c.id = chain_members.member_id
    WHERE c.continued_in_conv_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM json_each(chain_members.path) visited
          WHERE visited.value = c.continued_in_conv_id
      )
),
latest_member AS (
    SELECT root_id, member_id, archived
    FROM (
        SELECT
            member.root_id,
            member.member_id,
            c.archived,
            ROW_NUMBER() OVER (PARTITION BY member.root_id ORDER BY member.depth DESC, member.member_id DESC) AS rn
        FROM chain_members member
        JOIN conversations c ON c.id = member.member_id
        WHERE NOT EXISTS (
            SELECT 1 FROM conversation_creation_jobs job
            WHERE job.conversation_id = member.member_id
              AND job.status = 'deletion_pending'
        )
    )
    WHERE rn = 1
),
mixed_roots AS (
    SELECT latest.root_id, MAX(CASE WHEN c.archived THEN 1 ELSE 0 END) AS archived
    FROM latest_member latest
    JOIN chain_members member ON member.root_id = latest.root_id
    JOIN conversations c ON c.id = member.member_id
    GROUP BY latest.root_id
    HAVING COUNT(*) >= 2
       AND MIN(CASE WHEN c.archived THEN 1 ELSE 0 END)
           <> MAX(CASE WHEN c.archived THEN 1 ELSE 0 END)
)
UPDATE conversations
SET archived = (
    SELECT mixed_roots.archived
    FROM mixed_roots
    JOIN chain_members member ON member.root_id = mixed_roots.root_id
    WHERE member.member_id = conversations.id
)
WHERE id IN (
    SELECT member.member_id
    FROM chain_members member
    WHERE member.root_id IN (SELECT root_id FROM mixed_roots)
)
  AND NOT EXISTS (
      SELECT 1 FROM conversation_creation_jobs job
      WHERE job.conversation_id = conversations.id
        AND job.status = 'deletion_pending'
  );

CREATE TEMP TABLE migration_064_scope_fk_guard (
    missing_scope_rows INTEGER NOT NULL CHECK (missing_scope_rows = 0)
);
INSERT INTO migration_064_scope_fk_guard
SELECT COUNT(*)
FROM conversations c
WHERE c.work_scope_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM work_scopes scope WHERE scope.id = c.work_scope_id
  );

CREATE TEMP TABLE migration_064_continued_fk_guard (
    missing_continued_rows INTEGER NOT NULL CHECK (missing_continued_rows = 0)
);
INSERT INTO migration_064_continued_fk_guard
SELECT COUNT(*)
FROM conversations c
WHERE c.continued_in_conv_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM conversations next WHERE next.id = c.continued_in_conv_id
  );

DROP TABLE migration_064_scope_fk_guard;
DROP TABLE migration_064_continued_fk_guard;


CREATE TABLE close_obligations (
    chronology_ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id TEXT NOT NULL UNIQUE CHECK (attempt_id <> ''),
    root_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    phase TEXT NOT NULL CHECK (phase IN (
        'awaiting_blocker_resolution',
        'awaiting_stop_work_confirmation',
        'settling_active_work',
        'cancel_requested_during_settlement',
        'awaiting_retirement_inspection',
        'awaiting_loss_confirmation',
        'retirement_requested',
        'needs_repair',
        'completed'
    )),
    inspection_generation TEXT CHECK (inspection_generation IS NULL OR inspection_generation <> ''),
    inspection_fingerprint TEXT CHECK (inspection_fingerprint IS NULL OR inspection_fingerprint <> ''),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    close_outcome TEXT CHECK (close_outcome IS NULL OR close_outcome IN ('archived', 'cancelled')),
    topology_sealed INTEGER NOT NULL DEFAULT 0 CHECK (topology_sealed IN (0, 1)),
    CHECK ((inspection_generation IS NULL) = (inspection_fingerprint IS NULL)),
    CHECK (
        (
            phase IN (
                'awaiting_blocker_resolution',
                'awaiting_stop_work_confirmation',
                'settling_active_work',
                'cancel_requested_during_settlement'
            )
            AND inspection_generation IS NULL
        )
        OR phase = 'awaiting_retirement_inspection'
        OR (
                     phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair')
            AND inspection_generation IS NOT NULL
        )
        OR phase = 'completed'
    ),
    CHECK ((phase = 'completed') = (completed_at IS NOT NULL)),
    CHECK ((phase = 'completed') = (close_outcome IS NOT NULL))
);

CREATE TRIGGER close_obligations_require_admission_phase_on_insert
BEFORE INSERT ON close_obligations
FOR EACH ROW
WHEN NEW.phase <> 'awaiting_blocker_resolution'
  OR NEW.topology_sealed <> 0
  OR NEW.inspection_generation IS NOT NULL
  OR NEW.inspection_fingerprint IS NOT NULL
  OR NEW.completed_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'close obligation must begin at admission phase');
END;

CREATE TRIGGER close_obligations_reject_invalid_timestamps_on_insert
BEFORE INSERT ON close_obligations
FOR EACH ROW
WHEN (
      NEW.created_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.created_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 26) GLOB '*[^0-9]*')
  )
  OR (
      NEW.updated_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.updated_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.created_at, 1, 10), '+0 days') <> SUBSTR(NEW.created_at, 1, 10)
  OR CAST(SUBSTR(NEW.created_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.created_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.created_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.created_at) IS NULL
  OR date(SUBSTR(NEW.updated_at, 1, 10), '+0 days') <> SUBSTR(NEW.updated_at, 1, 10)
  OR CAST(SUBSTR(NEW.updated_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.updated_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.updated_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.updated_at) IS NULL
  OR (
      NEW.completed_at IS NOT NULL
      AND (
          (
              NEW.completed_at NOT GLOB '????-??-??T??:??:??Z'
              AND NEW.completed_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 21) GLOB '*[^0-9]*')
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 26) GLOB '*[^0-9]*')
          )
          OR date(SUBSTR(NEW.completed_at, 1, 10), '+0 days') <> SUBSTR(NEW.completed_at, 1, 10)
          OR CAST(SUBSTR(NEW.completed_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
          OR CAST(SUBSTR(NEW.completed_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR CAST(SUBSTR(NEW.completed_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR julianday(NEW.completed_at) IS NULL
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation timestamps must be valid RFC 3339');
END;

CREATE TRIGGER close_obligations_reject_invalid_timestamps_on_update
BEFORE UPDATE OF created_at, updated_at, completed_at ON close_obligations
FOR EACH ROW
WHEN (
      NEW.created_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.created_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 26) GLOB '*[^0-9]*')
  )
  OR (
      NEW.updated_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.updated_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.created_at, 1, 10), '+0 days') <> SUBSTR(NEW.created_at, 1, 10)
  OR CAST(SUBSTR(NEW.created_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.created_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.created_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.created_at) IS NULL
  OR date(SUBSTR(NEW.updated_at, 1, 10), '+0 days') <> SUBSTR(NEW.updated_at, 1, 10)
  OR CAST(SUBSTR(NEW.updated_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.updated_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.updated_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.updated_at) IS NULL
  OR (
      NEW.completed_at IS NOT NULL
      AND (
          (
              NEW.completed_at NOT GLOB '????-??-??T??:??:??Z'
              AND NEW.completed_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 21) GLOB '*[^0-9]*')
              AND (NEW.completed_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.completed_at, 21, LENGTH(NEW.completed_at) - 26) GLOB '*[^0-9]*')
          )
          OR date(SUBSTR(NEW.completed_at, 1, 10), '+0 days') <> SUBSTR(NEW.completed_at, 1, 10)
          OR CAST(SUBSTR(NEW.completed_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
          OR CAST(SUBSTR(NEW.completed_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR CAST(SUBSTR(NEW.completed_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
          OR julianday(NEW.completed_at) IS NULL
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation timestamps must be valid RFC 3339');
END;

CREATE TRIGGER close_obligations_reject_active_standalone_delete
BEFORE DELETE ON close_obligations
FOR EACH ROW
WHEN OLD.phase <> 'completed' AND EXISTS (
    SELECT 1 FROM conversations WHERE id = OLD.root_conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'active close obligation can only be deleted with its root');
END;

CREATE TRIGGER close_obligations_completed_outcome_is_immutable
BEFORE UPDATE OF phase, close_outcome, completed_at, updated_at ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'completed'
  AND (
      NEW.phase IS NOT OLD.phase
      OR NEW.close_outcome IS NOT OLD.close_outcome
      OR NEW.completed_at IS NOT OLD.completed_at
      OR NEW.updated_at IS NOT OLD.updated_at
  )
BEGIN
    SELECT RAISE(ABORT, 'completed close outcome is immutable');
END;

CREATE TRIGGER close_obligations_require_archived_members_for_completion
BEFORE UPDATE OF phase, close_outcome ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'completed'
  AND NEW.close_outcome = 'archived'
  AND EXISTS (
      SELECT 1 FROM close_attempt_members member
      JOIN conversations conversation ON conversation.id = member.conversation_id
      WHERE member.attempt_id = OLD.attempt_id AND conversation.archived <> 1
  )
BEGIN
    SELECT RAISE(ABORT, 'close completion requires archived captured members');
END;

CREATE TRIGGER close_obligations_require_open_members_for_cancelled_completion
BEFORE UPDATE OF phase, close_outcome ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'completed'
  AND NEW.close_outcome = 'cancelled'
  AND EXISTS (
      SELECT 1 FROM close_attempt_members member
      JOIN conversations conversation ON conversation.id = member.conversation_id
      WHERE member.attempt_id = OLD.attempt_id AND conversation.archived <> 0
  )
BEGIN
    SELECT RAISE(ABORT, 'cancelled close completion requires open captured members');
END;

CREATE TRIGGER close_obligations_cancelled_completion_clears_snapshot
BEFORE UPDATE OF phase, close_outcome, inspection_generation, inspection_fingerprint
ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'completed'
  AND NEW.close_outcome = 'cancelled'
  AND (NEW.inspection_generation IS NOT NULL OR NEW.inspection_fingerprint IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'cancelled Close completion must clear aggregate inspection snapshot');
END;

CREATE TRIGGER close_obligations_require_complete_retirement_proof
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase IN ('retirement_requested', 'needs_repair')
  AND NEW.phase = 'completed'
  AND (
      EXISTS (
          SELECT 1 FROM close_attempt_scopes target
          WHERE target.attempt_id = OLD.attempt_id
            AND NOT EXISTS (
                SELECT 1 FROM close_retirement_inventories inventory
                WHERE inventory.attempt_id = target.attempt_id
                  AND inventory.scope = target.scope
                  AND inventory.inspection_generation = OLD.inspection_generation
                  AND inventory.inspection_fingerprint = OLD.inspection_fingerprint
                  AND inventory.sealed = 1
            )
      )
      OR EXISTS (
          SELECT 1
          FROM close_attempt_scopes target
          JOIN work_scopes scope ON scope.id = target.scope
          WHERE target.attempt_id = OLD.attempt_id
            AND scope.environment_kind = 'allocated_worktree'
            AND NOT EXISTS (
                SELECT 1 FROM close_expected_retirement_resources expected
                WHERE expected.attempt_id = target.attempt_id
                  AND expected.scope = target.scope
                  AND expected.inspection_generation = OLD.inspection_generation
                  AND expected.inspection_fingerprint = OLD.inspection_fingerprint
                  AND expected.resource_kind = 'worktree'
            )
      )
      OR EXISTS (
          SELECT 1 FROM close_expected_retirement_resources expected
          WHERE expected.attempt_id = OLD.attempt_id
            AND expected.inspection_generation = OLD.inspection_generation
            AND expected.inspection_fingerprint = OLD.inspection_fingerprint
            AND NOT EXISTS (
                SELECT 1 FROM close_retirement_resources proof
                WHERE proof.attempt_id = expected.attempt_id
                  AND proof.scope = expected.scope
                  AND proof.inspection_generation = expected.inspection_generation
                  AND proof.inspection_fingerprint = expected.inspection_fingerprint
                  AND proof.resource_kind = expected.resource_kind
                  AND proof.identity_kind = expected.identity_kind
                  AND proof.identity_codec = expected.identity_codec
                  AND proof.identity_value = expected.identity_value
                  AND proof.proof_kind IN ('retired', 'absence_adopted')
            )
      )
      OR EXISTS (
          SELECT 1 FROM close_retirement_resources resource
          WHERE resource.attempt_id = OLD.attempt_id
            AND resource.inspection_generation = OLD.inspection_generation
            AND resource.inspection_fingerprint = OLD.inspection_fingerprint
            AND resource.proof_kind = 'residual'
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation lacks complete retirement proof');
END;

CREATE TRIGGER close_obligations_require_topology_seal_before_phase_transition
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase <> NEW.phase
  AND OLD.topology_sealed <> 1
BEGIN
    SELECT RAISE(ABORT, 'close obligation phase transition requires sealed topology');
END;

CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;

CREATE TRIGGER close_obligations_root_is_immutable
BEFORE UPDATE OF root_conversation_id ON close_obligations
FOR EACH ROW
WHEN OLD.root_conversation_id <> NEW.root_conversation_id
BEGIN
    SELECT RAISE(ABORT, 'close obligation root is immutable');
END;

CREATE TRIGGER close_obligations_created_at_is_immutable
BEFORE UPDATE OF created_at ON close_obligations
FOR EACH ROW
WHEN OLD.created_at <> NEW.created_at
BEGIN
    SELECT RAISE(ABORT, 'close obligation creation timestamp is immutable');
END;

CREATE TRIGGER close_obligations_chronology_ordinal_is_immutable
BEFORE UPDATE OF chronology_ordinal ON close_obligations
FOR EACH ROW
WHEN OLD.chronology_ordinal <> NEW.chronology_ordinal
BEGIN
    SELECT RAISE(ABORT, 'close obligation chronology ordinal is immutable');
END;

CREATE TRIGGER close_obligations_chronology_must_be_database_allocated
BEFORE INSERT ON close_obligations
FOR EACH ROW
WHEN NEW.chronology_ordinal <> -1
BEGIN
    SELECT RAISE(ABORT, 'close obligation chronology must be database allocated');
END;

CREATE TRIGGER close_obligations_require_closed_timestamps
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN ((NEW.phase = 'completed') <> (NEW.completed_at IS NOT NULL))
  OR ((NEW.phase = 'completed') <> (NEW.close_outcome IS NOT NULL))
  OR (OLD.phase <> 'completed' AND NEW.phase = 'completed'
      AND NEW.close_outcome = 'cancelled'
      AND OLD.phase IN ('retirement_requested', 'needs_repair'))
  OR (OLD.phase <> 'completed' AND NEW.phase = 'completed'
      AND NEW.close_outcome = 'archived'
      AND OLD.phase NOT IN ('retirement_requested', 'needs_repair'))
BEGIN
    SELECT RAISE(ABORT, 'close_obligations completion outcome must match legal source phase');
END;

CREATE TRIGGER close_obligations_reject_inspection_pair_mismatch_on_update
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN ((NEW.inspection_generation IS NULL) <> (NEW.inspection_fingerprint IS NULL))
BEGIN
    SELECT RAISE(ABORT, 'close_obligations inspection_generation/fingerprint must both be null or both nonnull');
END;

CREATE TRIGGER close_obligations_reject_missing_inspection_on_update
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN ((NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair')) AND NEW.inspection_generation IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'close_obligations inspection required for phase');
END;

CREATE TRIGGER close_obligations_require_complete_inspection_scope_coverage
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'awaiting_retirement_inspection'
  AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested')
  AND EXISTS (
      SELECT 1
      FROM close_attempt_scopes target
      JOIN work_scopes scope ON scope.id = target.scope
      WHERE target.attempt_id = NEW.attempt_id
        AND scope.environment_kind = 'allocated_worktree'
        AND NOT EXISTS (
            SELECT 1 FROM close_retirement_inspections inspection
            WHERE inspection.attempt_id = target.attempt_id
              AND inspection.scope = target.scope
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'close inspection must cover every targeted allocated worktree scope');
END;

CREATE TRIGGER close_obligations_require_loss_consistent_branch_from_inspection
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'awaiting_retirement_inspection'
  AND (
      (
          NEW.phase = 'retirement_requested'
          AND EXISTS (
              SELECT 1 FROM close_retirement_losses loss
              WHERE loss.attempt_id = NEW.attempt_id
          )
      )
      OR (
          NEW.phase = 'awaiting_loss_confirmation'
          AND NOT EXISTS (
              SELECT 1 FROM close_retirement_losses loss
              WHERE loss.attempt_id = NEW.attempt_id
          )
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'close obligation phase must match persisted inspection losses');
END;

CREATE TRIGGER close_obligations_invalidate_inspection_on_reentry
AFTER UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'awaiting_retirement_inspection'
  AND OLD.phase <> 'awaiting_retirement_inspection'
BEGIN
    DELETE FROM close_retirement_inspections WHERE attempt_id = NEW.attempt_id;
    UPDATE close_obligations
    SET inspection_generation = NULL, inspection_fingerprint = NULL
    WHERE attempt_id = NEW.attempt_id;
END;

CREATE TRIGGER close_obligations_snapshot_matches_inspection_aggregate
BEFORE UPDATE OF inspection_generation, inspection_fingerprint ON close_obligations
FOR EACH ROW
WHEN (
    OLD.inspection_generation IS NOT NEW.inspection_generation
    OR OLD.inspection_fingerprint IS NOT NEW.inspection_fingerprint
) AND NOT (
    NEW.phase = 'completed'
    AND NEW.close_outcome = 'cancelled'
    AND NEW.inspection_generation IS NULL
    AND NEW.inspection_fingerprint IS NULL
) AND (
    OLD.phase <> 'awaiting_retirement_inspection'
    OR NEW.inspection_generation <> CASE
        WHEN EXISTS (
            SELECT 1 FROM close_retirement_inspections
            WHERE attempt_id = NEW.attempt_id
        ) THEN (
            SELECT 'v1' || COALESCE(GROUP_CONCAT(component, ''), '')
            FROM (
                SELECT generation,
                       LENGTH(CAST(scope AS BLOB)) || ':' || scope ||
                       LENGTH(CAST(generation AS BLOB)) || ':' || generation AS component
                FROM close_retirement_inspections
                WHERE attempt_id = NEW.attempt_id
                ORDER BY scope
            )
        ) ELSE 'no-worktree'
    END
    OR NEW.inspection_fingerprint <> CASE
        WHEN EXISTS (
            SELECT 1 FROM close_retirement_inspections
            WHERE attempt_id = NEW.attempt_id
        ) THEN (
            SELECT 'v1' || COALESCE(GROUP_CONCAT(component, ''), '')
            FROM (
                SELECT fingerprint,
                       LENGTH(CAST(scope AS BLOB)) || ':' || scope ||
                       LENGTH(CAST(fingerprint AS BLOB)) || ':' || fingerprint AS component
                FROM close_retirement_inspections
                WHERE attempt_id = NEW.attempt_id
                ORDER BY scope
            )
        ) ELSE 'no-worktree'
    END
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation snapshot must match atomic inspection replacement');
END;

CREATE TRIGGER close_obligations_touch_updated_at
AFTER UPDATE ON close_obligations
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
  AND NEW.phase <> 'completed'
  AND julianday('now') > julianday(OLD.updated_at)
BEGIN
    UPDATE close_obligations
    SET updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE attempt_id = NEW.attempt_id;
END;
CREATE UNIQUE INDEX close_obligations_one_active_per_root
ON close_obligations(root_conversation_id)
WHERE phase <> 'completed';

CREATE TABLE close_attempt_members (
    attempt_id TEXT NOT NULL REFERENCES close_obligations(attempt_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    member_role TEXT NOT NULL CHECK (member_role IN ('root', 'intermediate', 'latest', 'root_latest')),
    continuation_ordinal INTEGER NOT NULL CHECK (continuation_ordinal >= 0),
    captured_continued_in_conv_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    captured_state_kind TEXT NOT NULL CHECK (captured_state_kind IN (
        'idle', 'llm_requesting', 'tool_executing', 'cancelling_tool',
        'awaiting_sub_agents', 'cancelling_sub_agents', 'error',
        'awaiting_continuation', 'recoverable_continuation_failure',
        'awaiting_recovery', 'awaiting_task_approval', 'awaiting_user_response',
        'awaiting_commission_review_approval', 'context_exhausted',
        'handed_off', 'terminal', 'completed', 'failed', 'provisioning',
        'creation_failed', 'creation_cancelled', 'seeded_llm_requesting'
    )),
    captured_runtime_role TEXT NOT NULL CHECK (captured_runtime_role IN ('user', 'sub_agent', 'coordinator')),
    captured_work_scope_id TEXT CHECK (captured_work_scope_id IS NULL OR captured_work_scope_id <> ''),
    captured_at TEXT NOT NULL,
    PRIMARY KEY (attempt_id, conversation_id)
);
CREATE UNIQUE INDEX close_attempt_members_one_ordinal_per_attempt
ON close_attempt_members(attempt_id, continuation_ordinal);
CREATE UNIQUE INDEX close_attempt_members_one_root_per_attempt
ON close_attempt_members(attempt_id)
WHERE member_role IN ('root', 'root_latest');
CREATE UNIQUE INDEX close_attempt_members_one_latest_per_attempt
ON close_attempt_members(attempt_id)
WHERE member_role IN ('latest', 'root_latest');

CREATE TRIGGER close_attempt_members_reject_invalid_timestamp
BEFORE INSERT ON close_attempt_members
FOR EACH ROW
WHEN (
      NEW.captured_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.captured_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.captured_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.captured_at, 21, LENGTH(NEW.captured_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.captured_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.captured_at, 21, LENGTH(NEW.captured_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.captured_at, 1, 10), '+0 days') <> SUBSTR(NEW.captured_at, 1, 10)
  OR CAST(SUBSTR(NEW.captured_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.captured_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.captured_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.captured_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'captured Close member timestamp must be valid RFC 3339');
END;

CREATE TABLE close_attempt_scopes (
    attempt_id TEXT NOT NULL REFERENCES close_obligations(attempt_id) ON DELETE CASCADE,
    scope TEXT NOT NULL REFERENCES work_scopes(id),
    captured_worktree_identity TEXT CHECK (
        captured_worktree_identity IS NULL OR captured_worktree_identity <> ''
    ),
    captured_worktree_fingerprint TEXT CHECK (
        captured_worktree_fingerprint IS NULL OR captured_worktree_fingerprint <> ''
    ),
    captured_worktree_locator TEXT CHECK (
        captured_worktree_locator IS NULL
        OR captured_worktree_locator GLOB 'git_path_bytes_hex_v1:*'
    ),
    captured_at TEXT NOT NULL,
    PRIMARY KEY (attempt_id, scope),
    CHECK (
        (captured_worktree_identity IS NULL
            AND captured_worktree_fingerprint IS NULL)
        OR (captured_worktree_identity IS NOT NULL
            AND captured_worktree_fingerprint IS NOT NULL
            AND captured_worktree_locator IS NOT NULL)
    )
);
CREATE TRIGGER close_attempt_scopes_reject_invalid_timestamp
BEFORE INSERT ON close_attempt_scopes
FOR EACH ROW
WHEN (
      NEW.captured_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.captured_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.captured_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.captured_at, 21, LENGTH(NEW.captured_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.captured_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.captured_at, 21, LENGTH(NEW.captured_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.captured_at, 1, 10), '+0 days') <> SUBSTR(NEW.captured_at, 1, 10)
  OR CAST(SUBSTR(NEW.captured_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.captured_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.captured_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.captured_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'captured Close scope timestamp must be valid RFC 3339');
END;

CREATE TRIGGER close_attempt_scopes_reject_insert_after_topology_seal
BEFORE INSERT ON close_attempt_scopes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id AND topology_sealed = 1
)
BEGIN
    SELECT RAISE(ABORT, 'captured close topology is sealed');
END;

CREATE TRIGGER close_attempt_scopes_require_captured_member_scope
BEFORE INSERT ON close_attempt_scopes
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_attempt_members member
    WHERE member.attempt_id = NEW.attempt_id
      AND member.captured_work_scope_id = NEW.scope
)
BEGIN
    SELECT RAISE(ABORT, 'close attempt scope must belong to a captured member');
END;

CREATE TRIGGER close_attempt_scopes_require_exact_environment_snapshot
BEFORE INSERT ON close_attempt_scopes
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_scopes scope
    WHERE scope.id = NEW.scope
      AND (
          (scope.environment_kind = 'allocated_worktree'
           AND (
               (scope.worktree_id IS NULL
                AND scope.worktree_fingerprint IS NULL
                AND NEW.captured_worktree_identity IS NULL
                AND NEW.captured_worktree_fingerprint IS NULL
                AND NEW.captured_worktree_locator =
                    'git_path_bytes_hex_v1:' || lower(hex(CAST(scope.worktree_path AS BLOB))))
               OR (NEW.captured_worktree_identity = scope.worktree_id
                   AND NEW.captured_worktree_fingerprint = scope.worktree_fingerprint
                   AND NEW.captured_worktree_locator =
                       'git_path_bytes_hex_v1:' || lower(hex(CAST(scope.worktree_path AS BLOB))))
           ))
          OR (scope.environment_kind <> 'allocated_worktree'
              AND NEW.captured_worktree_identity IS NULL
              AND NEW.captured_worktree_fingerprint IS NULL
              AND NEW.captured_worktree_locator IS NULL)
      )
)
BEGIN
    SELECT RAISE(ABORT, 'close attempt scope worktree snapshot must match environment kind');
END;

CREATE TRIGGER close_attempt_scopes_snapshot_is_immutable
BEFORE UPDATE OF attempt_id, scope, captured_worktree_identity, captured_worktree_fingerprint, captured_worktree_locator, captured_at ON close_attempt_scopes
FOR EACH ROW
WHEN OLD.attempt_id <> NEW.attempt_id
  OR OLD.scope <> NEW.scope
  OR OLD.captured_worktree_identity IS NOT NEW.captured_worktree_identity
  OR OLD.captured_worktree_fingerprint IS NOT NEW.captured_worktree_fingerprint
  OR OLD.captured_worktree_locator IS NOT NEW.captured_worktree_locator
  OR OLD.captured_at <> NEW.captured_at
BEGIN
    SELECT RAISE(ABORT, 'captured close scope snapshot is immutable');
END;
CREATE TRIGGER close_obligations_validate_topology_before_seal
BEFORE UPDATE OF topology_sealed ON close_obligations
FOR EACH ROW
WHEN OLD.topology_sealed = 0 AND NEW.topology_sealed = 1 AND (
    NOT EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.continuation_ordinal = 0
          AND member.conversation_id = OLD.root_conversation_id
          AND member.member_role IN ('root', 'root_latest')
    )
    OR NOT EXISTS (
        SELECT 1 FROM conversations root
        WHERE root.id = OLD.root_conversation_id
          AND root.runtime_role = 'user'
          AND root.parent_conversation_id IS NULL
          AND root.user_initiated = 1
          AND root.archived = 0
    )
    OR EXISTS (
        SELECT 1
        FROM close_attempt_members latest
        JOIN conversations live ON live.id = latest.conversation_id
        WHERE latest.attempt_id = OLD.attempt_id
          AND latest.member_role IN ('latest', 'root_latest')
          AND live.state_kind = 'handed_off'
          AND live.continued_in_conv_id IS NULL
    )
    OR EXISTS (
        SELECT 1
        FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND live.state_kind IN ('awaiting_task_approval', 'awaiting_continuation')
    )
    OR NOT EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.member_role IN ('latest', 'root_latest')
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members latest
        WHERE latest.attempt_id = OLD.attempt_id
          AND latest.member_role IN ('latest', 'root_latest')
          AND (
              latest.captured_continued_in_conv_id IS NOT NULL
              OR EXISTS (
                  SELECT 1 FROM close_attempt_members later
                  WHERE later.attempt_id = latest.attempt_id
                    AND later.continuation_ordinal > latest.continuation_ordinal
              )
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.continuation_ordinal > 0
          AND NOT EXISTS (
              SELECT 1 FROM close_attempt_members predecessor
              WHERE predecessor.attempt_id = member.attempt_id
                AND predecessor.continuation_ordinal = member.continuation_ordinal - 1
                AND predecessor.captured_continued_in_conv_id = member.conversation_id
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND (
              member.captured_continued_in_conv_id IS NOT live.continued_in_conv_id
              OR member.captured_state_kind <> live.state_kind
              OR member.captured_runtime_role <> live.runtime_role
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND live.archived <> 0
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND (
              (
                  member.continuation_ordinal = 0
                  AND (
                      SELECT COUNT(*) FROM conversations predecessor
                      WHERE predecessor.continued_in_conv_id = member.conversation_id
                  ) <> 0
              )
              OR (
                  member.continuation_ordinal > 0
                  AND (
                      SELECT COUNT(*) FROM conversations predecessor
                      WHERE predecessor.continued_in_conv_id = member.conversation_id
                  ) <> 1
              )
          )
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        JOIN conversations live ON live.id = member.conversation_id
        WHERE member.attempt_id = OLD.attempt_id
          AND member.captured_work_scope_id IS NOT live.work_scope_id
    )
    OR EXISTS (
        SELECT 1 FROM close_attempt_members member
        WHERE member.attempt_id = OLD.attempt_id
          AND member.captured_work_scope_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM close_attempt_scopes scope
              WHERE scope.attempt_id = member.attempt_id
                AND scope.scope = member.captured_work_scope_id
          )
    )
    OR EXISTS (
        SELECT 1
        FROM close_attempt_scopes captured
        JOIN work_scopes live ON live.id = captured.scope
        WHERE captured.attempt_id = OLD.attempt_id
          AND (
              (live.environment_kind = 'allocated_worktree'
               AND (captured.captured_worktree_identity IS NOT live.worktree_id
                   OR captured.captured_worktree_fingerprint IS NOT live.worktree_fingerprint
                   OR captured.captured_worktree_locator IS NOT
                       'git_path_bytes_hex_v1:' || lower(hex(CAST(live.worktree_path AS BLOB)))))
              OR (live.environment_kind <> 'allocated_worktree'
                  AND (captured.captured_worktree_identity IS NOT NULL
                      OR captured.captured_worktree_fingerprint IS NOT NULL
                      OR captured.captured_worktree_locator IS NOT NULL))
          )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'captured close topology is incomplete');
END;

CREATE TRIGGER close_obligations_topology_seal_is_monotonic
BEFORE UPDATE OF topology_sealed ON close_obligations
FOR EACH ROW
WHEN OLD.topology_sealed = 1 AND NEW.topology_sealed <> 1
BEGIN
    SELECT RAISE(ABORT, 'captured close topology seal is immutable');
END;

CREATE TRIGGER conversations_reject_close_root_identity_change
BEFORE UPDATE OF user_initiated, runtime_role, parent_conversation_id ON conversations
FOR EACH ROW
WHEN (
      OLD.user_initiated IS NOT NEW.user_initiated
      OR OLD.runtime_role IS NOT NEW.runtime_role
      OR OLD.parent_conversation_id IS NOT NEW.parent_conversation_id
  )
  AND EXISTS (
      SELECT 1 FROM close_obligations obligation
      WHERE obligation.root_conversation_id = OLD.id
        AND obligation.phase <> 'completed'
  )
BEGIN
    SELECT RAISE(ABORT, 'active Close preserves ProductConversation root identity');
END;

CREATE TRIGGER conversations_reject_continuation_change_during_close
BEFORE UPDATE OF continued_in_conv_id ON conversations
FOR EACH ROW
WHEN OLD.continued_in_conv_id IS NOT NEW.continued_in_conv_id
 AND EXISTS (
    SELECT 1
    FROM close_attempt_members member
    JOIN close_obligations obligation ON obligation.attempt_id = member.attempt_id
    WHERE obligation.phase <> 'completed'
      AND obligation.topology_sealed = 1
      AND (
          member.conversation_id = OLD.id
          OR member.conversation_id = OLD.continued_in_conv_id
          OR member.conversation_id = NEW.continued_in_conv_id
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'continuation topology is sealed by active Close');
END;

CREATE TRIGGER conversations_reject_continuation_insert_during_close
BEFORE INSERT ON conversations
FOR EACH ROW
WHEN NEW.continued_in_conv_id IS NOT NULL
 AND EXISTS (
    SELECT 1
    FROM close_attempt_members member
    JOIN close_obligations obligation ON obligation.attempt_id = member.attempt_id
    WHERE obligation.phase <> 'completed'
      AND obligation.topology_sealed = 1
      AND member.conversation_id = NEW.continued_in_conv_id
 )
BEGIN
    SELECT RAISE(ABORT, 'continuation topology is sealed by active Close');
END;

CREATE TRIGGER conversations_reject_work_scope_update_after_close_settlement
BEFORE UPDATE OF work_scope_id ON conversations
FOR EACH ROW
WHEN OLD.work_scope_id IS NOT NEW.work_scope_id
 AND EXISTS (
    SELECT 1
    FROM close_obligations obligation
    LEFT JOIN close_attempt_members member ON member.attempt_id = obligation.attempt_id
    LEFT JOIN close_attempt_scopes scope ON scope.attempt_id = obligation.attempt_id
    WHERE obligation.phase IN (
          'settling_active_work', 'cancel_requested_during_settlement',
          'awaiting_retirement_inspection', 'awaiting_loss_confirmation',
          'retirement_requested', 'needs_repair'
      )
      AND obligation.topology_sealed = 1
      AND (
          member.conversation_id = OLD.id
          OR scope.scope = OLD.work_scope_id
          OR scope.scope = NEW.work_scope_id
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'captured WorkScope attachment is sealed by active Close');
END;

CREATE TRIGGER conversations_reject_captured_work_scope_insert_after_close_settlement
BEFORE INSERT ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL
 AND EXISTS (
    SELECT 1
    FROM close_attempt_scopes scope
    JOIN close_obligations obligation ON obligation.attempt_id = scope.attempt_id
    WHERE obligation.phase IN (
          'settling_active_work', 'cancel_requested_during_settlement',
          'awaiting_retirement_inspection', 'awaiting_loss_confirmation',
          'retirement_requested', 'needs_repair'
      )
      AND obligation.topology_sealed = 1
      AND scope.scope = NEW.work_scope_id
 )
BEGIN
    SELECT RAISE(ABORT, 'captured WorkScope attachment is sealed by active Close');
END;

CREATE TRIGGER work_scopes_reject_environment_change_during_close
BEFORE UPDATE OF environment_kind, worktree_path, worktree_id, worktree_fingerprint ON work_scopes
FOR EACH ROW
WHEN (OLD.environment_kind IS NOT NEW.environment_kind
      OR OLD.worktree_path IS NOT NEW.worktree_path
      OR OLD.worktree_id IS NOT NEW.worktree_id
      OR OLD.worktree_fingerprint IS NOT NEW.worktree_fingerprint)
 AND EXISTS (
    SELECT 1 FROM close_attempt_scopes target
    JOIN close_obligations obligation ON obligation.attempt_id = target.attempt_id
    WHERE target.scope = OLD.id
      AND obligation.phase <> 'completed'
      AND obligation.topology_sealed = 1
 )
BEGIN
    SELECT RAISE(ABORT, 'captured WorkScope environment is sealed by active Close');
END;

CREATE TRIGGER close_obligations_require_member_cleanup_before_delete
BEFORE DELETE ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'completed'
  AND (
      EXISTS (
          SELECT 1 FROM close_attempt_members member
          WHERE member.attempt_id = OLD.attempt_id
      )
      OR EXISTS (
          SELECT 1 FROM close_attempt_scopes scope
          WHERE scope.attempt_id = OLD.attempt_id
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'completed Close history must remove member snapshots before obligation deletion');
END;

CREATE TRIGGER close_attempt_members_reject_insert_after_topology_seal
BEFORE INSERT ON close_attempt_members
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id AND topology_sealed = 1
)
BEGIN
    SELECT RAISE(ABORT, 'captured close topology is sealed');
END;

CREATE TRIGGER close_attempt_members_snapshot_is_immutable
BEFORE UPDATE OF attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                 captured_work_scope_id, captured_at
ON close_attempt_members
FOR EACH ROW
WHEN OLD.attempt_id <> NEW.attempt_id
  OR OLD.conversation_id <> NEW.conversation_id
  OR OLD.member_role <> NEW.member_role
  OR OLD.continuation_ordinal <> NEW.continuation_ordinal
  OR OLD.captured_continued_in_conv_id IS NOT NEW.captured_continued_in_conv_id
  OR OLD.captured_state_kind <> NEW.captured_state_kind
  OR OLD.captured_runtime_role <> NEW.captured_runtime_role
  OR OLD.captured_work_scope_id IS NOT NEW.captured_work_scope_id
  OR OLD.captured_at <> NEW.captured_at
BEGIN
    SELECT RAISE(ABORT, 'captured close member snapshot is immutable');
END;

CREATE TRIGGER close_attempt_members_preserve_target_scope_on_update
BEFORE UPDATE OF attempt_id, captured_work_scope_id ON close_attempt_members
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_attempt_scopes target
    WHERE target.attempt_id = OLD.attempt_id AND target.scope = OLD.captured_work_scope_id
) AND (NEW.attempt_id <> OLD.attempt_id OR NEW.captured_work_scope_id IS NOT OLD.captured_work_scope_id)
BEGIN
    SELECT RAISE(ABORT, 'captured member scope is targeted by close attempt');
END;

CREATE TRIGGER close_attempt_members_reject_delete_after_topology_seal
BEFORE DELETE ON close_attempt_members
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.topology_sealed = 1
      AND obligation.phase <> 'completed'
)
BEGIN
    SELECT RAISE(ABORT, 'captured close topology is sealed');
END;

CREATE TRIGGER close_attempt_members_preserve_target_scope_on_delete
BEFORE DELETE ON close_attempt_members
FOR EACH ROW
WHEN OLD.captured_work_scope_id IS NOT NULL AND EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'completed'
) AND EXISTS (
    SELECT 1 FROM close_attempt_scopes target
    WHERE target.attempt_id = OLD.attempt_id AND target.scope = OLD.captured_work_scope_id
) AND NOT EXISTS (
    SELECT 1 FROM close_attempt_members member
    WHERE member.attempt_id = OLD.attempt_id
      AND member.captured_work_scope_id = OLD.captured_work_scope_id
      AND member.conversation_id <> OLD.conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'captured member scope is targeted by close attempt');
END;

CREATE TRIGGER close_attempt_scopes_preserve_captured_target_on_delete
BEFORE DELETE ON close_attempt_scopes
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'completed'
)
BEGIN
    SELECT RAISE(ABORT, 'captured close target is immutable');
END;

CREATE TABLE close_retirement_inspections (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    generation TEXT NOT NULL CHECK (generation <> ''),
    fingerprint TEXT NOT NULL CHECK (fingerprint <> ''),
    inspected_at TEXT NOT NULL,
    PRIMARY KEY (attempt_id, scope),
    UNIQUE (attempt_id, scope, generation),
    FOREIGN KEY (attempt_id, scope)
        REFERENCES close_attempt_scopes(attempt_id, scope)
        ON DELETE CASCADE
);

CREATE TRIGGER close_retirement_inspections_require_targeted_allocated_scope
BEFORE INSERT ON close_retirement_inspections
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM close_attempt_scopes target
    JOIN close_obligations obligation ON obligation.attempt_id = target.attempt_id
    JOIN work_scopes scope ON scope.id = target.scope
    WHERE target.attempt_id = NEW.attempt_id
      AND target.scope = NEW.scope
      AND obligation.phase = 'awaiting_retirement_inspection'
      AND scope.environment_kind = 'allocated_worktree'
)
BEGIN
    SELECT RAISE(ABORT, 'close inspection scope must be a targeted allocated worktree');
END;

CREATE TRIGGER close_retirement_inspections_snapshot_is_immutable
BEFORE UPDATE OF attempt_id, scope, generation, fingerprint, inspected_at ON close_retirement_inspections
FOR EACH ROW
WHEN OLD.attempt_id <> NEW.attempt_id
  OR OLD.scope <> NEW.scope
  OR OLD.generation <> NEW.generation
  OR OLD.fingerprint <> NEW.fingerprint
  OR OLD.inspected_at <> NEW.inspected_at
BEGIN
    SELECT RAISE(ABORT, 'persisted close inspection snapshot is immutable');
END;

CREATE TRIGGER close_retirement_inspections_reject_invalid_timestamp
BEFORE INSERT ON close_retirement_inspections
FOR EACH ROW
WHEN (
      NEW.inspected_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.inspected_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.inspected_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.inspected_at, 21, LENGTH(NEW.inspected_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.inspected_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.inspected_at, 21, LENGTH(NEW.inspected_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.inspected_at, 1, 10), '+0 days') <> SUBSTR(NEW.inspected_at, 1, 10)
  OR CAST(SUBSTR(NEW.inspected_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.inspected_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.inspected_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.inspected_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'close inspection timestamp must be valid RFC 3339');
END;

CREATE TRIGGER close_retirement_inspections_reject_sealed_delete
BEFORE DELETE ON close_retirement_inspections
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'persisted close inspection snapshot is sealed');
END;

CREATE TABLE close_retirement_losses (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    generation TEXT NOT NULL,
    category TEXT NOT NULL CHECK (category IN (
        'staged_tracked_paths',
        'unstaged_tracked_paths',
        'untracked_non_ignored_paths',
        'initialized_submodule_state',
        'detached_unreachable_commits'
    )),
    identity_kind TEXT NOT NULL CHECK (identity_kind IN ('git_path', 'git_oid', 'opaque', 'worktree')),
    identity_codec TEXT NOT NULL CHECK (
        (identity_kind = 'git_path' AND identity_codec = 'git_path_bytes_hex_v1')
        OR (identity_kind = 'git_oid' AND identity_codec = 'hex')
        OR (identity_kind = 'opaque' AND identity_codec = 'opaque_string_v1')
        OR (identity_kind = 'worktree' AND identity_codec = 'worktree_id_v1')
    ),
    identity_value TEXT NOT NULL CHECK (
        identity_value <> ''
        AND (
            (
                identity_kind = 'git_path'
                AND SUBSTR(identity_value, 1, LENGTH('git_path_bytes_hex_v1:')) = 'git_path_bytes_hex_v1:'
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) > 0
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) % 2 = 0
                AND SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1) NOT GLOB '*[^0-9a-f]*'
            )
            OR (identity_kind = 'git_oid' AND LOWER(identity_value) GLOB REPLACE(HEX(ZEROBLOB(LENGTH(identity_value)/2)), '0', '[0-9a-f]'))
            OR (identity_kind = 'opaque' AND identity_value <> '')
            OR (identity_kind = 'worktree' AND identity_value <> '')
        )
    ),
    PRIMARY KEY (attempt_id, scope, generation, category, identity_kind, identity_value),
    FOREIGN KEY (attempt_id, scope, generation)
        REFERENCES close_retirement_inspections(attempt_id, scope, generation)
        ON DELETE CASCADE,
    CHECK (
        identity_kind <> 'git_oid'
        OR (LENGTH(identity_value) IN (40, 64) AND LOWER(identity_value) = identity_value)
    ),
    CHECK (
        (
            category IN (
                'staged_tracked_paths',
                'unstaged_tracked_paths',
                'untracked_non_ignored_paths',
                'initialized_submodule_state'
            )
            AND identity_kind = 'git_path'
        )
        OR (
            category = 'detached_unreachable_commits'
            AND identity_kind = 'git_oid'
        )
    )
);

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_insert
BEFORE INSERT ON close_retirement_losses
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id AND phase = 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_update
BEFORE UPDATE ON close_retirement_losses
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = OLD.attempt_id AND phase = 'awaiting_retirement_inspection'
) OR NOT EXISTS (
    SELECT 1 FROM close_obligations
    WHERE attempt_id = NEW.attempt_id AND phase = 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TRIGGER close_retirement_losses_require_open_inspection_on_delete
BEFORE DELETE ON close_retirement_losses
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
      AND obligation.phase <> 'awaiting_retirement_inspection'
)
BEGIN
    SELECT RAISE(ABORT, 'close loss inventory is sealed outside inspection replacement');
END;

CREATE TABLE close_retirement_inventories (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL CHECK (inspection_generation <> ''),
    inspection_fingerprint TEXT NOT NULL CHECK (inspection_fingerprint <> ''),
    sealed INTEGER NOT NULL DEFAULT 0 CHECK (sealed IN (0, 1)),
    captured_at TEXT NOT NULL,
    PRIMARY KEY (attempt_id, scope, inspection_generation, inspection_fingerprint),
    FOREIGN KEY (attempt_id, scope)
        REFERENCES close_attempt_scopes(attempt_id, scope)
        ON DELETE CASCADE
);

CREATE TRIGGER close_retirement_inventories_reject_invalid_timestamp
BEFORE INSERT ON close_retirement_inventories
FOR EACH ROW
WHEN (
      NEW.captured_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.captured_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.captured_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.captured_at, 21, LENGTH(NEW.captured_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.captured_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.captured_at, 21, LENGTH(NEW.captured_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.captured_at, 1, 10), '+0 days') <> SUBSTR(NEW.captured_at, 1, 10)
  OR CAST(SUBSTR(NEW.captured_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.captured_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.captured_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.captured_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Close inventory timestamp must be valid RFC 3339');
END;

CREATE TRIGGER close_retirement_inventories_require_exact_snapshot
BEFORE INSERT ON close_retirement_inventories
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('retirement_requested', 'needs_repair')
      AND obligation.inspection_generation = NEW.inspection_generation
      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
)
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory requires the exact active snapshot');
END;

CREATE TRIGGER close_retirement_inventories_reject_initial_seal
BEFORE INSERT ON close_retirement_inventories
FOR EACH ROW
WHEN NEW.sealed <> 0
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory must be populated before sealing');
END;

CREATE TRIGGER close_retirement_inventories_reject_distinct_owner_before_seal
BEFORE UPDATE OF sealed ON close_retirement_inventories
FOR EACH ROW
WHEN OLD.sealed = 0
  AND NEW.sealed = 1
  AND EXISTS (
      WITH RECURSIVE open_candidates(id) AS (
          SELECT id FROM conversations
          WHERE work_scope_id = NEW.scope
            AND runtime_role = 'user'
            AND parent_conversation_id IS NULL
            AND archived = 0
      ), ancestry(candidate_id, id, path) AS (
          SELECT id, id, json_array(id) FROM open_candidates
          UNION ALL
          SELECT ancestry.candidate_id, predecessor.id,
                 json_insert(ancestry.path, '$[#]', predecessor.id)
          FROM ancestry
          JOIN conversations predecessor
            ON predecessor.continued_in_conv_id = ancestry.id
          WHERE NOT EXISTS (
              SELECT 1 FROM json_each(ancestry.path) visited
              WHERE visited.value = predecessor.id
          )
      ), resolved(candidate_id, root_id) AS (
          SELECT ancestry.candidate_id, ancestry.id
          FROM ancestry
          WHERE NOT EXISTS (
              SELECT 1 FROM conversations predecessor
              WHERE predecessor.continued_in_conv_id = ancestry.id
          )
      ), captured_root(id) AS (
          SELECT root_conversation_id FROM close_obligations
          WHERE attempt_id = NEW.attempt_id
      )
      SELECT 1
      FROM open_candidates candidate
      LEFT JOIN resolved ON resolved.candidate_id = candidate.id
      CROSS JOIN captured_root
      GROUP BY candidate.id, captured_root.id
      HAVING COUNT(resolved.root_id) <> 1
          OR MAX(resolved.root_id) <> captured_root.id
  )
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory scope is retained by distinct open aggregate');
END;

CREATE TRIGGER close_retirement_inventories_require_allocated_worktree_before_seal
BEFORE UPDATE OF sealed ON close_retirement_inventories
FOR EACH ROW
WHEN OLD.sealed = 0
  AND NEW.sealed = 1
  AND EXISTS (
      SELECT 1
      FROM work_scopes scope
      WHERE scope.id = NEW.scope
        AND (
            (scope.environment_kind = 'allocated_worktree' AND (
            (SELECT COUNT(*) FROM close_expected_retirement_resources expected
             WHERE expected.attempt_id = NEW.attempt_id
               AND expected.scope = NEW.scope
               AND expected.inspection_generation = NEW.inspection_generation
               AND expected.inspection_fingerprint = NEW.inspection_fingerprint
               AND expected.resource_kind = 'worktree') <> 1
            OR
            (SELECT COUNT(*) FROM close_expected_retirement_resources expected
             WHERE expected.attempt_id = NEW.attempt_id
               AND expected.scope = NEW.scope
               AND expected.inspection_generation = NEW.inspection_generation
               AND expected.inspection_fingerprint = NEW.inspection_fingerprint
               AND expected.resource_kind = 'worktree'
               AND expected.identity_kind = 'worktree'
               AND expected.identity_codec = 'worktree_id_v1'
               AND expected.identity_value = (
                   SELECT target.captured_worktree_identity
                   FROM close_attempt_scopes target
                   WHERE target.attempt_id = NEW.attempt_id
                     AND target.scope = NEW.scope
               )) <> 1
            ))
            OR
            (scope.environment_kind <> 'allocated_worktree' AND EXISTS (
                SELECT 1 FROM close_expected_retirement_resources expected
                WHERE expected.attempt_id = NEW.attempt_id
                  AND expected.scope = NEW.scope
                  AND expected.inspection_generation = NEW.inspection_generation
                  AND expected.inspection_fingerprint = NEW.inspection_fingerprint
                  AND expected.resource_kind = 'worktree'
            ))
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory worktree rows must match the captured environment');
END;

CREATE TRIGGER close_retirement_inventories_are_immutable
BEFORE UPDATE ON close_retirement_inventories
FOR EACH ROW
WHEN OLD.attempt_id <> NEW.attempt_id
  OR OLD.scope <> NEW.scope
  OR OLD.inspection_generation <> NEW.inspection_generation
  OR OLD.inspection_fingerprint <> NEW.inspection_fingerprint
  OR OLD.captured_at <> NEW.captured_at
  OR OLD.sealed <> 0
  OR NEW.sealed <> 1
BEGIN
    SELECT RAISE(ABORT, 'captured retirement inventory is immutable');
END;

CREATE TRIGGER close_retirement_inventories_reject_standalone_delete
BEFORE DELETE ON close_retirement_inventories
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'captured retirement inventory can only be deleted with its root');
END;

CREATE TABLE close_expected_retirement_resources (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL CHECK (inspection_generation <> ''),
    inspection_fingerprint TEXT NOT NULL CHECK (inspection_fingerprint <> ''),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN (
        'worktree',
        'bash_process_group',
        'tmux_server',
        'pty_session',
        'browser_session',
        'equivalent_live_resource'
    )),
    identity_kind TEXT NOT NULL CHECK (identity_kind IN ('git_path', 'git_oid', 'opaque', 'worktree')),
    identity_codec TEXT NOT NULL CHECK (
        (identity_kind = 'git_path' AND identity_codec = 'git_path_bytes_hex_v1')
        OR (identity_kind = 'git_oid' AND identity_codec = 'hex')
        OR (identity_kind = 'opaque' AND identity_codec = 'opaque_string_v1')
        OR (identity_kind = 'worktree' AND identity_codec = 'worktree_id_v1')
    ),
    identity_value TEXT NOT NULL CHECK (
        identity_value <> ''
        AND (
            (
                identity_kind = 'git_path'
                AND SUBSTR(identity_value, 1, LENGTH('git_path_bytes_hex_v1:')) = 'git_path_bytes_hex_v1:'
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) > 0
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) % 2 = 0
                AND SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1) NOT GLOB '*[^0-9a-f]*'
            )
            OR (identity_kind = 'git_oid' AND LOWER(identity_value) GLOB REPLACE(HEX(ZEROBLOB(LENGTH(identity_value)/2)), '0', '[0-9a-f]'))
            OR (identity_kind = 'opaque' AND identity_value <> '')
            OR (identity_kind = 'worktree' AND identity_value <> '')
        )
    ),
    PRIMARY KEY (attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_value),
    FOREIGN KEY (attempt_id, scope, inspection_generation, inspection_fingerprint)
        REFERENCES close_retirement_inventories(attempt_id, scope, inspection_generation, inspection_fingerprint)
        ON DELETE CASCADE,
    CHECK (
        identity_kind <> 'git_oid'
        OR (LENGTH(identity_value) IN (40, 64) AND LOWER(identity_value) = identity_value)
    ),
    CHECK (
        (resource_kind = 'worktree' AND identity_kind = 'worktree')
        OR (resource_kind <> 'worktree' AND identity_kind = 'opaque')
    )
);

CREATE TRIGGER close_expected_retirement_resources_require_open_inventory
BEFORE INSERT ON close_expected_retirement_resources
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_retirement_inventories inventory
    WHERE inventory.attempt_id = NEW.attempt_id
      AND inventory.scope = NEW.scope
      AND inventory.inspection_generation = NEW.inspection_generation
      AND inventory.inspection_fingerprint = NEW.inspection_fingerprint
      AND inventory.sealed = 0
)
BEGIN
    SELECT RAISE(ABORT, 'expected retirement resource inventory is sealed');
END;

CREATE TRIGGER close_expected_retirement_resources_are_immutable
BEFORE UPDATE ON close_expected_retirement_resources
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'expected retirement resource is immutable');
END;

CREATE TRIGGER close_expected_retirement_resources_reject_standalone_delete
BEFORE DELETE ON close_expected_retirement_resources
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'expected retirement resource can only be deleted with its root');
END;

CREATE TABLE close_retirement_resources (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL DEFAULT 'legacy' CHECK (inspection_generation <> ''),
    inspection_fingerprint TEXT NOT NULL DEFAULT 'legacy' CHECK (inspection_fingerprint <> ''),
    resource_kind TEXT NOT NULL CHECK (resource_kind IN (
        'worktree',
        'bash_process_group',
        'tmux_server',
        'pty_session',
        'browser_session',
        'equivalent_live_resource'
    )),
    identity_kind TEXT NOT NULL CHECK (identity_kind IN ('git_path', 'git_oid', 'opaque', 'worktree')),
    identity_codec TEXT NOT NULL CHECK (
        (identity_kind = 'git_path' AND identity_codec = 'git_path_bytes_hex_v1')
        OR (identity_kind = 'git_oid' AND identity_codec = 'hex')
        OR (identity_kind = 'opaque' AND identity_codec = 'opaque_string_v1')
        OR (identity_kind = 'worktree' AND identity_codec = 'worktree_id_v1')
    ),
    identity_value TEXT NOT NULL CHECK (
        identity_value <> ''
        AND (
            (
                identity_kind = 'git_path'
                AND SUBSTR(identity_value, 1, LENGTH('git_path_bytes_hex_v1:')) = 'git_path_bytes_hex_v1:'
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) > 0
                AND LENGTH(SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1)) % 2 = 0
                AND SUBSTR(identity_value, LENGTH('git_path_bytes_hex_v1:') + 1) NOT GLOB '*[^0-9a-f]*'
            )
            OR (identity_kind = 'git_oid' AND LOWER(identity_value) GLOB REPLACE(HEX(ZEROBLOB(LENGTH(identity_value)/2)), '0', '[0-9a-f]'))
            OR (identity_kind = 'opaque' AND identity_value <> '')
            OR (identity_kind = 'worktree' AND identity_value <> '')
        )
    ),
    proof_kind TEXT NOT NULL CHECK (proof_kind IN ('retired', 'absence_adopted', 'residual')),
    absence_basis TEXT CHECK (absence_basis IS NULL OR absence_basis IN (
        'same_attempt_prior_retirement',
        'preexisting_exact_identity_evidence'
    )),
    residual_reason TEXT CHECK (residual_reason IS NULL OR residual_reason IN (
        'removal_failed',
        'still_shared_by_live_owner',
        'residual_process_alive',
        'identity_not_proven',
        'manual_repair_required'
    )),
    detail TEXT CHECK (detail IS NULL OR detail <> ''),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_value),
    FOREIGN KEY (attempt_id, scope)
        REFERENCES close_attempt_scopes(attempt_id, scope)
        ON DELETE CASCADE,
    CHECK (
        (proof_kind = 'retired' AND absence_basis IS NULL AND residual_reason IS NULL)
        OR (proof_kind = 'absence_adopted' AND absence_basis IS NOT NULL AND residual_reason IS NULL)
        OR (proof_kind = 'residual' AND absence_basis IS NULL AND residual_reason IS NOT NULL)
    ),
    CHECK (
        identity_kind <> 'git_oid'
        OR (LENGTH(identity_value) IN (40, 64) AND LOWER(identity_value) = identity_value)
    ),
    CHECK (
        (resource_kind = 'worktree' AND identity_kind = 'worktree')
        OR (resource_kind <> 'worktree' AND identity_kind = 'opaque')
    )
);

CREATE TRIGGER close_retirement_resources_reject_invalid_timestamps
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN (
      NEW.created_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.created_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.created_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.created_at, 21, LENGTH(NEW.created_at) - 26) GLOB '*[^0-9]*')
  )
  OR (
      NEW.updated_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.updated_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.created_at, 1, 10), '+0 days') <> SUBSTR(NEW.created_at, 1, 10)
  OR CAST(SUBSTR(NEW.created_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.created_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.created_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.created_at) IS NULL
  OR date(SUBSTR(NEW.updated_at, 1, 10), '+0 days') <> SUBSTR(NEW.updated_at, 1, 10)
  OR CAST(SUBSTR(NEW.updated_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.updated_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.updated_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.updated_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Close retirement evidence timestamps must be valid RFC 3339');
END;

CREATE TRIGGER close_retirement_resources_require_sealed_inventory_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM close_retirement_inventories inventory
    JOIN close_expected_retirement_resources expected
      ON expected.attempt_id = inventory.attempt_id
     AND expected.scope = inventory.scope
     AND expected.inspection_generation = inventory.inspection_generation
     AND expected.inspection_fingerprint = inventory.inspection_fingerprint
    WHERE inventory.attempt_id = NEW.attempt_id
      AND inventory.scope = NEW.scope
      AND inventory.inspection_generation = NEW.inspection_generation
      AND inventory.inspection_fingerprint = NEW.inspection_fingerprint
      AND inventory.sealed = 1
      AND expected.resource_kind = NEW.resource_kind
      AND expected.identity_kind = NEW.identity_kind
      AND expected.identity_codec = NEW.identity_codec
      AND expected.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence requires exact sealed inventory membership');
END;

CREATE TRIGGER close_retirement_resources_identity_is_immutable
BEFORE UPDATE OF attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
ON close_retirement_resources
FOR EACH ROW
WHEN OLD.attempt_id <> NEW.attempt_id
  OR OLD.scope <> NEW.scope
  OR OLD.inspection_generation <> NEW.inspection_generation
  OR OLD.inspection_fingerprint <> NEW.inspection_fingerprint
  OR OLD.resource_kind <> NEW.resource_kind
  OR OLD.identity_kind <> NEW.identity_kind
  OR OLD.identity_codec <> NEW.identity_codec
  OR OLD.identity_value <> NEW.identity_value
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence identity is immutable');
END;

CREATE TRIGGER close_retirement_resources_created_at_is_immutable
BEFORE UPDATE OF created_at ON close_retirement_resources
FOR EACH ROW
WHEN OLD.created_at <> NEW.created_at
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence creation timestamp is immutable');
END;

CREATE TRIGGER close_retirement_resources_reject_invalid_updated_at
BEFORE UPDATE OF updated_at ON close_retirement_resources
FOR EACH ROW
WHEN (
      NEW.updated_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.updated_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.updated_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.updated_at, 21, LENGTH(NEW.updated_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.updated_at, 1, 10), '+0 days') <> SUBSTR(NEW.updated_at, 1, 10)
  OR CAST(SUBSTR(NEW.updated_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.updated_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.updated_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.updated_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Close retirement evidence update timestamp must be valid RFC 3339');
END;

CREATE TRIGGER close_retirement_resources_outcome_is_monotonic
BEFORE UPDATE OF proof_kind, absence_basis, residual_reason, detail ON close_retirement_resources
FOR EACH ROW
WHEN OLD.proof_kind = 'retired' AND (
        NEW.proof_kind <> OLD.proof_kind
        OR NEW.absence_basis IS NOT OLD.absence_basis
        OR NEW.residual_reason IS NOT OLD.residual_reason
        OR NEW.detail IS NOT OLD.detail
    )
  OR OLD.proof_kind = 'absence_adopted' AND (
        NEW.proof_kind = 'residual'
        OR NEW.absence_basis IS NOT OLD.absence_basis
        OR NEW.detail IS NOT OLD.detail
    )
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence cannot be downgraded');
END;

CREATE TABLE close_retirement_resource_history (
    history_id INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    identity_kind TEXT NOT NULL,
    identity_codec TEXT NOT NULL CHECK (
        (identity_kind = 'git_path' AND identity_codec = 'git_path_bytes_hex_v1')
        OR (identity_kind = 'git_oid' AND identity_codec = 'hex')
        OR (identity_kind = 'opaque' AND identity_codec = 'opaque_string_v1')
        OR (identity_kind = 'worktree' AND identity_codec = 'worktree_id_v1')
    ),
    identity_value TEXT NOT NULL,
    proof_kind TEXT NOT NULL,
    absence_basis TEXT,
    residual_reason TEXT,
    detail TEXT,
    recorded_at TEXT NOT NULL,
    CHECK (proof_kind IN ('retired', 'absence_adopted', 'residual')),
    CHECK (
        (proof_kind = 'retired' AND absence_basis IS NULL AND residual_reason IS NULL)
        OR (proof_kind = 'absence_adopted'
            AND absence_basis IN ('same_attempt_prior_retirement', 'preexisting_exact_identity_evidence')
            AND residual_reason IS NULL)
        OR (proof_kind = 'residual'
            AND absence_basis IS NULL
            AND residual_reason IN (
                'removal_failed', 'still_shared_by_live_owner', 'residual_process_alive',
                'identity_not_proven', 'manual_repair_required'
            ))
    ),
    FOREIGN KEY (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ) REFERENCES close_expected_retirement_resources (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ) ON DELETE CASCADE
);
CREATE TRIGGER close_retirement_resource_history_reject_invalid_timestamp
BEFORE INSERT ON close_retirement_resource_history
FOR EACH ROW
WHEN (
      NEW.recorded_at NOT GLOB '????-??-??T??:??:??Z'
      AND NEW.recorded_at NOT GLOB '????-??-??T??:??:??[+-]??:??'
      AND (NEW.recorded_at NOT GLOB '????-??-??T??:??:??.*Z' OR SUBSTR(NEW.recorded_at, 21, LENGTH(NEW.recorded_at) - 21) GLOB '*[^0-9]*')
      AND (NEW.recorded_at NOT GLOB '????-??-??T??:??:??.*[+-]??:??' OR SUBSTR(NEW.recorded_at, 21, LENGTH(NEW.recorded_at) - 26) GLOB '*[^0-9]*')
  )
  OR date(SUBSTR(NEW.recorded_at, 1, 10), '+0 days') <> SUBSTR(NEW.recorded_at, 1, 10)
  OR CAST(SUBSTR(NEW.recorded_at, 12, 2) AS INTEGER) NOT BETWEEN 0 AND 23
  OR CAST(SUBSTR(NEW.recorded_at, 15, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR CAST(SUBSTR(NEW.recorded_at, 18, 2) AS INTEGER) NOT BETWEEN 0 AND 59
  OR julianday(NEW.recorded_at) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'close retirement resource history timestamp must be valid RFC 3339');
END;
CREATE UNIQUE INDEX close_retirement_resource_history_idempotent_replay
ON close_retirement_resource_history (
    attempt_id, scope, inspection_generation, inspection_fingerprint,
    resource_kind, identity_kind, identity_codec, identity_value, proof_kind,
    IFNULL(absence_basis, ''), IFNULL(residual_reason, ''), IFNULL(detail, '')
);
CREATE TRIGGER close_retirement_resource_history_reject_update
BEFORE UPDATE ON close_retirement_resource_history
BEGIN
    SELECT RAISE(ABORT, 'retirement resource history is append-only');
END;
CREATE TRIGGER close_retirement_resource_history_reject_delete
BEFORE DELETE ON close_retirement_resource_history
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'retirement resource history belongs to its Close aggregate');
END;
CREATE TRIGGER close_retirement_resources_allow_only_residual_resolution
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN NOT (
    OLD.proof_kind = 'residual'
    AND NEW.proof_kind IN ('retired', 'absence_adopted')
    AND OLD.attempt_id = NEW.attempt_id
    AND OLD.scope = NEW.scope
    AND OLD.inspection_generation = NEW.inspection_generation
    AND OLD.inspection_fingerprint = NEW.inspection_fingerprint
    AND OLD.resource_kind = NEW.resource_kind
    AND OLD.identity_kind = NEW.identity_kind
    AND OLD.identity_codec = NEW.identity_codec
    AND OLD.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence may only resolve residual proof');
END;

CREATE TRIGGER close_obligations_require_residual_before_needs_repair
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN OLD.phase = 'retirement_requested'
  AND NEW.phase = 'needs_repair'
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_resources resource
      WHERE resource.attempt_id = OLD.attempt_id
        AND resource.inspection_generation = OLD.inspection_generation
        AND resource.inspection_fingerprint = OLD.inspection_fingerprint
        AND resource.proof_kind = 'residual'
  )
  AND NOT EXISTS (
      SELECT 1
      FROM close_attempt_scopes captured
      WHERE captured.attempt_id = OLD.attempt_id
        AND captured.captured_worktree_identity IS NULL
        AND captured.captured_worktree_fingerprint IS NULL
        AND captured.captured_worktree_locator IS NOT NULL
  )
BEGIN
    SELECT RAISE(ABORT, 'needs_repair requires current-snapshot residual evidence');
END;

CREATE TRIGGER close_retirement_resources_require_authority_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('retirement_requested', 'needs_repair')
      AND obligation.inspection_generation = NEW.inspection_generation
      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence requires authorized phase and snapshot');
END;

CREATE TRIGGER close_retirement_resources_require_authority_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('retirement_requested', 'needs_repair')
      AND obligation.inspection_generation = NEW.inspection_generation
      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence requires authorized phase and snapshot');
END;

CREATE TRIGGER close_retirement_resources_require_absence_proof_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NEW.proof_kind = 'absence_adopted' AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    JOIN close_obligations proof_obligation ON proof_obligation.attempt_id = proof.attempt_id
    JOIN close_obligations current_obligation ON current_obligation.attempt_id = NEW.attempt_id
    WHERE proof.scope = NEW.scope
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND (
          (NEW.absence_basis = 'same_attempt_prior_retirement'
           AND proof.attempt_id = NEW.attempt_id
           AND proof.inspection_generation = NEW.inspection_generation
           AND proof.inspection_fingerprint = NEW.inspection_fingerprint
           AND proof.proof_kind = 'retired')
          OR
          (NEW.absence_basis = 'preexisting_exact_identity_evidence'
           AND proof.attempt_id <> NEW.attempt_id
           AND proof_obligation.root_conversation_id = current_obligation.root_conversation_id
           AND proof.inspection_generation = proof_obligation.inspection_generation
           AND proof.inspection_fingerprint = proof_obligation.inspection_fingerprint
           AND proof.proof_kind IN ('retired', 'absence_adopted'))
      )
)
BEGIN
    SELECT RAISE(ABORT, 'adopted absence requires exact retained proof');
END;

CREATE TRIGGER close_retirement_resources_require_absence_proof_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN NEW.proof_kind = 'absence_adopted'
 AND (
     OLD.proof_kind <> 'absence_adopted'
     OR OLD.absence_basis IS NOT NEW.absence_basis
     OR OLD.attempt_id <> NEW.attempt_id
     OR OLD.scope <> NEW.scope
     OR OLD.inspection_generation <> NEW.inspection_generation
     OR OLD.inspection_fingerprint <> NEW.inspection_fingerprint
     OR OLD.resource_kind <> NEW.resource_kind
     OR OLD.identity_kind <> NEW.identity_kind
     OR OLD.identity_codec <> NEW.identity_codec
     OR OLD.identity_value <> NEW.identity_value
 )
 AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    JOIN close_obligations proof_obligation ON proof_obligation.attempt_id = proof.attempt_id
    JOIN close_obligations current_obligation ON current_obligation.attempt_id = NEW.attempt_id
    WHERE proof.scope = NEW.scope
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND (
          (NEW.absence_basis = 'same_attempt_prior_retirement'
           AND proof.attempt_id = NEW.attempt_id
           AND proof.inspection_generation = NEW.inspection_generation
           AND proof.inspection_fingerprint = NEW.inspection_fingerprint
           AND proof.proof_kind = 'retired')
          OR
          (NEW.absence_basis = 'preexisting_exact_identity_evidence'
           AND proof.attempt_id <> NEW.attempt_id
           AND proof_obligation.root_conversation_id = current_obligation.root_conversation_id
           AND proof.inspection_generation = proof_obligation.inspection_generation
           AND proof.inspection_fingerprint = proof_obligation.inspection_fingerprint
           AND proof.proof_kind IN ('retired', 'absence_adopted'))
      )
)
BEGIN
    SELECT RAISE(ABORT, 'adopted absence requires exact retained proof');
END;

CREATE TRIGGER close_obligations_preserve_dependent_absence_on_delete
BEFORE DELETE ON close_obligations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM conversations root
    JOIN close_retirement_resources proof ON proof.attempt_id = OLD.attempt_id
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.root_conversation_id = OLD.root_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE root.id = OLD.root_conversation_id
      AND proof.inspection_generation = OLD.inspection_generation
      AND proof.inspection_fingerprint = OLD.inspection_fingerprint
      AND proof.proof_kind IN ('retired', 'absence_adopted')
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = proof.scope
      AND dependent.resource_kind = proof.resource_kind
      AND dependent.identity_kind = proof.identity_kind
      AND dependent.identity_codec = proof.identity_codec
      AND dependent.identity_value = proof.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation retains proof for adopted absence');
END;

CREATE TRIGGER close_obligations_preserve_dependent_absence_on_snapshot_update
BEFORE UPDATE OF inspection_generation, inspection_fingerprint ON close_obligations
FOR EACH ROW
WHEN (
    OLD.inspection_generation IS NOT NEW.inspection_generation
    OR OLD.inspection_fingerprint IS NOT NEW.inspection_fingerprint
) AND EXISTS (
    SELECT 1
    FROM close_retirement_resources proof
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.root_conversation_id = OLD.root_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE proof.attempt_id = OLD.attempt_id
      AND proof.inspection_generation = OLD.inspection_generation
      AND proof.inspection_fingerprint = OLD.inspection_fingerprint
      AND proof.proof_kind IN ('retired', 'absence_adopted')
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = proof.scope
      AND dependent.resource_kind = proof.resource_kind
      AND dependent.identity_kind = proof.identity_kind
      AND dependent.identity_codec = proof.identity_codec
      AND dependent.identity_value = proof.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation retains proof for adopted absence');
END;

CREATE TRIGGER close_retirement_resources_reject_standalone_delete
BEFORE DELETE ON close_retirement_resources
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN conversations root ON root.id = obligation.root_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence can only be deleted with its root');
END;

CREATE TRIGGER close_retirement_resources_preserve_dependent_absence_on_delete
BEFORE DELETE ON close_retirement_resources
FOR EACH ROW
WHEN OLD.proof_kind IN ('retired', 'absence_adopted') AND EXISTS (
    SELECT 1
    FROM close_obligations proof_obligation
    JOIN conversations root ON root.id = proof_obligation.root_conversation_id
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.root_conversation_id = proof_obligation.root_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE proof_obligation.attempt_id = OLD.attempt_id
      AND OLD.inspection_generation = proof_obligation.inspection_generation
      AND OLD.inspection_fingerprint = proof_obligation.inspection_fingerprint
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = OLD.scope
      AND dependent.resource_kind = OLD.resource_kind
      AND dependent.identity_kind = OLD.identity_kind
      AND dependent.identity_codec = OLD.identity_codec
      AND dependent.identity_value = OLD.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'retained proof has dependent adopted absence');
END;

CREATE TRIGGER close_retirement_resources_preserve_dependent_absence_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN OLD.proof_kind IN ('retired', 'absence_adopted')
 AND (
     NEW.proof_kind NOT IN ('retired', 'absence_adopted')
     OR NEW.attempt_id <> OLD.attempt_id
     OR NEW.scope <> OLD.scope
     OR NEW.inspection_generation <> OLD.inspection_generation
     OR NEW.inspection_fingerprint <> OLD.inspection_fingerprint
     OR NEW.resource_kind <> OLD.resource_kind
     OR NEW.identity_kind <> OLD.identity_kind
     OR NEW.identity_codec <> OLD.identity_codec
     OR NEW.identity_value <> OLD.identity_value
 )
 AND EXISTS (
    SELECT 1
    FROM close_obligations proof_obligation
    JOIN conversations root ON root.id = proof_obligation.root_conversation_id
    JOIN close_obligations dependent_obligation
      ON dependent_obligation.root_conversation_id = proof_obligation.root_conversation_id
    JOIN close_retirement_resources dependent
      ON dependent.attempt_id = dependent_obligation.attempt_id
    WHERE proof_obligation.attempt_id = OLD.attempt_id
      AND OLD.inspection_generation = proof_obligation.inspection_generation
      AND OLD.inspection_fingerprint = proof_obligation.inspection_fingerprint
      AND dependent.attempt_id <> OLD.attempt_id
      AND dependent.scope = OLD.scope
      AND dependent.resource_kind = OLD.resource_kind
      AND dependent.identity_kind = OLD.identity_kind
      AND dependent.identity_codec = OLD.identity_codec
      AND dependent.identity_value = OLD.identity_value
      AND dependent.proof_kind = 'absence_adopted'
      AND dependent.absence_basis = 'preexisting_exact_identity_evidence'
)
BEGIN
    SELECT RAISE(ABORT, 'retained proof has dependent adopted absence');
END;

CREATE TRIGGER conversations_reject_member_archival_while_close_is_cancellable
BEFORE UPDATE OF archived ON conversations
FOR EACH ROW
WHEN OLD.archived = 0
  AND NEW.archived = 1
  AND EXISTS (
      SELECT 1
      FROM close_attempt_members member
      JOIN close_obligations obligation ON obligation.attempt_id = member.attempt_id
      WHERE member.conversation_id = OLD.id
        AND obligation.phase <> 'completed'
        AND (
            obligation.phase NOT IN ('retirement_requested', 'needs_repair')
            OR EXISTS (
                SELECT 1 FROM close_attempt_scopes target
                WHERE target.attempt_id = obligation.attempt_id
                  AND NOT EXISTS (
                      SELECT 1 FROM close_retirement_inventories inventory
                      WHERE inventory.attempt_id = target.attempt_id
                        AND inventory.scope = target.scope
                        AND inventory.inspection_generation = obligation.inspection_generation
                        AND inventory.inspection_fingerprint = obligation.inspection_fingerprint
                        AND inventory.sealed = 1
                  )
            )
            OR EXISTS (
                SELECT 1 FROM close_expected_retirement_resources expected
                WHERE expected.attempt_id = obligation.attempt_id
                  AND expected.inspection_generation = obligation.inspection_generation
                  AND expected.inspection_fingerprint = obligation.inspection_fingerprint
                  AND NOT EXISTS (
                      SELECT 1 FROM close_retirement_resources proof
                      WHERE proof.attempt_id = expected.attempt_id
                        AND proof.scope = expected.scope
                        AND proof.inspection_generation = expected.inspection_generation
                        AND proof.inspection_fingerprint = expected.inspection_fingerprint
                        AND proof.resource_kind = expected.resource_kind
                        AND proof.identity_kind = expected.identity_kind
                        AND proof.identity_codec = expected.identity_codec
                        AND proof.identity_value = expected.identity_value
                        AND proof.proof_kind IN ('retired', 'absence_adopted')
                  )
            )
            OR EXISTS (
                SELECT 1 FROM close_retirement_resources resource
                WHERE resource.attempt_id = obligation.attempt_id
                  AND resource.inspection_generation = obligation.inspection_generation
                  AND resource.inspection_fingerprint = obligation.inspection_fingerprint
                  AND resource.proof_kind = 'residual'
            )
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'active Close preserves open captured members until retirement proof is complete');
END;

CREATE TRIGGER conversations_reject_owner_reactivation_during_close
BEFORE UPDATE OF archived, parent_conversation_id, runtime_role, work_scope_id ON conversations
FOR EACH ROW
WHEN NEW.archived = 0
  AND NEW.parent_conversation_id IS NULL
  AND NEW.runtime_role = 'user'
  AND NEW.work_scope_id IS NOT NULL
  AND (
      OLD.archived <> 0
      OR OLD.parent_conversation_id IS NOT NULL
      OR OLD.runtime_role IS NOT 'user'
      OR OLD.work_scope_id IS NOT NEW.work_scope_id
  )
  AND EXISTS (
      SELECT 1
      FROM close_attempt_scopes scope
      JOIN close_obligations obligation ON obligation.attempt_id = scope.attempt_id
      WHERE scope.scope = NEW.work_scope_id
        AND obligation.phase <> 'completed'
  )
BEGIN
    SELECT RAISE(ABORT, 'active Close prevents WorkScope owner reactivation');
END;

CREATE TRIGGER close_attempt_scopes_reject_nul_git_path
BEFORE INSERT ON close_attempt_scopes
FOR EACH ROW
WHEN NEW.captured_worktree_locator IS NOT NULL
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.captured_worktree_locator)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.captured_worktree_locator, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

CREATE TRIGGER close_retirement_losses_reject_nul_git_path
BEFORE INSERT ON close_retirement_losses
FOR EACH ROW
WHEN NEW.identity_kind = 'git_path'
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.identity_value)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.identity_value, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

CREATE TRIGGER close_retirement_losses_reject_nul_git_path_on_update
BEFORE UPDATE OF identity_kind, identity_value ON close_retirement_losses
FOR EACH ROW
WHEN NEW.identity_kind = 'git_path'
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.identity_value)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.identity_value, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

CREATE TRIGGER close_expected_retirement_resources_reject_nul_git_path
BEFORE INSERT ON close_expected_retirement_resources
FOR EACH ROW
WHEN NEW.identity_kind = 'git_path'
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.identity_value)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.identity_value, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

CREATE TRIGGER close_retirement_resources_reject_nul_git_path
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NEW.identity_kind = 'git_path'
  AND EXISTS (
      WITH RECURSIVE byte_pos(pos) AS (
          SELECT LENGTH('git_path_bytes_hex_v1:') + 1
          UNION ALL
          SELECT pos + 2 FROM byte_pos
          WHERE pos + 2 <= LENGTH(NEW.identity_value)
      )
      SELECT 1 FROM byte_pos
      WHERE SUBSTR(NEW.identity_value, pos, 2) = '00'
  )
BEGIN
    SELECT RAISE(ABORT, 'Git path identity cannot contain a NUL byte');
END;

";

const MIGRATION_061: &str = r"
CREATE VIEW IF NOT EXISTS conversation_work_scope_attachments AS
SELECT id AS conversation_id, work_scope_id
FROM conversations
WHERE work_scope_id IS NOT NULL;
";

const MIGRATION_058: &str = r"
ALTER TABLE conversations ADD COLUMN effort TEXT
CHECK (effort IS NULL OR effort IN ('none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'));
ALTER TABLE turn_usage RENAME TO turn_usage_legacy_effort;
CREATE TABLE turn_usage (
    id INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    root_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    first_byte_at TEXT,
    reasoning_tokens INTEGER,
    effort_source TEXT NOT NULL
        CHECK (effort_source IN ('native_known', 'native_unknown', 'explicit', 'unsupported')),
    effort_level TEXT
        CHECK (effort_level IS NULL OR effort_level IN ('none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'))
);
INSERT INTO turn_usage (
    id, conversation_id, root_conversation_id, model,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    created_at, first_byte_at, reasoning_tokens, effort_source, effort_level
)
SELECT
    id, conversation_id, root_conversation_id, model,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    created_at, first_byte_at, NULL, 'native_unknown', NULL
FROM turn_usage_legacy_effort;
DROP TABLE turn_usage_legacy_effort;
CREATE INDEX idx_turn_usage_conversation ON turn_usage(conversation_id);
CREATE INDEX idx_turn_usage_root ON turn_usage(root_conversation_id);
CREATE TRIGGER turn_usage_effort_shape_insert
BEFORE INSERT ON turn_usage
WHEN ((NEW.effort_source IN ('explicit', 'native_known')) != (NEW.effort_level IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'invalid turn usage effort shape');
END;
CREATE TRIGGER turn_usage_effort_shape_update
BEFORE UPDATE OF effort_source, effort_level ON turn_usage
WHEN ((NEW.effort_source IN ('explicit', 'native_known')) != (NEW.effort_level IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'invalid turn usage effort shape');
END;
";

const MIGRATION_059: &str = r"
ALTER TABLE conversations ADD COLUMN state_kind TEXT NOT NULL DEFAULT 'idle'
    CHECK (state_kind IN (
        'idle', 'llm_requesting', 'tool_executing', 'cancelling_tool',
        'awaiting_sub_agents', 'cancelling_sub_agents', 'error',
        'awaiting_continuation', 'recoverable_continuation_failure',
        'awaiting_recovery', 'awaiting_task_approval', 'awaiting_user_response',
        'awaiting_commission_review_approval', 'context_exhausted',
        'handed_off', 'terminal', 'completed', 'failed', 'provisioning',
        'creation_failed', 'creation_cancelled', 'seeded_llm_requesting'
    ));

UPDATE conversations
SET state_kind = json_extract(state, '$.type');

CREATE INDEX IF NOT EXISTS idx_conversations_state_kind ON conversations(state_kind);
";

const MIGRATION_060: &str = r"
CREATE TRIGGER conversations_state_kind_insert
BEFORE INSERT ON conversations
WHEN json_extract(NEW.state, '$.type') IS NOT NEW.state_kind
BEGIN
    SELECT RAISE(ABORT, 'conversation state_kind must match state type');
END;

CREATE TRIGGER conversations_state_kind_update
BEFORE UPDATE OF state, state_kind ON conversations
WHEN json_extract(NEW.state, '$.type') IS NOT NEW.state_kind
BEGIN
    SELECT RAISE(ABORT, 'conversation state_kind must match state type');
END;
";

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

const MIGRATION_080: &str = r"
DROP TRIGGER close_obligations_transition_graph;
CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;

DROP TRIGGER close_obligations_reject_missing_inspection_on_update;
CREATE TRIGGER close_obligations_reject_missing_inspection_on_update
BEFORE UPDATE ON close_obligations
FOR EACH ROW
WHEN NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested')
  AND NEW.inspection_generation IS NULL
BEGIN
    SELECT RAISE(ABORT, 'close_obligations inspection required for phase');
END;

DROP TRIGGER IF EXISTS close_expected_runtime_instance_kind_scope_matches;
DROP TRIGGER IF EXISTS close_expected_runtime_instance_required;
ALTER TABLE close_expected_retirement_resources DROP COLUMN runtime_resource_instance_id;
DROP TRIGGER IF EXISTS runtime_resource_instances_preserve_identity;
DROP INDEX IF EXISTS runtime_resource_instances_live_exact_identity;
DROP TABLE runtime_resource_instances;
DROP TRIGGER IF EXISTS close_expected_retirement_resources_reject_standalone_delete;
DROP TRIGGER IF EXISTS close_retirement_resource_history_reject_delete;
DELETE FROM close_retirement_resource_history
WHERE resource_kind IN ('bash_process_group', 'pty_session', 'browser_session');
DELETE FROM close_retirement_resource_dispatches
WHERE resource_kind IN ('bash_process_group', 'pty_session', 'browser_session');
DELETE FROM close_expected_retirement_resources
WHERE resource_kind IN ('bash_process_group', 'pty_session', 'browser_session');
CREATE TRIGGER close_expected_retirement_resources_reject_standalone_delete
BEFORE DELETE ON close_expected_retirement_resources
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM close_obligations obligation
    JOIN product_conversations product ON product.id = obligation.product_conversation_id
    WHERE obligation.attempt_id = OLD.attempt_id
)
BEGIN
    SELECT RAISE(ABORT, 'expected retirement resource can only be deleted with its root');
END;

CREATE TRIGGER close_retirement_resource_history_reject_delete
BEFORE DELETE ON close_retirement_resource_history
WHEN EXISTS (SELECT 1 FROM close_obligations WHERE attempt_id = OLD.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'retirement resource history belongs to its Close aggregate');
END;

CREATE TABLE product_conversation_work_scope_repairs (
    work_scope_id TEXT PRIMARY KEY,
    first_product_conversation_id TEXT NOT NULL,
    second_product_conversation_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state = 'needs_repair'),
    CHECK (first_product_conversation_id <> second_product_conversation_id)
);

INSERT INTO product_conversation_work_scope_repairs (
    work_scope_id, first_product_conversation_id, second_product_conversation_id, state
)
SELECT c.work_scope_id, MIN(c.product_conversation_id), MAX(c.product_conversation_id), 'needs_repair'
FROM conversations c
JOIN product_conversations product ON product.id = c.product_conversation_id
WHERE c.work_scope_id IS NOT NULL
  AND c.runtime_role <> 'coordinator'
  AND product.kind = 'ordinary'
GROUP BY c.work_scope_id
HAVING COUNT(DISTINCT c.product_conversation_id) > 1;

CREATE TABLE product_conversation_work_scope_missing_owners (
    work_scope_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state = 'needs_repair')
);

INSERT INTO product_conversation_work_scope_missing_owners (work_scope_id, state)
SELECT scope.id, 'needs_repair'
FROM work_scopes scope
WHERE NOT EXISTS (
    SELECT 1
    FROM conversations c
    JOIN product_conversations product ON product.id = c.product_conversation_id
    WHERE c.work_scope_id = scope.id
      AND c.runtime_role = 'user'
      AND product.kind = 'ordinary'
);

CREATE TABLE product_conversation_work_scopes (
    work_scope_id TEXT PRIMARY KEY REFERENCES work_scopes(id) ON DELETE RESTRICT,
    product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id) ON DELETE CASCADE,
    UNIQUE (product_conversation_id, work_scope_id)
);

INSERT INTO product_conversation_work_scopes (work_scope_id, product_conversation_id)
SELECT c.work_scope_id, MIN(c.product_conversation_id)
FROM conversations c
JOIN product_conversations product ON product.id = c.product_conversation_id
WHERE c.work_scope_id IS NOT NULL
  AND c.runtime_role = 'user'
  AND product.kind = 'ordinary'
GROUP BY c.work_scope_id
HAVING COUNT(DISTINCT c.product_conversation_id) = 1;

CREATE TRIGGER conversations_reject_repair_scope_owner
BEFORE INSERT ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL
 AND NEW.runtime_role <> 'coordinator'
 AND (
    EXISTS (
        SELECT 1 FROM product_conversation_work_scope_repairs
        WHERE work_scope_id = NEW.work_scope_id
    )
    OR EXISTS (
        SELECT 1 FROM product_conversation_work_scope_missing_owners
        WHERE work_scope_id = NEW.work_scope_id
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'work scope has unresolved product conversation ownership repair');
END;

CREATE TRIGGER conversations_reject_repair_scope_owner_on_update
BEFORE UPDATE OF product_conversation_id, work_scope_id, runtime_role ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL
 AND NEW.runtime_role <> 'coordinator'
 AND (
    EXISTS (
        SELECT 1 FROM product_conversation_work_scope_repairs
        WHERE work_scope_id = NEW.work_scope_id
    )
    OR EXISTS (
        SELECT 1 FROM product_conversation_work_scope_missing_owners
        WHERE work_scope_id = NEW.work_scope_id
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'work scope has unresolved product conversation ownership repair');
END;

CREATE TRIGGER conversations_assign_or_validate_work_scope_owner
BEFORE INSERT ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL AND NEW.runtime_role = 'user'
BEGIN
    INSERT OR IGNORE INTO product_conversation_work_scopes (work_scope_id, product_conversation_id)
    SELECT NEW.work_scope_id, NEW.product_conversation_id
    WHERE EXISTS (
        SELECT 1 FROM product_conversations
        WHERE id = NEW.product_conversation_id AND kind = 'ordinary'
    );
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM product_conversation_work_scopes
        WHERE work_scope_id = NEW.work_scope_id
          AND product_conversation_id = NEW.product_conversation_id
    ) THEN RAISE(ABORT, 'work scope belongs to a different ordinary product conversation') END;
END;

CREATE TRIGGER conversations_validate_participant_work_scope_owner
BEFORE INSERT ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL AND NEW.runtime_role = 'sub_agent'
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM product_conversation_work_scopes
        WHERE work_scope_id = NEW.work_scope_id
          AND product_conversation_id = NEW.product_conversation_id
    ) THEN RAISE(ABORT, 'work scope participant belongs to a different product conversation') END;
END;

CREATE TRIGGER conversations_validate_participant_work_scope_owner_on_update
BEFORE UPDATE OF product_conversation_id, work_scope_id, runtime_role ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL AND NEW.runtime_role = 'sub_agent'
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM product_conversation_work_scopes
        WHERE work_scope_id = NEW.work_scope_id
          AND product_conversation_id = NEW.product_conversation_id
    ) THEN RAISE(ABORT, 'work scope participant belongs to a different product conversation') END;
END;

CREATE TRIGGER conversations_validate_work_scope_owner_on_update
BEFORE UPDATE OF product_conversation_id, work_scope_id, runtime_role ON conversations
FOR EACH ROW
WHEN NEW.work_scope_id IS NOT NULL AND NEW.runtime_role = 'user'
BEGIN
    INSERT OR IGNORE INTO product_conversation_work_scopes (work_scope_id, product_conversation_id)
    SELECT NEW.work_scope_id, NEW.product_conversation_id
    WHERE EXISTS (
        SELECT 1 FROM product_conversations
        WHERE id = NEW.product_conversation_id AND kind = 'ordinary'
    );
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM product_conversation_work_scopes
        WHERE work_scope_id = NEW.work_scope_id
          AND product_conversation_id = NEW.product_conversation_id
    ) THEN RAISE(ABORT, 'work scope belongs to a different ordinary product conversation') END;
END;
";

#[cfg(test)]
mod migration_094_tests {
    use sqlx::sqlite::SqlitePoolOptions;

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn preserves_order_and_remaps_generation_and_watermark_before_enforcing_sequences() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE conversations (
                 id TEXT PRIMARY KEY NOT NULL,
                 clear_watermark INTEGER NOT NULL DEFAULT 0,
                 transcript_generation INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE messages (
                message_id TEXT PRIMARY KEY NOT NULL,
                conversation_id TEXT NOT NULL,
                sequence_id INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations(id, clear_watermark, transcript_generation)
             VALUES ('affected', 3, 7), ('clean', 5, 11)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (message_id, sequence_id, created_at) in [
            ("zero", 0_i64, "2026-08-30T00:00:00Z"),
            ("one-a", 1, "2026-08-30T00:00:01Z"),
            ("one-b", 1, "2026-08-30T00:00:02Z"),
            ("three-a", 3, "2026-08-30T00:00:03Z"),
            ("three-b", 3, "2026-08-30T00:00:04Z"),
            ("ten", 10, "2026-08-30T00:00:05Z"),
        ] {
            sqlx::query(
                "INSERT INTO messages(message_id, conversation_id, sequence_id, created_at)
                 VALUES (?1, 'affected', ?2, ?3)",
            )
            .bind(message_id)
            .bind(sequence_id)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO messages(message_id, conversation_id, sequence_id, created_at)
             VALUES ('clean-row', 'clean', 5, '2026-08-30T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(super::MIGRATION_094)
            .execute(&pool)
            .await
            .unwrap();
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT message_id, sequence_id FROM messages
             WHERE conversation_id = 'affected' ORDER BY sequence_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![
                ("zero".into(), 0),
                ("one-a".into(), 1),
                ("one-b".into(), 2),
                ("three-a".into(), 4),
                ("three-b".into(), 5),
                ("ten".into(), 12),
            ]
        );
        let conversations = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT id, clear_watermark, transcript_generation FROM conversations ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            conversations,
            vec![("affected".into(), 5, 8), ("clean".into(), 5, 11)]
        );

        // Idempotent replay of an existing identity is exempt from the monotonic
        // trigger; a new identity at the same or a regressing sequence is not.
        sqlx::query(
            "INSERT OR IGNORE INTO messages(message_id, conversation_id, sequence_id, created_at)
             VALUES ('ten', 'affected', 12, '2026-08-30T00:00:05Z')",
        )
        .execute(&pool)
        .await
        .expect("existing message identity remains replayable");
        for (message_id, sequence_id) in [("duplicate", 12_i64), ("regressing", 6_i64)] {
            assert!(sqlx::query(
                "INSERT OR IGNORE INTO messages(message_id, conversation_id, sequence_id, created_at)
                 VALUES (?1, 'affected', ?2, '2026-08-30T00:00:06Z')",
            )
            .bind(message_id)
            .bind(sequence_id)
            .execute(&pool)
            .await
            .is_err());
        }
    }

    #[test]
    fn lifecycle_cutover_preserves_timestamp_contract() {
        assert!(super::MIGRATION_095.contains("captured_at_unix_micros INTEGER NOT NULL"));
        assert!(super::MIGRATION_095.contains("CHECK (captured_at_unix_micros >= 0)"));
    }

    #[tokio::test]
    async fn lifecycle_cutover_preserves_cancellation_settlement_for_recovery() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE close_obligations (
                 attempt_id TEXT PRIMARY KEY, phase TEXT NOT NULL, updated_at TEXT NOT NULL
             );
             CREATE TABLE close_attempt_participants (
                 attempt_id TEXT NOT NULL, conversation_id TEXT NOT NULL
             );
             CREATE TABLE durable_turns (
                 turn_id INTEGER PRIMARY KEY, conversation_id TEXT NOT NULL,
                 generation INTEGER NOT NULL, owns_conversation INTEGER NOT NULL,
                 terminal_kind TEXT
             );
             CREATE TABLE close_attempt_direct_turn_settlement_captures (
                 attempt_id TEXT PRIMARY KEY, captured_at TEXT NOT NULL
             );
             CREATE TABLE close_attempt_direct_turn_settlements (
                 attempt_id TEXT NOT NULL, turn_id INTEGER NOT NULL,
                 expected_generation INTEGER NOT NULL,
                 PRIMARY KEY (attempt_id, turn_id)
             );
             INSERT INTO close_obligations VALUES
                 ('settling', 'settling_active_work', 'settling-time'),
                 ('cancelling', 'cancel_requested_during_settlement', 'cancelling-time');
             INSERT INTO close_attempt_participants VALUES
                 ('settling', 'settling-conv'), ('cancelling', 'cancelling-conv');
             INSERT INTO durable_turns VALUES
                 (11, 'settling-conv', 4, 1, NULL),
                 (12, 'cancelling-conv', 7, 1, NULL);
             INSERT OR IGNORE INTO close_attempt_direct_turn_settlement_captures (
                 attempt_id, captured_at
             )
             SELECT obligation.attempt_id, obligation.updated_at
             FROM close_obligations obligation
             WHERE obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement');
             INSERT OR IGNORE INTO close_attempt_direct_turn_settlements (
                 attempt_id, turn_id, expected_generation
             )
             SELECT obligation.attempt_id, turn.turn_id, turn.generation
             FROM close_obligations obligation
             JOIN close_attempt_participants participant
               ON participant.attempt_id = obligation.attempt_id
             JOIN durable_turns turn ON turn.conversation_id = participant.conversation_id
             WHERE obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement')
               AND turn.owns_conversation = 1 AND turn.terminal_kind IS NULL;",
        )
        .execute(&pool)
        .await
        .unwrap();

        let phases = sqlx::query_as::<_, (String, String)>(
            "SELECT attempt_id, phase FROM close_obligations ORDER BY attempt_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            phases,
            vec![
                (
                    "cancelling".into(),
                    "cancel_requested_during_settlement".into()
                ),
                ("settling".into(), "settling_active_work".into()),
            ]
        );
        let captures = sqlx::query_as::<_, (String, String)>(
            "SELECT attempt_id, captured_at FROM close_attempt_direct_turn_settlement_captures ORDER BY attempt_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            captures,
            vec![
                ("cancelling".into(), "cancelling-time".into()),
                ("settling".into(), "settling-time".into())
            ]
        );
        let targets = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT attempt_id, turn_id, expected_generation FROM close_attempt_direct_turn_settlements ORDER BY attempt_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            targets,
            vec![("cancelling".into(), 12, 7), ("settling".into(), 11, 4)]
        );
    }

    #[test]
    fn participant_snapshot_upgrade_captures_active_subordinates_and_cascades_delete() {
        assert!(super::MIGRATION_095.contains("WHERE obligation.phase <> 'completed'"));
        assert!(super::MIGRATION_095.contains("FOREIGN KEY (attempt_id, product_conversation_id)"));
        assert!(super::MIGRATION_095
            .contains("close participant must belong to the attempted ProductConversation"));
        assert!(super::MIGRATION_095.contains("CHECK (settlement_state IN ('live', 'deleted'))"));
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn lifecycle_cutover_reconciles_released_drift_without_dropping_rows() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE product_conversations (
                 id TEXT PRIMARY KEY, kind TEXT NOT NULL, ordinary_lifecycle TEXT
             );
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY, product_conversation_id TEXT, runtime_role TEXT,
                 parent_conversation_id TEXT, continued_in_conv_id TEXT, archived INTEGER
             );
             CREATE TABLE product_creation_jobs (
                 published_product_id TEXT, status TEXT NOT NULL
             );
             CREATE TABLE workflows (
                 workflow_id INTEGER PRIMARY KEY, profile_kind TEXT NOT NULL,
                 version INTEGER NOT NULL, generation INTEGER NOT NULL,
                 status TEXT NOT NULL, updated_at INTEGER NOT NULL
             );
             CREATE TABLE workflow_transitions (
                 workflow_id INTEGER NOT NULL, transition_id INTEGER NOT NULL,
                 from_version INTEGER NOT NULL, to_version INTEGER NOT NULL,
                 generation INTEGER NOT NULL, event_codec_family TEXT NOT NULL,
                 event_codec_version INTEGER NOT NULL, event_payload BLOB NOT NULL,
                 committed_at INTEGER NOT NULL,
                 PRIMARY KEY (workflow_id, transition_id),
                 UNIQUE (workflow_id, to_version),
                 CHECK (to_version = from_version + 1)
             );
             CREATE TABLE workflow_effects (
                 workflow_id INTEGER NOT NULL, effect_id INTEGER NOT NULL,
                 status TEXT NOT NULL, next_eligible_at INTEGER,
                 generation INTEGER NOT NULL, pending_reconciliation INTEGER NOT NULL,
                 PRIMARY KEY (workflow_id, effect_id)
             );
             CREATE TABLE durable_turns (
                 turn_id INTEGER PRIMARY KEY, workflow_id INTEGER NOT NULL UNIQUE,
                 conversation_id TEXT NOT NULL,
                 disposition TEXT NOT NULL CHECK (disposition IN ('Runtime', 'Steering')),
                 terminal_kind TEXT, terminal_reason TEXT, generation INTEGER NOT NULL,
                 owns_conversation INTEGER NOT NULL,
                 canonical_message_id TEXT,
                 CHECK (terminal_kind = 'Failed' OR terminal_reason IS NULL)
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO product_conversations VALUES
                 ('open-product', 'ordinary', 'history'),
                 ('history-product', 'ordinary', 'open'),
                 ('delivery-pending-product', 'ordinary', 'history'),
                 ('delivery-failed-product', 'ordinary', 'history'),
                 ('published-history-product', 'ordinary', 'open'),
                 ('coordinator', 'coordinator', NULL);
             INSERT INTO conversations VALUES
                 ('open-row', 'open-product', 'user', NULL, NULL, 0),
                 ('history-row', 'history-product', 'user', NULL, NULL, 1),
                 ('subordinate', 'history-product', 'sub_agent', 'history-row', NULL, 0),
                 ('delivery-pending-row', 'delivery-pending-product', 'user', NULL, NULL, 1),
                 ('delivery-failed-row', 'delivery-failed-product', 'user', NULL, NULL, 1),
                 ('published-history-row', 'published-history-product', 'user', NULL, NULL, 1),
                 ('coordinator-row', 'coordinator', 'coordinator', NULL, NULL, 0);
             INSERT INTO product_creation_jobs VALUES
                 ('delivery-pending-product', 'delivery_pending'),
                 ('delivery-failed-product', 'delivery_failed'),
                 ('published-history-product', 'published');
             INSERT INTO workflows VALUES
                 (11, 'direct_turn', 2, 7, 'Active', 100),
                 (12, 'wake', 2, 7, 'Active', 100);
             INSERT INTO workflow_transitions VALUES
                 (11, 1, 0, 1, 0, 'direct_turn.event', 1, X'01', 90),
                 (12, 1, 0, 1, 0, 'wake.event', 1, X'01', 90);
             INSERT INTO workflow_effects VALUES
                 (11, 1, 'Eligible', 200, 7, 1),
                 (12, 1, 'Eligible', 200, 7, 1);
             INSERT INTO durable_turns VALUES
                 (1, 11, 'history-row', 'Runtime', NULL, NULL, 7, 1, NULL),
                 (2, 12, 'subordinate', 'Runtime', NULL, NULL, 7, 1, NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(
            "CREATE TEMP TABLE migration_095_cancelled_direct_turns AS
             SELECT turn.turn_id, turn.workflow_id, workflow.generation + 1 AS next_generation,
                    workflow.version AS from_version, workflow.updated_at AS committed_at
             FROM durable_turns turn
             JOIN workflows workflow ON workflow.workflow_id = turn.workflow_id
             JOIN conversations member ON member.id = turn.conversation_id
             JOIN product_conversations product ON product.id = member.product_conversation_id
             WHERE turn.terminal_kind IS NULL AND turn.canonical_message_id IS NULL
               AND workflow.profile_kind = 'direct_turn'
               AND workflow.status NOT IN ('Cancelled', 'Completed', 'Failed', 'Deleted')
               AND product.kind = 'ordinary' AND NOT EXISTS (
                 SELECT 1 FROM conversations latest
                 WHERE latest.product_conversation_id = product.id
                   AND latest.runtime_role = 'user'
                   AND latest.parent_conversation_id IS NULL
                   AND latest.continued_in_conv_id IS NULL
                   AND latest.archived = 0
               );
             INSERT INTO workflow_transitions (
                 workflow_id, transition_id, from_version, to_version, generation,
                 event_codec_family, event_codec_version, event_payload, committed_at
             )
             SELECT workflow_id, 3, from_version, from_version + 1, next_generation,
                    'direct_turn.event', 1,
                    X'7b225465726d696e616c223a7b227465726d696e616c223a2243616e63656c6c6564227d7d',
                    committed_at + 1
             FROM migration_095_cancelled_direct_turns;
             UPDATE workflow_effects
             SET status = 'Invalidated', next_eligible_at = NULL,
                 generation = generation + 1, pending_reconciliation = 0
             WHERE workflow_id IN (SELECT workflow_id FROM migration_095_cancelled_direct_turns)
               AND status NOT IN ('Receipted', 'Invalidated');
             UPDATE durable_turns
             SET terminal_kind = 'Cancelled', terminal_reason = NULL,
                 generation = (
                     SELECT next_generation FROM migration_095_cancelled_direct_turns cancelled
                     WHERE cancelled.turn_id = durable_turns.turn_id
                 ),
                 owns_conversation = 0
             WHERE turn_id IN (SELECT turn_id FROM migration_095_cancelled_direct_turns);
             UPDATE workflows
             SET status = 'Cancelled', generation = (
                     SELECT next_generation FROM migration_095_cancelled_direct_turns cancelled
                     WHERE cancelled.workflow_id = workflows.workflow_id
                 ), version = version + 1, updated_at = updated_at + 1
             WHERE workflow_id IN (SELECT workflow_id FROM migration_095_cancelled_direct_turns);
             DROP TABLE migration_095_cancelled_direct_turns;",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "UPDATE product_conversations
             SET ordinary_lifecycle = CASE WHEN EXISTS (
                 SELECT 1 FROM conversations latest
                 WHERE latest.product_conversation_id = product_conversations.id
                   AND latest.runtime_role = 'user'
                   AND latest.parent_conversation_id IS NULL
                   AND latest.continued_in_conv_id IS NULL
                   AND latest.archived = 0
             ) OR EXISTS (
                 SELECT 1 FROM product_creation_jobs job
                 WHERE job.published_product_id = product_conversations.id
                   AND job.status IN ('delivery_pending', 'delivery_failed')
             ) THEN 'open' ELSE 'history' END
             WHERE kind = 'ordinary'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let products = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT id, ordinary_lifecycle FROM product_conversations ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            products,
            vec![
                ("coordinator".into(), None),
                ("delivery-failed-product".into(), Some("open".into())),
                ("delivery-pending-product".into(), Some("open".into())),
                ("history-product".into(), Some("history".into())),
                ("open-product".into(), Some("open".into())),
                ("published-history-product".into(), Some("history".into())),
            ]
        );
        let turn: (String, String, Option<String>, i64, i64) = sqlx::query_as(
            "SELECT disposition, terminal_kind, terminal_reason, generation, owns_conversation
             FROM durable_turns WHERE turn_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(turn, ("Runtime".into(), "Cancelled".into(), None, 8, 0));
        let untouched_wake_turn: (Option<String>, i64, i64) = sqlx::query_as(
            "SELECT terminal_kind, generation, owns_conversation FROM durable_turns WHERE turn_id = 2",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(untouched_wake_turn, (None, 7, 1));
        let workflow: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT status, version, generation, updated_at FROM workflows WHERE workflow_id = 11",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workflow, ("Cancelled".into(), 3, 8, 101));
        let transition: (i64, i64, i64, String, i64, Vec<u8>, i64) = sqlx::query_as(
            "SELECT from_version, to_version, generation, event_codec_family,
                    event_codec_version, event_payload, committed_at
             FROM workflow_transitions WHERE workflow_id = 11 AND transition_id = 3",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(transition.0, 2);
        assert_eq!(transition.1, 3);
        assert_eq!(transition.2, 8);
        assert_eq!(transition.3, "direct_turn.event");
        assert_eq!(transition.4, 1);
        assert_eq!(transition.5, br#"{"Terminal":{"terminal":"Cancelled"}}"#);
        assert_eq!(transition.6, 101);
        let effect: (String, Option<i64>, i64, i64) = sqlx::query_as(
            "SELECT status, next_eligible_at, generation, pending_reconciliation
             FROM workflow_effects WHERE workflow_id = 11 AND effect_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(effect, ("Invalidated".into(), None, 8, 0));
        let wake_workflow: (String, i64, i64) = sqlx::query_as(
            "SELECT status, version, generation FROM workflows WHERE workflow_id = 12",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(wake_workflow, ("Active".into(), 2, 7));

        let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row_count, 7);
    }
}

const MIGRATION_095: &str = r"
CREATE UNIQUE INDEX close_obligations_attempt_product_identity
ON close_obligations(attempt_id, product_conversation_id);
CREATE UNIQUE INDEX conversations_member_product_identity
ON conversations(id, product_conversation_id);
CREATE TABLE close_attempt_participants (
    attempt_id TEXT NOT NULL,
    product_conversation_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    settlement_state TEXT NOT NULL DEFAULT 'live'
        CHECK (settlement_state IN ('live', 'deleted')),
    captured_at_unix_micros INTEGER NOT NULL
        CHECK (typeof(captured_at_unix_micros) = 'integer')
        CHECK (captured_at_unix_micros >= 0),
    PRIMARY KEY (attempt_id, conversation_id),
    FOREIGN KEY (attempt_id, product_conversation_id)
        REFERENCES close_obligations(attempt_id, product_conversation_id) ON DELETE CASCADE
);
CREATE TRIGGER close_attempt_participants_require_same_product_on_insert
BEFORE INSERT ON close_attempt_participants
WHEN NOT EXISTS (
    SELECT 1 FROM conversations participant
    WHERE participant.id = NEW.conversation_id
      AND participant.product_conversation_id = NEW.product_conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'close participant must belong to the attempted ProductConversation');
END;
CREATE TRIGGER conversations_reject_sealed_participant_delete
BEFORE DELETE ON conversations
WHEN EXISTS (
    SELECT 1
    FROM close_attempt_participants participant
    JOIN close_obligations obligation ON obligation.attempt_id = participant.attempt_id
    WHERE participant.conversation_id = OLD.id
      AND participant.settlement_state = 'live'
      AND obligation.phase <> 'completed'
)
BEGIN
    SELECT RAISE(ABORT, 'active Close rejects sealed participant deletion');
END;
INSERT INTO close_attempt_participants (
    attempt_id, product_conversation_id, conversation_id, captured_at_unix_micros
)
SELECT obligation.attempt_id, obligation.product_conversation_id, participant.id,
       CAST(ROUND((julianday(obligation.created_at) - 2440587.5) * 86400000000.0) AS INTEGER)
FROM close_obligations obligation
JOIN conversations participant
  ON participant.product_conversation_id = obligation.product_conversation_id
WHERE obligation.phase <> 'completed';
INSERT OR IGNORE INTO close_attempt_participants (
    attempt_id, product_conversation_id, conversation_id, captured_at_unix_micros
)
SELECT member.attempt_id, obligation.product_conversation_id, member.conversation_id,
       CAST(ROUND((julianday(member.captured_at) - 2440587.5) * 86400000000.0) AS INTEGER)
FROM close_attempt_members member
JOIN close_obligations obligation ON obligation.attempt_id = member.attempt_id;

DROP TRIGGER close_attempt_direct_turn_settlement_target_requires_latest_member;
CREATE TRIGGER close_attempt_direct_turn_settlement_target_requires_sealed_participant
BEFORE INSERT ON close_attempt_direct_turn_settlements
WHEN NOT EXISTS (
    SELECT 1
    FROM durable_turns turn
    JOIN close_attempt_participants participant
      ON participant.conversation_id = turn.conversation_id
    WHERE participant.attempt_id = NEW.attempt_id
      AND turn.turn_id = NEW.turn_id
      AND turn.generation = NEW.expected_generation
      AND turn.owns_conversation = 1
      AND turn.terminal_kind IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'close direct-turn settlement target must be a sealed active participant');
END;
INSERT OR IGNORE INTO close_attempt_direct_turn_settlement_captures (
    attempt_id, captured_at
)
SELECT obligation.attempt_id, obligation.updated_at
FROM close_obligations obligation
WHERE obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement');
INSERT OR IGNORE INTO close_attempt_direct_turn_settlements (
    attempt_id, turn_id, expected_generation
)
SELECT obligation.attempt_id, turn.turn_id, turn.generation
FROM close_obligations obligation
JOIN close_attempt_participants participant
  ON participant.attempt_id = obligation.attempt_id
JOIN durable_turns turn ON turn.conversation_id = participant.conversation_id
WHERE obligation.phase IN ('settling_active_work', 'cancel_requested_during_settlement')
  AND turn.owns_conversation = 1
  AND turn.terminal_kind IS NULL;

DROP TRIGGER close_obligations_transition_graph;
CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('awaiting_retirement_inspection', 'needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;

CREATE TRIGGER conversations_reject_participant_insert_during_close
BEFORE INSERT ON conversations
WHEN NEW.product_conversation_id IS NOT NULL
 AND (
    EXISTS (
      SELECT 1 FROM close_obligations obligation
      WHERE obligation.product_conversation_id = NEW.product_conversation_id
        AND obligation.phase <> 'completed'
    )
    OR EXISTS (
      SELECT 1 FROM product_conversations product
      WHERE product.id = NEW.product_conversation_id
        AND product.kind = 'ordinary'
        AND product.ordinary_lifecycle = 'history'
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'non-writable ProductConversation rejects new aggregate participants');
END;

CREATE TEMP TABLE migration_095_cancelled_direct_turns AS
SELECT turn.turn_id, turn.workflow_id, workflow.generation + 1 AS next_generation,
       workflow.version AS from_version, workflow.updated_at AS committed_at
FROM durable_turns turn
JOIN workflows workflow ON workflow.workflow_id = turn.workflow_id
JOIN conversations member ON member.id = turn.conversation_id
JOIN product_conversations product ON product.id = member.product_conversation_id
WHERE turn.terminal_kind IS NULL
  AND turn.canonical_message_id IS NULL
  AND workflow.profile_kind = 'direct_turn'
  AND workflow.status NOT IN ('Cancelled', 'Completed', 'Failed', 'Deleted')
  AND product.kind = 'ordinary'
  AND NOT EXISTS (
    SELECT 1 FROM conversations latest
    WHERE latest.product_conversation_id = product.id
      AND latest.runtime_role = 'user'
      AND latest.parent_conversation_id IS NULL
      AND latest.continued_in_conv_id IS NULL
      AND latest.archived = 0
  );
INSERT INTO workflow_transitions (
    workflow_id, transition_id, from_version, to_version, generation,
    event_codec_family, event_codec_version, event_payload, committed_at
)
SELECT workflow_id, 3, from_version, from_version + 1, next_generation,
       'direct_turn.event', 1,
       X'7b225465726d696e616c223a7b227465726d696e616c223a2243616e63656c6c6564227d7d',
       committed_at + 1
FROM migration_095_cancelled_direct_turns;
UPDATE workflow_effects
SET status = 'Invalidated', next_eligible_at = NULL,
    generation = generation + 1, pending_reconciliation = 0
WHERE workflow_id IN (
    SELECT workflow_id FROM migration_095_cancelled_direct_turns
) AND status NOT IN ('Receipted', 'Invalidated');
UPDATE durable_turns
SET terminal_kind = 'Cancelled', terminal_reason = NULL,
    generation = (
        SELECT next_generation FROM migration_095_cancelled_direct_turns cancelled
        WHERE cancelled.turn_id = durable_turns.turn_id
    ),
    owns_conversation = 0
WHERE turn_id IN (SELECT turn_id FROM migration_095_cancelled_direct_turns);
UPDATE workflows
SET status = 'Cancelled', generation = (
        SELECT next_generation FROM migration_095_cancelled_direct_turns cancelled
        WHERE cancelled.workflow_id = workflows.workflow_id
    ),
    version = version + 1, updated_at = updated_at + 1
WHERE workflow_id IN (SELECT workflow_id FROM migration_095_cancelled_direct_turns);
DROP TABLE migration_095_cancelled_direct_turns;

UPDATE product_creation_jobs
SET status = 'delivery_failed', last_error = 'completed_close_won_cutover',
    delivery_retry_at_unix_micros = NULL,
    claim_worker_id = NULL, claim_token = NULL, claim_lease_until_unix_micros = NULL
WHERE status = 'delivery_pending'
  AND published_product_id IN (
    SELECT obligation.product_conversation_id FROM close_obligations obligation
    WHERE obligation.phase = 'completed' AND obligation.close_outcome = 'archived'
      AND NOT EXISTS (
        SELECT 1 FROM conversations latest
        WHERE latest.product_conversation_id = obligation.product_conversation_id
          AND latest.runtime_role = 'user'
          AND latest.parent_conversation_id IS NULL
          AND latest.continued_in_conv_id IS NULL
          AND latest.archived = 0
      )
  );

UPDATE product_conversations
SET ordinary_lifecycle = CASE WHEN EXISTS (
    SELECT 1
    FROM conversations latest
    WHERE latest.product_conversation_id = product_conversations.id
      AND latest.runtime_role = 'user'
      AND latest.parent_conversation_id IS NULL
      AND latest.continued_in_conv_id IS NULL
      AND latest.archived = 0
) OR EXISTS (
    SELECT 1 FROM product_creation_jobs job
    WHERE job.published_product_id = product_conversations.id
      AND job.status IN ('delivery_pending', 'delivery_failed')
      AND NOT EXISTS (
        SELECT 1 FROM close_obligations obligation
        WHERE obligation.product_conversation_id = product_conversations.id
          AND obligation.phase = 'completed'
          AND obligation.close_outcome = 'archived'
          AND NOT EXISTS (
            SELECT 1 FROM conversations latest
            WHERE latest.product_conversation_id = product_conversations.id
              AND latest.runtime_role = 'user'
              AND latest.parent_conversation_id IS NULL
              AND latest.continued_in_conv_id IS NULL
              AND latest.archived = 0
          )
      )
) THEN 'open' ELSE 'history' END
WHERE kind = 'ordinary';
";

const MIGRATION_094: &str = r"
-- Released databases could contain duplicate conversation-local sequences because
-- message_id was the only uniqueness constraint. Preserve every row and the durable
-- order (sequence_id, created_at, message_id), shifting later rows only enough to
-- make each affected conversation strictly increasing.
CREATE TEMP TABLE migration_094_sequence_map (
    message_id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL,
    old_sequence_id INTEGER NOT NULL,
    new_sequence_id INTEGER NOT NULL
);

INSERT INTO migration_094_sequence_map
    (message_id, conversation_id, old_sequence_id, new_sequence_id)
WITH duplicate_conversations AS (
    SELECT conversation_id
    FROM messages
    GROUP BY conversation_id, sequence_id
    HAVING COUNT(*) > 1
), ordered AS (
    SELECT message_id, conversation_id, sequence_id,
           ROW_NUMBER() OVER (
               PARTITION BY conversation_id
               ORDER BY sequence_id, created_at, message_id
           ) AS ordered_rank,
           DENSE_RANK() OVER (
               PARTITION BY conversation_id ORDER BY sequence_id
           ) AS distinct_sequence_rank
    FROM messages
    WHERE conversation_id IN (SELECT conversation_id FROM duplicate_conversations)
)
SELECT message_id, conversation_id, sequence_id,
       sequence_id + ordered_rank - distinct_sequence_rank
FROM ordered;

-- Move the complete affected partition out of the positive sequence namespace
-- before installing final values, avoiding transient collisions during UPDATE.
UPDATE messages
SET sequence_id = -1 - (
    SELECT new_sequence_id FROM migration_094_sequence_map mapping
    WHERE mapping.message_id = messages.message_id
)
WHERE message_id IN (SELECT message_id FROM migration_094_sequence_map);

UPDATE messages
SET sequence_id = (
    SELECT new_sequence_id FROM migration_094_sequence_map mapping
    WHERE mapping.message_id = messages.message_id
)
WHERE message_id IN (SELECT message_id FROM migration_094_sequence_map);

-- clear_watermark is the only durable scalar keyed to transcript sequences. Map
-- it to the greatest repaired sequence that represents an originally-cleared row.
UPDATE conversations
SET clear_watermark = COALESCE((
        SELECT MAX(mapping.new_sequence_id)
        FROM migration_094_sequence_map mapping
        WHERE mapping.conversation_id = conversations.id
          AND mapping.old_sequence_id <= conversations.clear_watermark
    ), clear_watermark),
    transcript_generation = transcript_generation + 1
WHERE id IN (SELECT DISTINCT conversation_id FROM migration_094_sequence_map);

DROP TABLE migration_094_sequence_map;
DROP INDEX IF EXISTS idx_messages_conversation;
CREATE UNIQUE INDEX messages_conversation_sequence
ON messages(conversation_id, sequence_id);

CREATE TRIGGER messages_require_increasing_sequence
BEFORE INSERT ON messages
FOR EACH ROW
WHEN NOT EXISTS (SELECT 1 FROM messages WHERE message_id = NEW.message_id)
 AND NEW.sequence_id <= COALESCE((
     SELECT MAX(sequence_id) FROM messages WHERE conversation_id = NEW.conversation_id
 ), -1)
BEGIN
    SELECT RAISE(ABORT, 'message sequence must strictly increase within conversation');
END;
";

const MIGRATION_079: &str = r"
ALTER TABLE close_expected_retirement_resources
ADD COLUMN runtime_resource_instance_id TEXT REFERENCES runtime_resource_instances(instance_id) ON DELETE RESTRICT;

CREATE TRIGGER close_expected_runtime_instance_kind_scope_matches
BEFORE UPDATE OF runtime_resource_instance_id ON close_expected_retirement_resources
FOR EACH ROW
WHEN NEW.runtime_resource_instance_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1 FROM runtime_resource_instances instance
    WHERE instance.instance_id = NEW.runtime_resource_instance_id
      AND instance.work_scope_id = NEW.scope
      AND (
        (NEW.resource_kind = 'bash_process_group' AND instance.resource_kind = 'bash')
        OR (NEW.resource_kind = 'tmux_server' AND instance.resource_kind = 'tmux')
        OR (NEW.resource_kind = 'pty_session' AND instance.resource_kind = 'pty')
        OR (NEW.resource_kind = 'browser_session' AND instance.resource_kind = 'browser')
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'close expected runtime resource must bind matching scope and kind instance');
END;

CREATE TRIGGER close_expected_runtime_instance_required
BEFORE UPDATE OF runtime_resource_instance_id ON close_expected_retirement_resources
FOR EACH ROW
WHEN NEW.resource_kind IN ('bash_process_group', 'tmux_server', 'pty_session', 'browser_session')
 AND NEW.runtime_resource_instance_id IS NULL
BEGIN
    SELECT RAISE(ABORT, 'close runtime resource instance binding cannot be removed');
END;
";

const MIGRATION_078: &str = r"
CREATE TABLE runtime_resource_instances (
    instance_id TEXT PRIMARY KEY NOT NULL,
    work_scope_id TEXT NOT NULL REFERENCES work_scopes(id) ON DELETE RESTRICT,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('bash', 'tmux', 'pty', 'browser')),
    state TEXT NOT NULL CHECK (state IN ('live', 'retirement_pending', 'retired', 'needs_repair')),
    launch_uuid TEXT NOT NULL CHECK (length(trim(launch_uuid)) > 0),
    pid INTEGER,
    process_birth TEXT,
    pgid INTEGER,
    tmux_socket_path TEXT,
    tmux_server_token TEXT,
    browser_session_key TEXT,
    browser_audience TEXT,
    browser_profile_path TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (resource_kind = 'bash'
         AND pid IS NOT NULL AND process_birth IS NOT NULL AND pgid IS NOT NULL
         AND tmux_socket_path IS NULL AND tmux_server_token IS NULL
         AND browser_session_key IS NULL AND browser_audience IS NULL AND browser_profile_path IS NULL)
        OR
        (resource_kind = 'tmux'
         AND pid IS NULL AND process_birth IS NULL AND pgid IS NULL
         AND tmux_socket_path IS NOT NULL AND tmux_server_token IS NOT NULL
         AND browser_session_key IS NULL AND browser_audience IS NULL AND browser_profile_path IS NULL)
        OR
        (resource_kind = 'pty'
         AND pid IS NOT NULL AND process_birth IS NOT NULL AND pgid IS NULL
         AND tmux_socket_path IS NULL AND tmux_server_token IS NULL
         AND browser_session_key IS NULL AND browser_audience IS NULL AND browser_profile_path IS NULL)
        OR
        (resource_kind = 'browser'
         AND pid IS NOT NULL AND process_birth IS NOT NULL AND pgid IS NULL
         AND tmux_socket_path IS NULL AND tmux_server_token IS NULL
         AND browser_session_key IS NOT NULL AND browser_audience IS NOT NULL AND browser_profile_path IS NOT NULL)
    )
);

CREATE UNIQUE INDEX runtime_resource_instances_live_exact_identity
ON runtime_resource_instances (
    work_scope_id, resource_kind, launch_uuid,
    COALESCE(pid, -1), COALESCE(process_birth, ''), COALESCE(pgid, -1),
    COALESCE(tmux_socket_path, ''), COALESCE(tmux_server_token, ''),
    COALESCE(browser_session_key, ''), COALESCE(browser_audience, ''), COALESCE(browser_profile_path, '')
)
WHERE state <> 'retired';

CREATE TRIGGER runtime_resource_instances_preserve_identity
BEFORE UPDATE ON runtime_resource_instances
FOR EACH ROW
WHEN NEW.instance_id <> OLD.instance_id
  OR NEW.work_scope_id <> OLD.work_scope_id
  OR NEW.resource_kind <> OLD.resource_kind
  OR NEW.launch_uuid <> OLD.launch_uuid
  OR NEW.pid IS NOT OLD.pid
  OR NEW.process_birth IS NOT OLD.process_birth
  OR NEW.pgid IS NOT OLD.pgid
  OR NEW.tmux_socket_path IS NOT OLD.tmux_socket_path
  OR NEW.tmux_server_token IS NOT OLD.tmux_server_token
  OR NEW.browser_session_key IS NOT OLD.browser_session_key
  OR NEW.browser_audience IS NOT OLD.browser_audience
  OR NEW.browser_profile_path IS NOT OLD.browser_profile_path
BEGIN
    SELECT RAISE(ABORT, 'runtime resource instance identity is immutable');
END;
";

const MIGRATION_077: &str = r"
CREATE TABLE close_retirement_resource_dispatches (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    identity_kind TEXT NOT NULL,
    identity_codec TEXT NOT NULL,
    identity_value TEXT NOT NULL,
    dispatched_at_us INTEGER NOT NULL
        CHECK (typeof(dispatched_at_us) = 'integer' AND dispatched_at_us >= 0),
    PRIMARY KEY (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ),
    FOREIGN KEY (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ) REFERENCES close_expected_retirement_resources (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ) ON DELETE RESTRICT,
    CHECK (resource_kind IN (
        'worktree', 'work_scope', 'bash_process_group', 'tmux_server',
        'pty_session', 'browser_session', 'equivalent_live_resource'
    )),
    CHECK (
        (resource_kind = 'worktree' AND identity_kind = 'worktree')
        OR (resource_kind <> 'worktree' AND identity_kind = 'opaque')
    )
);

CREATE TRIGGER close_retirement_resource_dispatches_require_authority
BEFORE INSERT ON close_retirement_resource_dispatches
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM close_obligations obligation
    JOIN close_retirement_inventories inventory
      ON inventory.attempt_id = obligation.attempt_id
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('retirement_requested', 'needs_repair')
      AND obligation.inspection_generation = NEW.inspection_generation
      AND obligation.inspection_fingerprint = NEW.inspection_fingerprint
      AND inventory.scope = NEW.scope
      AND inventory.inspection_generation = NEW.inspection_generation
      AND inventory.inspection_fingerprint = NEW.inspection_fingerprint
      AND inventory.sealed = 1
)
BEGIN
    SELECT RAISE(ABORT, 'close retirement dispatch lacks exact active authority');
END;
";

const MIGRATION_076: &str = "SELECT 1;";

async fn migration_076_extend_close_retirement_resource_kinds(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<()> {
    const TABLES: [&str; 3] = [
        "close_expected_retirement_resources",
        "close_retirement_resources",
        "close_retirement_resource_history",
    ];
    const RESOURCE_KIND_TAIL: &str =
        "'browser_session',\n        'equivalent_live_resource'\n    ))";
    const EXTENDED_RESOURCE_KIND_TAIL: &str =
        "'browser_session',\n        'equivalent_live_resource',\n        'work_scope'\n    ))";
    const HISTORY_FOREIGN_KEY: &str = "    FOREIGN KEY (\n        attempt_id, scope, inspection_generation, inspection_fingerprint,";
    const HISTORY_CONSTRAINTS: &str = "    CHECK (resource_kind IN (\n        'worktree',\n        'bash_process_group',\n        'tmux_server',\n        'pty_session',\n        'browser_session',\n        'equivalent_live_resource',\n        'work_scope'\n    )),\n    CHECK (\n        (resource_kind = 'worktree' AND identity_kind = 'worktree')\n        OR (resource_kind <> 'worktree' AND identity_kind = 'opaque')\n    ),\n";

    let mut table_sql = Vec::with_capacity(TABLES.len());
    for table in TABLES {
        let sql: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1")
                .bind(table)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| {
                    DbError::Serialization(format!("missing {table} for migration 76"))
                })?;
        table_sql.push((table, sql));
    }

    let objects: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master\n         WHERE sql IS NOT NULL\n           AND (\n             (type = 'index' AND tbl_name IN (\n                 'close_expected_retirement_resources',\n                 'close_retirement_resources',\n                 'close_retirement_resource_history'\n             ))\n             OR (type = 'trigger' AND (\n                 tbl_name IN (\n                     'close_expected_retirement_resources',\n                     'close_retirement_resources',\n                     'close_retirement_resource_history'\n                 )\n                 OR sql LIKE '%close_expected_retirement_resources%'\n                 OR sql LIKE '%close_retirement_resources%'\n                 OR sql LIKE '%close_retirement_resource_history%'\n             ))\n           )\n         ORDER BY CASE type WHEN 'index' THEN 0 ELSE 1 END, name",
    )
    .fetch_all(&mut **tx)
    .await?;
    for (name, _) in &objects {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP INDEX IF EXISTS {name};")))
            .execute(&mut **tx)
            .await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP TRIGGER IF EXISTS {name};"
        )))
        .execute(&mut **tx)
        .await?;
    }

    for table in TABLES.iter().rev() {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} RENAME TO {table}_migration_076_old;"
        )))
        .execute(&mut **tx)
        .await?;
    }

    for (table, sql) in &table_sql {
        let sql = if *table == "close_retirement_resource_history" {
            sql.replacen(
                HISTORY_FOREIGN_KEY,
                &(HISTORY_CONSTRAINTS.to_string() + HISTORY_FOREIGN_KEY),
                1,
            )
        } else {
            sql.replacen(RESOURCE_KIND_TAIL, EXTENDED_RESOURCE_KIND_TAIL, 1)
        };
        if sql
            == *table_sql
                .iter()
                .find_map(|(candidate, original)| (*candidate == *table).then_some(original))
                .expect("captured table SQL must contain the current table")
        {
            return Err(DbError::Serialization(format!(
                "unexpected {table} definition for migration 76"
            )));
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut **tx)
            .await?;
    }

    for table in TABLES {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {table} SELECT * FROM {table}_migration_076_old;"
        )))
        .execute(&mut **tx)
        .await?;
    }

    for table in TABLES.iter().rev() {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP TABLE {table}_migration_076_old;"
        )))
        .execute(&mut **tx)
        .await?;
    }

    for (_, sql) in objects {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut **tx)
            .await?;
    }

    Ok(())
}

async fn migration_065_preflight(tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
    let work_scope_id: Option<String> = sqlx::query_scalar(
        "SELECT work_scope_id
         FROM conversations
         WHERE work_scope_id IS NOT NULL AND project_id IS NOT NULL
         GROUP BY work_scope_id
         HAVING COUNT(DISTINCT project_id) > 1
         ORDER BY work_scope_id
         LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(work_scope_id) = work_scope_id else {
        return Ok(());
    };
    let project_ids: Vec<String> = sqlx::query(
        "SELECT DISTINCT project_id
         FROM conversations
         WHERE work_scope_id = ?1 AND project_id IS NOT NULL
         ORDER BY project_id
         LIMIT 2",
    )
    .bind(&work_scope_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|row| row.get("project_id"))
    .collect();
    let [first, second] = project_ids.as_slice() else {
        return Err(DbError::Serialization(
            "migration 65 conflict preflight found fewer than two project ids".to_string(),
        ));
    };
    Err(DbError::GitRepositoryWorkScopeProjectConflict {
        work_scope_id: WorkScopeId::parse(work_scope_id)
            .map_err(|error| DbError::Serialization(error.to_string()))?,
        project_ids: [
            ProjectSeedId::parse(first.clone())
                .map_err(|error| DbError::Serialization(error.to_string()))?,
            ProjectSeedId::parse(second.clone())
                .map_err(|error| DbError::Serialization(error.to_string()))?,
        ],
    })
}

async fn migration_080_prepare(tx: &mut Transaction<'_, Sqlite>) -> DbResult<bool> {
    let has_product_conversation: bool = sqlx::query_scalar(
        "SELECT EXISTS(\n            SELECT 1 FROM pragma_table_info('conversations')\n            WHERE name = 'product_conversation_id'\n        )",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !has_product_conversation {
        return Ok(false);
    }
    Ok(true)
}

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

        if migration.version == 68 {
            retire_commission_review::run(pool, migration.version, migration.name).await?;
            applied += 1;
            continue;
        }

        if migration.version == 69 {
            retire_commission_review::backfill_settlements(pool, migration.version, migration.name)
                .await?;
            applied += 1;
            continue;
        }

        // Apply the migration body and its version record in one transaction so
        // a crash mid-migration leaves the database all-or-nothing: a partially
        // applied but unrecorded migration would fail to re-run (missing/duplicate
        // column) and abort startup.
        let mut tx = pool.begin().await?;

        if migration.version == 76 {
            migration_076_extend_close_retirement_resource_kinds(&mut tx).await?;
        }

        if migration.version == 65 {
            migration_065_preflight(&mut tx).await?;
        }

        if migration.version == 80 && !migration_080_prepare(&mut tx).await? {
            sqlx::query("INSERT INTO _migrations (version, name) VALUES (?, ?)")
                .bind(migration.version)
                .bind(migration.name)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            applied += 1;
            continue;
        }

        if migration.version == 59 {
            let state_kind_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info('conversations') WHERE name = 'state_kind'
                )",
            )
            .fetch_one(&mut *tx)
            .await?;
            if state_kind_exists {
                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_conversations_state_kind
                     ON conversations(state_kind)",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query("INSERT INTO _migrations (version, name) VALUES (?, ?)")
                    .bind(migration.version)
                    .bind(migration.name)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                applied += 1;
                continue;
            }
        }

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

const MIGRATION_081: &str = r"
DROP TRIGGER close_retirement_resources_require_absence_proof_on_insert;
CREATE TRIGGER close_retirement_resources_require_absence_proof_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NEW.proof_kind = 'absence_adopted' AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    WHERE proof.scope = NEW.scope
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND NEW.absence_basis = 'preexisting_exact_identity_evidence'
      AND proof.attempt_id <> NEW.attempt_id
      AND EXISTS (
          SELECT 1
          FROM close_attempt_members proof_member
          JOIN conversations proof_conversation ON proof_conversation.id = proof_member.conversation_id
          JOIN close_attempt_members current_member ON current_member.attempt_id = NEW.attempt_id
          JOIN conversations current_conversation ON current_conversation.id = current_member.conversation_id
          WHERE proof_member.attempt_id = proof.attempt_id
            AND proof_conversation.product_conversation_id = current_conversation.product_conversation_id
      )
      AND proof.proof_kind IN ('retired', 'absence_adopted')
    UNION ALL
    SELECT 1 FROM close_retirement_resources proof
    WHERE NEW.absence_basis = 'same_attempt_prior_retirement'
      AND proof.attempt_id = NEW.attempt_id
      AND proof.scope = NEW.scope
      AND proof.inspection_generation = NEW.inspection_generation
      AND proof.inspection_fingerprint = NEW.inspection_fingerprint
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND proof.proof_kind = 'retired'
    UNION ALL
    SELECT 1 FROM close_retirement_resource_dispatches dispatch
    WHERE NEW.absence_basis = 'same_attempt_prior_retirement'
      AND dispatch.attempt_id = NEW.attempt_id
      AND dispatch.scope = NEW.scope
      AND dispatch.inspection_generation = NEW.inspection_generation
      AND dispatch.inspection_fingerprint = NEW.inspection_fingerprint
      AND dispatch.resource_kind = NEW.resource_kind
      AND dispatch.identity_kind = NEW.identity_kind
      AND dispatch.identity_codec = NEW.identity_codec
      AND dispatch.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'adopted absence requires exact retained proof');
END;

DROP TRIGGER close_retirement_resources_require_absence_proof_on_update;
CREATE TRIGGER close_retirement_resources_require_absence_proof_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN NEW.proof_kind = 'absence_adopted'
 AND (
     OLD.proof_kind <> 'absence_adopted' OR OLD.absence_basis IS NOT NEW.absence_basis
     OR OLD.attempt_id <> NEW.attempt_id OR OLD.scope <> NEW.scope
     OR OLD.inspection_generation <> NEW.inspection_generation
     OR OLD.inspection_fingerprint <> NEW.inspection_fingerprint
     OR OLD.resource_kind <> NEW.resource_kind OR OLD.identity_kind <> NEW.identity_kind
     OR OLD.identity_codec <> NEW.identity_codec OR OLD.identity_value <> NEW.identity_value
 )
 AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    WHERE proof.scope = NEW.scope
      AND proof.resource_kind = NEW.resource_kind
      AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec
      AND proof.identity_value = NEW.identity_value
      AND NEW.absence_basis = 'preexisting_exact_identity_evidence'
      AND proof.attempt_id <> NEW.attempt_id
      AND EXISTS (
          SELECT 1
          FROM close_attempt_members proof_member
          JOIN conversations proof_conversation ON proof_conversation.id = proof_member.conversation_id
          JOIN close_attempt_members current_member ON current_member.attempt_id = NEW.attempt_id
          JOIN conversations current_conversation ON current_conversation.id = current_member.conversation_id
          WHERE proof_member.attempt_id = proof.attempt_id
            AND proof_conversation.product_conversation_id = current_conversation.product_conversation_id
      )
      AND proof.proof_kind IN ('retired', 'absence_adopted')
    UNION ALL
    SELECT 1 FROM close_retirement_resources proof
    WHERE NEW.absence_basis = 'same_attempt_prior_retirement'
      AND proof.attempt_id = NEW.attempt_id AND proof.scope = NEW.scope
      AND proof.inspection_generation = NEW.inspection_generation
      AND proof.inspection_fingerprint = NEW.inspection_fingerprint
      AND proof.resource_kind = NEW.resource_kind AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec AND proof.identity_value = NEW.identity_value
      AND proof.proof_kind = 'retired'
    UNION ALL
    SELECT 1 FROM close_retirement_resource_dispatches dispatch
    WHERE NEW.absence_basis = 'same_attempt_prior_retirement'
      AND dispatch.attempt_id = NEW.attempt_id AND dispatch.scope = NEW.scope
      AND dispatch.inspection_generation = NEW.inspection_generation
      AND dispatch.inspection_fingerprint = NEW.inspection_fingerprint
      AND dispatch.resource_kind = NEW.resource_kind AND dispatch.identity_kind = NEW.identity_kind
      AND dispatch.identity_codec = NEW.identity_codec AND dispatch.identity_value = NEW.identity_value
)
BEGIN
    SELECT RAISE(ABORT, 'adopted absence requires exact retained proof');
END;

";

const MIGRATION_082: &str = r"
DROP TRIGGER close_obligations_transition_graph;
CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('awaiting_retirement_inspection', 'needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;
";

const MIGRATION_084: &str = r"
CREATE TABLE close_worktree_cleanup_plans (
    attempt_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    inspection_generation TEXT NOT NULL,
    inspection_fingerprint TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind = 'worktree'),
    identity_kind TEXT NOT NULL CHECK (identity_kind = 'worktree'),
    identity_codec TEXT NOT NULL,
    identity_value TEXT NOT NULL,
    administrative_dir_codec TEXT NOT NULL CHECK (administrative_dir_codec = 'hex_path_v1'),
    administrative_dir_value TEXT NOT NULL CHECK (length(administrative_dir_value) > 0),
    planned_at_us INTEGER NOT NULL
        CHECK (typeof(planned_at_us) = 'integer' AND planned_at_us >= 0),
    PRIMARY KEY (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ),
    FOREIGN KEY (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ) REFERENCES close_retirement_resource_dispatches (
        attempt_id, scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_value
    ) ON DELETE RESTRICT
);
";

const MIGRATION_083: &str = r"
DROP TRIGGER close_obligations_transition_graph;
CREATE TRIGGER close_obligations_transition_graph
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NOT (
    (OLD.phase = 'awaiting_blocker_resolution' AND NEW.phase IN ('awaiting_stop_work_confirmation', 'settling_active_work', 'completed'))
    OR (OLD.phase = 'awaiting_stop_work_confirmation' AND NEW.phase IN ('settling_active_work', 'completed'))
    OR (OLD.phase = 'settling_active_work' AND NEW.phase IN ('cancel_requested_during_settlement', 'awaiting_retirement_inspection'))
    OR (OLD.phase = 'cancel_requested_during_settlement' AND NEW.phase = 'completed')
    OR (OLD.phase = 'awaiting_retirement_inspection' AND NEW.phase IN ('awaiting_loss_confirmation', 'retirement_requested', 'needs_repair', 'completed'))
    OR (OLD.phase = 'awaiting_loss_confirmation' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
    OR (OLD.phase = 'retirement_requested' AND NEW.phase IN ('awaiting_retirement_inspection', 'needs_repair', 'completed'))
    OR (OLD.phase = 'needs_repair' AND NEW.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'completed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid close obligation phase transition');
END;
";

const MIGRATION_085: &str = r"
DROP TRIGGER close_obligations_invalidate_inspection_on_reentry;
CREATE TRIGGER close_obligations_invalidate_inspection_on_reentry
AFTER UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'awaiting_retirement_inspection'
  AND OLD.phase <> 'awaiting_retirement_inspection'
  AND OLD.phase <> 'needs_repair'
BEGIN
    DELETE FROM close_retirement_inspections WHERE attempt_id = NEW.attempt_id;
    UPDATE close_obligations
    SET inspection_generation = NULL, inspection_fingerprint = NULL
    WHERE attempt_id = NEW.attempt_id;
END;
";

const MIGRATION_088: &str = r"
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN administrative_dir_incarnation TEXT NOT NULL DEFAULT 'legacy_unknown'
CHECK (length(administrative_dir_incarnation) > 0);
";

const MIGRATION_087: &str = r"
CREATE TRIGGER close_worktree_absence_requires_cleanup_plan_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NEW.resource_kind = 'worktree'
 AND NEW.proof_kind = 'absence_adopted'
 AND NEW.absence_basis = 'same_attempt_prior_retirement'
 AND NOT EXISTS (
    SELECT 1 FROM close_retirement_resources proof
    WHERE proof.attempt_id = NEW.attempt_id AND proof.scope = NEW.scope
      AND proof.inspection_generation = NEW.inspection_generation
      AND proof.inspection_fingerprint = NEW.inspection_fingerprint
      AND proof.resource_kind = NEW.resource_kind AND proof.identity_kind = NEW.identity_kind
      AND proof.identity_codec = NEW.identity_codec AND proof.identity_value = NEW.identity_value
      AND proof.proof_kind = 'retired'
 )
 AND NOT EXISTS (
    SELECT 1 FROM close_worktree_cleanup_plans plan
    WHERE plan.attempt_id = NEW.attempt_id AND plan.scope = NEW.scope
      AND plan.inspection_generation = NEW.inspection_generation
      AND plan.inspection_fingerprint = NEW.inspection_fingerprint
      AND plan.resource_kind = NEW.resource_kind AND plan.identity_kind = NEW.identity_kind
      AND plan.identity_codec = NEW.identity_codec AND plan.identity_value = NEW.identity_value
 )
BEGIN
    SELECT RAISE(ABORT, 'adopted worktree absence requires exact cleanup plan');
END;

CREATE TRIGGER close_worktree_absence_requires_cleanup_plan_on_update
BEFORE UPDATE ON close_retirement_resources
FOR EACH ROW
WHEN NEW.resource_kind = 'worktree'
 AND NEW.proof_kind = 'absence_adopted'
 AND NEW.absence_basis = 'same_attempt_prior_retirement'
 AND (
     OLD.proof_kind <> 'absence_adopted' OR OLD.absence_basis IS NOT NEW.absence_basis
     OR OLD.attempt_id <> NEW.attempt_id OR OLD.scope <> NEW.scope
     OR OLD.inspection_generation <> NEW.inspection_generation
     OR OLD.inspection_fingerprint <> NEW.inspection_fingerprint
     OR OLD.resource_kind <> NEW.resource_kind OR OLD.identity_kind <> NEW.identity_kind
     OR OLD.identity_codec <> NEW.identity_codec OR OLD.identity_value <> NEW.identity_value
 )
 AND NOT EXISTS (
    SELECT 1 FROM close_worktree_cleanup_plans plan
    WHERE plan.attempt_id = NEW.attempt_id AND plan.scope = NEW.scope
      AND plan.inspection_generation = NEW.inspection_generation
      AND plan.inspection_fingerprint = NEW.inspection_fingerprint
      AND plan.resource_kind = NEW.resource_kind AND plan.identity_kind = NEW.identity_kind
      AND plan.identity_codec = NEW.identity_codec AND plan.identity_value = NEW.identity_value
 )
BEGIN
    SELECT RAISE(ABORT, 'adopted worktree absence requires exact cleanup plan');
END;
";

const MIGRATION_086: &str = r"
DROP TRIGGER close_obligations_snapshot_matches_inspection_aggregate;
CREATE TRIGGER close_obligations_snapshot_matches_inspection_aggregate
BEFORE UPDATE OF inspection_generation, inspection_fingerprint ON close_obligations
FOR EACH ROW
WHEN (
    OLD.inspection_generation IS NOT NEW.inspection_generation
    OR OLD.inspection_fingerprint IS NOT NEW.inspection_fingerprint
) AND NOT (
    NEW.phase = 'completed'
    AND NEW.close_outcome = 'cancelled'
    AND NEW.inspection_generation IS NULL
    AND NEW.inspection_fingerprint IS NULL
) AND (
    OLD.phase <> 'awaiting_retirement_inspection'
    OR NEW.inspection_generation <> CASE
        WHEN EXISTS (
            SELECT 1 FROM close_retirement_inspections
            WHERE attempt_id = NEW.attempt_id
        ) THEN (
            SELECT 'v1' || COALESCE(GROUP_CONCAT(component, ''), '')
            FROM (
                SELECT generation,
                       LENGTH(CAST(scope AS BLOB)) || ':' || scope ||
                       LENGTH(CAST(generation AS BLOB)) || ':' || generation AS component
                FROM close_retirement_inspections
                WHERE attempt_id = NEW.attempt_id
                ORDER BY scope
            )
        )
        WHEN NEW.inspection_generation LIKE 'server_git_status_v2_retry_%'
            THEN NEW.inspection_generation
        ELSE 'no-worktree'
    END
    OR NEW.inspection_fingerprint <> CASE
        WHEN EXISTS (
            SELECT 1 FROM close_retirement_inspections
            WHERE attempt_id = NEW.attempt_id
        ) THEN (
            SELECT 'v1' || COALESCE(GROUP_CONCAT(component, ''), '')
            FROM (
                SELECT fingerprint,
                       LENGTH(CAST(scope AS BLOB)) || ':' || scope ||
                       LENGTH(CAST(fingerprint AS BLOB)) || ':' || fingerprint AS component
                FROM close_retirement_inspections
                WHERE attempt_id = NEW.attempt_id
                ORDER BY scope
            )
        ) ELSE 'no-worktree'
    END
)
BEGIN
    SELECT RAISE(ABORT, 'close obligation snapshot must match atomic inspection replacement');
END;
";

const MIGRATION_090: &str = r"
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN final_tombstone_root_codec TEXT
CHECK (final_tombstone_root_codec IS NULL OR final_tombstone_root_codec = 'hex_path_v1');
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN final_tombstone_root_value TEXT;
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN final_tombstone_root_device TEXT;
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN final_tombstone_root_inode TEXT;
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN final_tombstone_object_device TEXT;
ALTER TABLE close_worktree_cleanup_plans
ADD COLUMN final_tombstone_object_inode TEXT;
CREATE TRIGGER close_worktree_final_tombstone_is_complete_on_insert
BEFORE INSERT ON close_worktree_cleanup_plans
FOR EACH ROW
WHEN (NEW.final_tombstone_root_codec IS NULL) <> (NEW.final_tombstone_root_value IS NULL)
  OR (NEW.final_tombstone_root_codec IS NULL) <> (NEW.final_tombstone_root_device IS NULL)
  OR (NEW.final_tombstone_root_codec IS NULL) <> (NEW.final_tombstone_root_inode IS NULL)
  OR (NEW.final_tombstone_root_value IS NOT NULL AND length(NEW.final_tombstone_root_value) = 0)
  OR (NEW.final_tombstone_root_device IS NOT NULL AND length(NEW.final_tombstone_root_device) = 0)
  OR (NEW.final_tombstone_root_inode IS NOT NULL AND length(NEW.final_tombstone_root_inode) = 0)
  OR (NEW.final_tombstone_object_device IS NULL) <> (NEW.final_tombstone_object_inode IS NULL)
  OR (NEW.final_tombstone_object_device IS NOT NULL AND NEW.final_tombstone_root_codec IS NULL)
  OR (NEW.final_tombstone_object_device IS NOT NULL AND length(NEW.final_tombstone_object_device) = 0)
  OR (NEW.final_tombstone_object_inode IS NOT NULL AND length(NEW.final_tombstone_object_inode) = 0)
BEGIN
    SELECT RAISE(ABORT, 'final worktree tombstone binding must be complete');
END;
CREATE TRIGGER close_worktree_final_tombstone_is_complete_on_update
BEFORE UPDATE OF final_tombstone_root_codec, final_tombstone_root_value,
                 final_tombstone_root_device, final_tombstone_root_inode,
                 final_tombstone_object_device, final_tombstone_object_inode
ON close_worktree_cleanup_plans
FOR EACH ROW
WHEN (NEW.final_tombstone_root_codec IS NULL) <> (NEW.final_tombstone_root_value IS NULL)
  OR (NEW.final_tombstone_root_codec IS NULL) <> (NEW.final_tombstone_root_device IS NULL)
  OR (NEW.final_tombstone_root_codec IS NULL) <> (NEW.final_tombstone_root_inode IS NULL)
  OR (NEW.final_tombstone_root_value IS NOT NULL AND length(NEW.final_tombstone_root_value) = 0)
  OR (NEW.final_tombstone_root_device IS NOT NULL AND length(NEW.final_tombstone_root_device) = 0)
  OR (NEW.final_tombstone_root_inode IS NOT NULL AND length(NEW.final_tombstone_root_inode) = 0)
  OR (NEW.final_tombstone_object_device IS NULL) <> (NEW.final_tombstone_object_inode IS NULL)
  OR (NEW.final_tombstone_object_device IS NOT NULL AND NEW.final_tombstone_root_codec IS NULL)
  OR (NEW.final_tombstone_object_device IS NOT NULL AND length(NEW.final_tombstone_object_device) = 0)
  OR (NEW.final_tombstone_object_inode IS NOT NULL AND length(NEW.final_tombstone_object_inode) = 0)
BEGIN
    SELECT RAISE(ABORT, 'final worktree tombstone binding must be complete');
END;
";
const MIGRATION_089: &str = r"
DROP TRIGGER close_retirement_inventories_require_exact_snapshot;
CREATE TRIGGER close_retirement_inventories_require_exact_snapshot
BEFORE INSERT ON close_retirement_inventories
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND (
          (obligation.phase IN ('retirement_requested', 'needs_repair')
           AND obligation.inspection_generation = NEW.inspection_generation
           AND obligation.inspection_fingerprint = NEW.inspection_fingerprint)
          OR (obligation.phase = 'awaiting_retirement_inspection'
              AND obligation.inspection_generation IS NULL
              AND obligation.inspection_fingerprint IS NULL
              AND NEW.inspection_generation = 'no-worktree'
              AND NEW.inspection_fingerprint = 'no-worktree')
      )
)
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory requires the exact active snapshot');
END;

DROP TRIGGER close_retirement_inventories_require_allocated_worktree_before_seal;
CREATE TRIGGER close_retirement_inventories_require_allocated_worktree_before_seal
BEFORE UPDATE OF sealed ON close_retirement_inventories
FOR EACH ROW
WHEN OLD.sealed = 0
  AND NEW.sealed = 1
  AND EXISTS (
      SELECT 1
      FROM close_attempt_scopes target
      WHERE target.attempt_id = NEW.attempt_id
        AND target.scope = NEW.scope
        AND (
            (target.captured_worktree_identity IS NOT NULL AND (
                (SELECT COUNT(*) FROM close_expected_retirement_resources expected
                 WHERE expected.attempt_id = NEW.attempt_id
                   AND expected.scope = NEW.scope
                   AND expected.inspection_generation = NEW.inspection_generation
                   AND expected.inspection_fingerprint = NEW.inspection_fingerprint
                   AND expected.resource_kind = 'worktree'
                   AND expected.identity_kind = 'worktree'
                   AND expected.identity_codec = 'worktree_id_v1'
                   AND expected.identity_value = target.captured_worktree_identity) <> 1
            ))
            OR
            (target.captured_worktree_identity IS NULL AND EXISTS (
                SELECT 1 FROM close_expected_retirement_resources expected
                WHERE expected.attempt_id = NEW.attempt_id
                  AND expected.scope = NEW.scope
                  AND expected.inspection_generation = NEW.inspection_generation
                  AND expected.inspection_fingerprint = NEW.inspection_fingerprint
                  AND expected.resource_kind = 'worktree'
            ))
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'retirement inventory worktree rows must match the captured environment');
END;

DROP TRIGGER close_retirement_resources_require_authority_on_insert;
CREATE TRIGGER close_retirement_resources_require_authority_on_insert
BEFORE INSERT ON close_retirement_resources
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM close_obligations obligation
    WHERE obligation.attempt_id = NEW.attempt_id
      AND obligation.phase IN ('awaiting_retirement_inspection', 'retirement_requested', 'needs_repair')
      AND COALESCE(obligation.inspection_generation, 'no-worktree') = NEW.inspection_generation
      AND COALESCE(obligation.inspection_fingerprint, 'no-worktree') = NEW.inspection_fingerprint
)
BEGIN
    SELECT RAISE(ABORT, 'retirement evidence requires authorized phase and snapshot');
END;

DROP TRIGGER close_obligations_require_residual_before_needs_repair;
CREATE TRIGGER close_obligations_require_residual_before_needs_repair
BEFORE UPDATE OF phase ON close_obligations
FOR EACH ROW
WHEN NEW.phase = 'needs_repair'
  AND OLD.phase <> 'needs_repair'
  AND NOT EXISTS (
      SELECT 1 FROM close_retirement_resources resource
      WHERE resource.attempt_id = OLD.attempt_id
        AND resource.inspection_generation = COALESCE(NEW.inspection_generation, 'no-worktree')
        AND resource.inspection_fingerprint = COALESCE(NEW.inspection_fingerprint, 'no-worktree')
        AND resource.proof_kind = 'residual'
  )
BEGIN
    SELECT RAISE(ABORT, 'needs_repair requires current-snapshot residual evidence');
END;
";

const MIGRATION_091: &str = r"
PRAGMA writable_schema = ON;
UPDATE sqlite_schema
SET sql = replace(
    sql,
    'authority_kind IN (''restricted_explore'', ''work'')',
    'authority_kind IN (''direct'', ''restricted_explore'', ''work'')'
)
WHERE type = 'table' AND name = 'work_scopes';
PRAGMA writable_schema = RESET;

CREATE TABLE product_creation_jobs (
    request_id TEXT PRIMARY KEY CHECK (typeof(request_id) = 'text' AND trim(request_id) <> ''),
    product_conversation_id TEXT NOT NULL UNIQUE
        CHECK (typeof(product_conversation_id) = 'text' AND trim(product_conversation_id) <> ''),
    cwd TEXT NOT NULL CHECK (typeof(cwd) = 'text' AND trim(cwd) <> '' AND instr(cwd, char(0)) = 0),
    objective TEXT NOT NULL CHECK (typeof(objective) = 'text'),
    model TEXT CHECK (model IS NULL OR (typeof(model) = 'text' AND trim(model) <> '')),
    effort TEXT CHECK (effort IS NULL OR (typeof(effort) = 'text' AND trim(effort) <> '')),
    llm_language TEXT NOT NULL CHECK (llm_language IN ('phoenix-native', 'caveman')),
    status TEXT NOT NULL CHECK (status IN ('accepted', 'claimed', 'retry_scheduled', 'cancelling', 'cancelled', 'deletion_pending', 'delivery_pending', 'delivery_failed', 'published', 'failed', 'cleanup_ambiguous')),
    accepted_at_unix_micros INTEGER NOT NULL CHECK (typeof(accepted_at_unix_micros) = 'integer' AND accepted_at_unix_micros >= 0),
    updated_at_unix_micros INTEGER NOT NULL CHECK (typeof(updated_at_unix_micros) = 'integer' AND updated_at_unix_micros >= 0),
    attempt_count INTEGER NOT NULL DEFAULT 1 CHECK (typeof(attempt_count) = 'integer' AND attempt_count >= 1 AND attempt_count <= 4),
    claim_generation INTEGER NOT NULL DEFAULT 0 CHECK (typeof(claim_generation) = 'integer' AND claim_generation >= 0),
    claim_worker_id TEXT CHECK (claim_worker_id IS NULL OR (typeof(claim_worker_id) = 'text' AND trim(claim_worker_id) <> '')),
    claim_token TEXT CHECK (claim_token IS NULL OR (typeof(claim_token) = 'text' AND trim(claim_token) <> '')),
    claim_lease_until_unix_micros INTEGER CHECK (claim_lease_until_unix_micros IS NULL OR (typeof(claim_lease_until_unix_micros) = 'integer' AND claim_lease_until_unix_micros >= 0)),
    retry_at_unix_micros INTEGER CHECK (retry_at_unix_micros IS NULL OR (typeof(retry_at_unix_micros) = 'integer' AND retry_at_unix_micros >= 0)),
    cleanup_worker_id TEXT CHECK (cleanup_worker_id IS NULL OR (typeof(cleanup_worker_id) = 'text' AND trim(cleanup_worker_id) <> '')),
    cleanup_token TEXT CHECK (cleanup_token IS NULL OR (typeof(cleanup_token) = 'text' AND trim(cleanup_token) <> '')),
    cleanup_lease_until_unix_micros INTEGER CHECK (cleanup_lease_until_unix_micros IS NULL OR (typeof(cleanup_lease_until_unix_micros) = 'integer' AND cleanup_lease_until_unix_micros >= 0)),
    delivery_attempt_count INTEGER NOT NULL DEFAULT 1 CHECK (typeof(delivery_attempt_count) = 'integer' AND delivery_attempt_count >= 1 AND delivery_attempt_count <= 4),
    delivery_retry_at_unix_micros INTEGER CHECK (delivery_retry_at_unix_micros IS NULL OR (typeof(delivery_retry_at_unix_micros) = 'integer' AND delivery_retry_at_unix_micros >= 0)),
    pin_exact_checkout_oid TEXT CHECK (pin_exact_checkout_oid IS NULL OR (typeof(pin_exact_checkout_oid) = 'text' AND trim(pin_exact_checkout_oid) <> '')),
    pin_logical_base TEXT CHECK (pin_logical_base IS NULL OR (typeof(pin_logical_base) = 'text' AND trim(pin_logical_base) <> '')),
    pin_freshness TEXT CHECK (pin_freshness IS NULL OR pin_freshness = 'fresh'),
    staging_path TEXT CHECK (staging_path IS NULL OR (typeof(staging_path) = 'text' AND trim(staging_path) <> '' AND instr(staging_path, char(0)) = 0)),
    staging_repo_root TEXT CHECK (staging_repo_root IS NULL OR (typeof(staging_repo_root) = 'text' AND trim(staging_repo_root) <> '' AND instr(staging_repo_root, char(0)) = 0)),
    staging_exact_oid TEXT CHECK (staging_exact_oid IS NULL OR (typeof(staging_exact_oid) = 'text' AND trim(staging_exact_oid) <> '')),
    published_product_id TEXT UNIQUE REFERENCES product_conversations(id) ON DELETE CASCADE,
    published_conversation_id TEXT UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    last_error TEXT CHECK (last_error IS NULL OR typeof(last_error) = 'text'),
    cancelled_at_unix_micros INTEGER CHECK (cancelled_at_unix_micros IS NULL OR (typeof(cancelled_at_unix_micros) = 'integer' AND cancelled_at_unix_micros >= 0)),
    deletion_requested_at_unix_micros INTEGER CHECK (deletion_requested_at_unix_micros IS NULL OR (typeof(deletion_requested_at_unix_micros) = 'integer' AND deletion_requested_at_unix_micros >= 0)),
    CHECK ((pin_exact_checkout_oid IS NULL AND pin_logical_base IS NULL AND pin_freshness IS NULL)
        OR (pin_exact_checkout_oid IS NOT NULL AND pin_logical_base IS NOT NULL AND pin_freshness = 'fresh')),
    CHECK ((status = 'accepted' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status = 'claimed' AND claim_worker_id IS NOT NULL AND claim_token IS NOT NULL AND claim_lease_until_unix_micros IS NOT NULL AND retry_at_unix_micros IS NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status = 'retry_scheduled' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NOT NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status = 'cancelling' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status = 'cancelled' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND cancelled_at_unix_micros IS NOT NULL)
        OR (status = 'deletion_pending' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND deletion_requested_at_unix_micros IS NOT NULL)
        OR (status = 'delivery_pending' AND ((claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL) OR (claim_worker_id IS NOT NULL AND claim_token IS NOT NULL AND claim_lease_until_unix_micros IS NOT NULL)) AND retry_at_unix_micros IS NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND published_product_id IS NOT NULL AND published_conversation_id IS NOT NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status = 'delivery_failed' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND delivery_retry_at_unix_micros IS NULL AND published_product_id IS NOT NULL AND published_conversation_id IS NOT NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status = 'published' AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND published_product_id IS NOT NULL AND published_conversation_id IS NOT NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL)
        OR (status IN ('failed', 'cleanup_ambiguous') AND claim_worker_id IS NULL AND claim_token IS NULL AND claim_lease_until_unix_micros IS NULL AND retry_at_unix_micros IS NULL AND cleanup_worker_id IS NULL AND cleanup_token IS NULL AND cleanup_lease_until_unix_micros IS NULL AND published_product_id IS NULL AND published_conversation_id IS NULL AND cancelled_at_unix_micros IS NULL AND deletion_requested_at_unix_micros IS NULL))
);
CREATE INDEX product_creation_jobs_claim_order
    ON product_creation_jobs(status, accepted_at_unix_micros, request_id);
CREATE INDEX product_creation_jobs_retry_order
    ON product_creation_jobs(status, retry_at_unix_micros, accepted_at_unix_micros, request_id);
CREATE INDEX product_creation_jobs_cleanup_order
    ON product_creation_jobs(status, cleanup_lease_until_unix_micros, updated_at_unix_micros, request_id);
CREATE INDEX product_creation_jobs_delivery_order
    ON product_creation_jobs(status, delivery_retry_at_unix_micros, updated_at_unix_micros, request_id);
CREATE INDEX product_creation_jobs_published_cwd
    ON product_creation_jobs(status, cwd, updated_at_unix_micros DESC, request_id DESC);
CREATE TRIGGER product_creation_checkout_pin_immutable
BEFORE UPDATE OF pin_exact_checkout_oid, pin_logical_base, pin_freshness ON product_creation_jobs
WHEN OLD.pin_exact_checkout_oid IS NOT NULL AND (
    NEW.pin_exact_checkout_oid IS NULL
    OR NEW.pin_exact_checkout_oid <> OLD.pin_exact_checkout_oid
    OR NEW.pin_logical_base <> OLD.pin_logical_base
    OR NEW.pin_freshness <> OLD.pin_freshness
)
BEGIN
    SELECT RAISE(ABORT, 'product creation checkout pin is immutable after first set');
END;

CREATE TABLE product_creation_job_images (
    request_id TEXT NOT NULL REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (typeof(ordinal) = 'integer' AND ordinal >= 0),
    media_type TEXT NOT NULL CHECK (typeof(media_type) = 'text' AND trim(media_type) <> ''),
    data TEXT NOT NULL CHECK (typeof(data) = 'text' AND trim(data) <> ''),
    PRIMARY KEY (request_id, ordinal)
);

CREATE TABLE product_creation_resource_reservations (
    id TEXT PRIMARY KEY CHECK (typeof(id) = 'text' AND trim(id) <> ''),
    request_id TEXT NOT NULL REFERENCES product_creation_jobs(request_id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (typeof(generation) = 'integer' AND generation > 0),
    repository_identity TEXT NOT NULL CHECK (typeof(repository_identity) = 'text' AND trim(repository_identity) <> ''),
    resource_identity TEXT NOT NULL CHECK (typeof(resource_identity) = 'text' AND trim(resource_identity) <> ''),
    status TEXT NOT NULL CHECK (status IN ('reserved', 'present', 'cleanup_required', 'released', 'conflict')),
    created_at_unix_micros INTEGER NOT NULL CHECK (typeof(created_at_unix_micros) = 'integer' AND created_at_unix_micros >= 0),
    updated_at_unix_micros INTEGER NOT NULL CHECK (typeof(updated_at_unix_micros) = 'integer' AND updated_at_unix_micros >= 0),
    UNIQUE(request_id, resource_identity)
);
CREATE INDEX product_creation_resource_reservations_job
    ON product_creation_resource_reservations(request_id, status);

";

const MIGRATION_092: &str = r"ALTER TABLE conversation_creation_jobs ADD COLUMN exact_checkout_oid TEXT;
ALTER TABLE conversation_creation_jobs ADD COLUMN exact_checkout_logical_base TEXT;
CREATE TRIGGER conversation_creation_checkout_pin_insert
BEFORE INSERT ON conversation_creation_jobs
WHEN (NEW.exact_checkout_oid IS NULL) <> (NEW.exact_checkout_logical_base IS NULL)
  OR (NEW.exact_checkout_oid IS NOT NULL AND trim(NEW.exact_checkout_oid) = '')
  OR (NEW.exact_checkout_logical_base IS NOT NULL AND trim(NEW.exact_checkout_logical_base) = '')
BEGIN
    SELECT RAISE(ABORT, 'conversation creation checkout pin must be a non-empty pair');
END;
CREATE TRIGGER conversation_creation_checkout_pin_update
BEFORE UPDATE OF exact_checkout_oid, exact_checkout_logical_base ON conversation_creation_jobs
WHEN (NEW.exact_checkout_oid IS NULL) <> (NEW.exact_checkout_logical_base IS NULL)
  OR (NEW.exact_checkout_oid IS NOT NULL AND trim(NEW.exact_checkout_oid) = '')
  OR (NEW.exact_checkout_logical_base IS NOT NULL AND trim(NEW.exact_checkout_logical_base) = '')
  OR (OLD.exact_checkout_oid IS NOT NULL AND (
        NEW.exact_checkout_oid IS NULL
        OR NEW.exact_checkout_oid <> OLD.exact_checkout_oid
        OR NEW.exact_checkout_logical_base <> OLD.exact_checkout_logical_base
      ))
BEGIN
    SELECT RAISE(ABORT, 'conversation creation checkout pin must be a non-empty immutable pair');
END;
ALTER TABLE product_conversation_sources ADD COLUMN approved_title TEXT;
ALTER TABLE product_conversation_sources ADD COLUMN approved_priority TEXT;
ALTER TABLE product_conversation_sources ADD COLUMN approved_artifact_body TEXT;
ALTER TABLE product_conversation_sources ADD COLUMN approved_task_title TEXT;
ALTER TABLE product_conversation_sources ADD COLUMN approved_plan TEXT;
ALTER TABLE product_conversation_sources ADD COLUMN approved_task_file TEXT;
CREATE TABLE approved_task_creation_bindings (
    job_id TEXT PRIMARY KEY REFERENCES conversation_creation_jobs(id) ON DELETE CASCADE,
    source_product_conversation_id TEXT NOT NULL REFERENCES product_conversations(id),
    source_conversation_id TEXT NOT NULL CHECK (trim(source_conversation_id) <> ''),
    task_id TEXT NOT NULL CHECK (trim(task_id) <> ''), task_title TEXT NOT NULL CHECK (trim(task_title) <> ''),
    approved_title TEXT NOT NULL CHECK (trim(approved_title) <> ''), approved_priority TEXT NOT NULL CHECK (trim(approved_priority) <> ''),
    approved_plan TEXT NOT NULL, approved_task_file TEXT NOT NULL CHECK (trim(approved_task_file) <> ''), approved_artifact_body TEXT NOT NULL,
    UNIQUE(source_product_conversation_id, task_id)
);
CREATE TABLE conversation_approved_task_objectives (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL CHECK (trim(task_id) <> ''), task_title TEXT NOT NULL CHECK (trim(task_title) <> ''),
    approved_title TEXT NOT NULL CHECK (trim(approved_title) <> ''), approved_priority TEXT NOT NULL CHECK (trim(approved_priority) <> ''),
    approved_plan TEXT NOT NULL, approved_task_file TEXT NOT NULL CHECK (trim(approved_task_file) <> ''), approved_artifact_body TEXT NOT NULL,
    created_at_us INTEGER NOT NULL CHECK (created_at_us >= 0)
);
CREATE TABLE work_scope_approved_task_authorities (
    work_scope_id TEXT PRIMARY KEY REFERENCES work_scopes(id) ON DELETE CASCADE,
    objective_conversation_id TEXT NOT NULL UNIQUE REFERENCES conversation_approved_task_objectives(conversation_id) ON DELETE CASCADE,
    created_at_us INTEGER NOT NULL CHECK (created_at_us >= 0)
);";

const MIGRATION_093: &str = r"ALTER TABLE product_creation_resource_reservations
ADD COLUMN ownership_token TEXT
CHECK (ownership_token IS NULL OR (
    typeof(ownership_token) = 'text'
    AND trim(ownership_token) <> ''
    AND instr(ownership_token, char(0)) = 0
));";

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use sqlx::Row;
    use std::str::FromStr;

    #[test]
    fn compiled_migration_digest_binds_version_name_and_sql_body() {
        let baseline =
            migration_digest_from_parts([(1, b"name".as_slice(), b"SELECT 1".as_slice())]);
        assert_ne!(
            baseline,
            migration_digest_from_parts([(1, b"name".as_slice(), b"SELECT 2".as_slice())])
        );
        assert_ne!(
            baseline,
            migration_digest_from_parts([(1, b"other".as_slice(), b"SELECT 1".as_slice())])
        );
        assert_ne!(
            baseline,
            migration_digest_from_parts([(2, b"name".as_slice(), b"SELECT 1".as_slice())])
        );
    }

    #[test]
    fn r1_expected_table_definitions_cover_every_migration_65_table() {
        assert_eq!(
            r1_expected_table_definitions()
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![
                "git_repositories",
                "git_repository_default_branch_observations",
                "git_repository_locator_observations",
                "work_scope_git_repositories",
            ]
        );
    }

    #[test]
    fn normalize_sql_preserves_quoted_literal_case() {
        assert_ne!(
            normalize_sql("CREATE TABLE examples (value TEXT CHECK (value = 'Present'))"),
            normalize_sql("CREATE TABLE examples (value TEXT CHECK (value = 'present'))")
        );
    }

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

    #[tokio::test]
    async fn migration_088_backfills_and_requires_admin_incarnation() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE close_worktree_cleanup_plans (
                 attempt_id TEXT PRIMARY KEY,
                 administrative_dir_value TEXT NOT NULL
             );
             INSERT INTO close_worktree_cleanup_plans (
                 attempt_id, administrative_dir_value
             ) VALUES ('legacy-attempt', '2f746d70');",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATION_088).execute(&pool).await.unwrap();

        let legacy_incarnation: String = sqlx::query_scalar(
            "SELECT administrative_dir_incarnation
             FROM close_worktree_cleanup_plans
             WHERE attempt_id = 'legacy-attempt'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(legacy_incarnation, "legacy_unknown");

        assert!(sqlx::query(
            "INSERT INTO close_worktree_cleanup_plans (
                 attempt_id, administrative_dir_value, administrative_dir_incarnation
             ) VALUES ('empty-incarnation', '2f746d70', '')",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::query(
            "INSERT INTO close_worktree_cleanup_plans (
                 attempt_id, administrative_dir_value, administrative_dir_incarnation
             ) VALUES ('bound-incarnation', '2f746d70', 'git_admin_dir_v1:1:2')",
        )
        .execute(&pool)
        .await
        .unwrap();
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

    async fn setup_pre_065_git_repository_schema(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 canonical_path TEXT UNIQUE NOT NULL,
                 main_ref TEXT NOT NULL DEFAULT 'main',
                 created_at TEXT NOT NULL
             );
             CREATE TABLE work_scopes (
                 id TEXT PRIMARY KEY
             );
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 work_scope_id TEXT REFERENCES work_scopes(id),
                 project_id TEXT REFERENCES projects(id)
             );",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn migration_058_adds_typed_effort_and_usage_columns() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE conversations (id TEXT PRIMARY KEY, model TEXT);\
             INSERT INTO conversations (id, model) VALUES ('legacy', 'gpt-5.3-codex');\
             CREATE TABLE turn_usage (\
                 id INTEGER PRIMARY KEY,\
                 conversation_id TEXT NOT NULL,\
                 root_conversation_id TEXT NOT NULL,\
                 model TEXT NOT NULL,\
                 input_tokens INTEGER NOT NULL DEFAULT 0,\
                 output_tokens INTEGER NOT NULL DEFAULT 0,\
                 cache_creation_tokens INTEGER NOT NULL DEFAULT 0,\
                 cache_read_tokens INTEGER NOT NULL DEFAULT 0,\
                 created_at TEXT NOT NULL,\
                 first_byte_at TEXT\
             );\
             INSERT INTO turn_usage (id, conversation_id, root_conversation_id, model, created_at)\
             VALUES (9, 'legacy', 'legacy', 'gpt-5.3-codex', '2026-01-01T00:00:00Z');\
             CREATE TABLE conversation_creation_jobs (id TEXT PRIMARY KEY, intent_json TEXT NOT NULL);\
             INSERT INTO conversation_creation_jobs (id, intent_json) VALUES ('job', '{\"model\":\"gpt-5.3-codex\"}');",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATION_058).execute(&pool).await.unwrap();

        let conversation_columns: Vec<String> = sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("name"))
            .collect();
        assert!(conversation_columns.iter().any(|column| column == "effort"));

        let usage_columns: Vec<String> = sqlx::query("PRAGMA table_info(turn_usage)")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get("name"))
            .collect();
        for expected in ["reasoning_tokens", "effort_source", "effort_level"] {
            assert!(usage_columns.iter().any(|column| column == expected));
        }

        assert!(sqlx::query(
            "INSERT INTO turn_usage (id, conversation_id, root_conversation_id, model, created_at) VALUES (1, 'legacy', 'legacy', 'gpt-5.4', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .is_err());
        let row = sqlx::query(
            "SELECT reasoning_tokens, effort_source, effort_level FROM turn_usage WHERE id = 9",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<Option<i64>, _>("reasoning_tokens"), None);
        assert_eq!(row.get::<String, _>("effort_source"), "native_unknown");
        assert_eq!(row.get::<Option<String>, _>("effort_level"), None);
        assert!(sqlx::query(
            "INSERT INTO turn_usage (id, conversation_id, root_conversation_id, model, created_at, effort_source, effort_level) VALUES (2, 'legacy', 'legacy', 'gpt-5.4', '2026-01-01T00:00:00Z', 'explicit', NULL)",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO turn_usage (id, conversation_id, root_conversation_id, model, created_at, effort_source, effort_level) VALUES (3, 'legacy', 'legacy', 'gpt-5.4', '2026-01-01T00:00:00Z', 'unsupported', 'high')",
        )
        .execute(&pool)
        .await
        .is_err());
        let migrated_model: String =
            sqlx::query_scalar("SELECT model FROM conversations WHERE id = 'legacy'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(migrated_model, "gpt-5.3-codex");
        let historical_turn_usage_model: String =
            sqlx::query_scalar("SELECT model FROM turn_usage WHERE id = 9")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(historical_turn_usage_model, "gpt-5.3-codex");
        let durable_job_intent: String = sqlx::query_scalar(
            "SELECT intent_json FROM conversation_creation_jobs WHERE id = 'job'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(durable_job_intent, r#"{"model":"gpt-5.3-codex"}"#);

        sqlx::query("INSERT INTO conversations (id) VALUES ('c')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(sqlx::query("UPDATE conversations SET effort = 'nonsense'")
            .execute(&pool)
            .await
            .is_err());
        assert!(
            sqlx::query("UPDATE turn_usage SET effort_source = 'nonsense' WHERE id = 9")
                .execute(&pool)
                .await
                .is_err()
        );
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

    type ConversationTopologyRow = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );

    async fn insert_close_admission(
        pool: &SqlitePool,
        attempt_id: &str,
        root_conversation_id: &str,
        timestamp: &str,
    ) {
        sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (?1, ?2, 'awaiting_blocker_resolution', ?3, ?3, NULL)",
        )
        .bind(attempt_id)
        .bind(root_conversation_id)
        .bind(timestamp)
        .execute(pool)
        .await
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_close_member(
        pool: &SqlitePool,
        attempt_id: &str,
        conversation_id: &str,
        member_role: &str,
        continuation_ordinal: i64,
        captured_continued_in_conv_id: Option<&str>,
        captured_state_kind: &str,
        captured_runtime_role: &str,
        captured_work_scope_id: Option<&str>,
        captured_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_continued_in_conv_id, captured_state_kind, captured_runtime_role,
                 captured_work_scope_id, captured_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(attempt_id)
        .bind(conversation_id)
        .bind(member_role)
        .bind(continuation_ordinal)
        .bind(captured_continued_in_conv_id)
        .bind(captured_state_kind)
        .bind(captured_runtime_role)
        .bind(captured_work_scope_id)
        .bind(captured_at)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_close_scope(
        pool: &SqlitePool,
        attempt_id: &str,
        scope: &str,
        captured_at: &str,
    ) {
        sqlx::query(
            "INSERT INTO close_attempt_scopes (
                 attempt_id, scope, captured_worktree_identity,
                 captured_worktree_fingerprint, captured_worktree_locator, captured_at
             )
             SELECT ?1, ?2, worktree_id, worktree_fingerprint,
                    CASE WHEN environment_kind = 'allocated_worktree'
                         THEN 'git_path_bytes_hex_v1:' || lower(hex(CAST(worktree_path AS BLOB)))
                         ELSE NULL END,
                    ?3
             FROM work_scopes WHERE id = ?2",
        )
        .bind(attempt_id)
        .bind(scope)
        .bind(captured_at)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn migration_065_creates_git_repository_shadow_tables_with_deterministic_seed() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES
                 ('project-a', '/repos/a', 'main', 1735689600000000),
                 ('project-b', '/repos/b', 'trunk', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (id) VALUES ('scope-a'), ('scope-b'), ('scope-c'), ('scope-empty')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, work_scope_id, project_id) VALUES
                 ('conv-a1', 'scope-a', 'project-a'),
                 ('conv-a2', 'scope-a', 'project-a'),
                 ('conv-b1', 'scope-b', 'project-b'),
                 ('conv-c1', 'scope-c', NULL),
                 ('conv-c2', 'scope-c', 'project-a')",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 65).await;

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);

        let repositories: Vec<String> =
            sqlx::query_scalar("SELECT id FROM git_repositories ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            repositories,
            vec!["project-a".to_string(), "project-b".to_string()]
        );

        let attachments: Vec<(String, String)> = sqlx::query_as(
            "SELECT work_scope_id, repository_id
             FROM work_scope_git_repositories
             ORDER BY work_scope_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            attachments,
            vec![
                ("scope-a".to_string(), "project-a".to_string()),
                ("scope-b".to_string(), "project-b".to_string()),
                ("scope-c".to_string(), "project-a".to_string()),
            ]
        );

        let unattached_scope_empty: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scope_git_repositories WHERE work_scope_id = 'scope-empty'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(unattached_scope_empty, 0);

        let locator_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM git_repository_locator_observations")
                .fetch_one(&pool)
                .await
                .unwrap();
        let default_branch_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM git_repository_default_branch_observations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            locator_count, 0,
            "migration must not backfill locator observations"
        );
        assert_eq!(
            default_branch_count, 0,
            "migration must not backfill default-branch observations from projects.main_ref"
        );
    }

    #[tokio::test]
    async fn migration_065_leaves_zero_project_scopes_unattached_and_replays_cleanly() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES
                 ('project-a', '/repos/a', 'main', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO work_scopes (id) VALUES ('scope-none'), ('scope-null-only')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, work_scope_id, project_id) VALUES
                 ('conv-null', 'scope-null-only', NULL),
                 ('conv-unscoped', NULL, 'project-a')",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 65).await;

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 0);

        let attachment_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM work_scope_git_repositories")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(attachment_count, 0);

        let repositories: Vec<String> =
            sqlx::query_scalar("SELECT id FROM git_repositories ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(repositories, vec!["project-a".to_string()]);

        let applied_versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _migrations WHERE version = 65")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(applied_versions, vec![65]);
    }

    #[tokio::test]
    async fn migration_065_rejects_whitespace_only_work_scope_identity_transactionally() {
        for (work_scope_id, conversation_id) in [
            ("   ", "conv-ascii-whitespace-scope"),
            ("\u{2003}", "conv-unicode-whitespace-scope"),
        ] {
            let pool = test_pool().await;
            setup_pre_065_git_repository_schema(&pool).await;
            sqlx::query(
                "INSERT INTO projects (id, canonical_path, main_ref, created_at)
                 VALUES ('project-a', '/repos/a', 'main', 1735689600000000)",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO work_scopes (id) VALUES (?1)")
                .bind(work_scope_id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO conversations (id, work_scope_id, project_id)
                 VALUES (?1, ?2, 'project-a')",
            )
            .bind(conversation_id)
            .bind(work_scope_id)
            .execute(&pool)
            .await
            .unwrap();
            stamp_migrations_except(&pool, 65).await;

            let error = run_pending_migrations(&pool).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("work_scope_git_repositories_work_scope_id_nonblank"),
                "unexpected migration failure for {work_scope_id:?}: {error}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'work_scope_git_repositories'"
                )
                .fetch_one(&pool)
                .await
                .unwrap(),
                0,
                "invalid WorkScope identity must roll back additive DDL"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version = 65")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                0,
                "failed migration must not stamp version 65"
            );
        }
    }

    #[tokio::test]
    async fn migration_065_rejects_empty_legacy_project_identity_transactionally() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at)
             VALUES ('', '/repos/empty-id', 'main', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 65).await;

        assert!(run_pending_migrations(&pool).await.is_err());

        let shadow_table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'git_repositories'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(shadow_table_count, 0, "failed migration must roll back DDL");

        let applied_65: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _migrations WHERE version = 65")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(applied_65, 0, "failed migration must not stamp version 65");
    }

    #[tokio::test]
    async fn migration_065_rejects_blob_legacy_project_identity_transactionally() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at)
             VALUES (X'70726f6a6563742d626c6f62', '/repos/blob-id', 'main', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 65).await;

        assert!(run_pending_migrations(&pool).await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'git_repositories'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0,
            "malformed legacy identity must roll back additive DDL"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version = 65")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "failed migration must not stamp version 65"
        );
    }

    #[tokio::test]
    async fn migration_065_fails_transactionally_when_one_scope_maps_to_multiple_projects() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        sqlx::query(
            "INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES
                 ('project-a', '/repos/a', 'main', 1735689600000000),
                 ('project-b', '/repos/b', 'main', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO work_scopes (id) VALUES ('scope-conflict')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, work_scope_id, project_id) VALUES
                 ('conv-a', 'scope-conflict', 'project-a'),
                 ('conv-b', 'scope-conflict', 'project-b')",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 65).await;

        let error = run_pending_migrations(&pool).await.unwrap_err();
        let DbError::GitRepositoryWorkScopeProjectConflict {
            work_scope_id,
            project_ids,
        } = error
        else {
            panic!("migration 65 must return a typed project conflict");
        };
        assert_eq!(work_scope_id.as_str(), "scope-conflict");
        assert_eq!(
            project_ids.map(|id| id.as_str().to_string()),
            ["project-a".to_string(), "project-b".to_string()]
        );

        let git_repositories_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'git_repositories'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            git_repositories_exists, 0,
            "failed migration must roll back additive tables"
        );

        let applied_65: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _migrations WHERE version = 65")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(applied_65, 0, "failed migration must not stamp version 65");
    }

    #[tokio::test]
    async fn migration_065_preflight_selects_the_exact_opaque_pair_independent_of_insert_order() {
        let fixtures = [
            [
                ("z-1", "scope-z", "repo,z"),
                ("z-2", "scope-z", "repo-m"),
                ("a-1", "scope-a", "repo,z"),
                ("a-2", "scope-a", "   "),
                ("a-3", "scope-a", "repo-m"),
            ],
            [
                ("a-3", "scope-a", "repo-m"),
                ("a-2", "scope-a", "   "),
                ("a-1", "scope-a", "repo,z"),
                ("z-2", "scope-z", "repo-m"),
                ("z-1", "scope-z", "repo,z"),
            ],
        ];

        for conversations in fixtures {
            let pool = test_pool().await;
            setup_pre_065_git_repository_schema(&pool).await;
            sqlx::query(
                "INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES
                 ('repo,z', '/repos/z', 'main', '2025-01-01'),
                 ('   ', '/repos/a', 'main', '2025-01-01'),
                 ('repo-m', '/repos/m', 'main', '2025-01-01')",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO work_scopes (id) VALUES ('scope-z'), ('scope-a')")
                .execute(&pool)
                .await
                .unwrap();
            for (id, scope_id, project_id) in conversations {
                sqlx::query(
                    "INSERT INTO conversations (id, work_scope_id, project_id) VALUES (?1, ?2, ?3)",
                )
                .bind(id)
                .bind(scope_id)
                .bind(project_id)
                .execute(&pool)
                .await
                .unwrap();
            }
            stamp_migrations_except(&pool, 65).await;

            let DbError::GitRepositoryWorkScopeProjectConflict {
                work_scope_id,
                project_ids,
            } = run_pending_migrations(&pool).await.unwrap_err()
            else {
                panic!("expected typed migration conflict");
            };
            assert_eq!(work_scope_id.as_str(), "scope-a");
            assert_eq!(
                project_ids.map(|id| id.as_str().to_string()),
                ["   ".to_string(), "repo,z".to_string()]
            );
            for table in [
                "git_repositories",
                "work_scope_git_repositories",
                "git_repository_locator_observations",
                "git_repository_default_branch_observations",
            ] {
                let exists: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                )
                .bind(table)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(exists, 0, "preflight rollback must leave no {table}");
            }
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version = 65")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                0
            );
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_065_text_primary_keys_are_explicitly_not_null_and_reject_nulls() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        stamp_migrations_except(&pool, 65).await;
        run_pending_migrations(&pool).await.unwrap();

        for (table, column) in [
            ("git_repositories", "id"),
            ("git_repository_locator_observations", "repository_id"),
            ("git_repository_locator_observations", "locator_kind"),
            (
                "git_repository_default_branch_observations",
                "repository_id",
            ),
            ("work_scope_git_repositories", "work_scope_id"),
        ] {
            let row = sqlx::query("SELECT name, \"notnull\", pk FROM pragma_table_info(?1)")
                .bind(table)
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .find(|row| row.get::<String, _>("name") == column)
                .unwrap();
            assert!(row.get::<i64, _>("pk") > 0, "{table}.{column} is a PK");
            assert_eq!(row.get::<i64, _>("notnull"), 1, "{table}.{column}");
        }
        assert!(MIGRATION_065.contains(
            "CREATE TEMP TABLE migration_065_scope_project_counts (\n    work_scope_id TEXT NOT NULL PRIMARY KEY"
        ));

        assert!(
            sqlx::query("INSERT INTO git_repositories (id) VALUES (NULL)")
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("INSERT INTO git_repositories (id) VALUES ('repo-1')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO work_scopes (id) VALUES ('scope-1')")
            .execute(&pool)
            .await
            .unwrap();
        for statement in [
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES (NULL, 1, 'unresolved', NULL, NULL, 1735689600000000)",
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id) VALUES (NULL, 'repo-1')",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES (NULL, 'common_dir', 'present', '/repo', 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-1', NULL, 'present', '/repo', 1735689600000000)",
        ] {
            assert!(sqlx::query(statement).execute(&pool).await.is_err(), "{statement}");
        }
    }

    #[tokio::test]
    async fn migration_065_rejects_blob_storage_for_every_persisted_text_field() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        stamp_migrations_except(&pool, 65).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::query("INSERT INTO git_repositories (id) VALUES ('repo-text')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO work_scopes (id) VALUES ('scope-text')")
            .execute(&pool)
            .await
            .unwrap();

        let mut connection = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        for statement in [
            "INSERT INTO git_repositories (id) VALUES (X'7265706f2d626c6f62')",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES (X'7265706f2d74657874', 'common_dir', 'present', '/repo', 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-locator-kind', X'636f6d6d6f6e5f646972', 'present', '/repo', 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-locator-status', 'common_dir', X'70726573656e74', '/repo', 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-locator-path', 'common_dir', 'present', X'2f7265706f', 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-locator-time', 'common_dir', 'present', '/repo', 'not-integer')",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES (X'7265706f2d74657874', 1, 'unresolved', NULL, NULL, 1735689600000000)",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-default-status', 1, X'7265736f6c766564', 'main', 'user_selected', 1735689600000000)",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-default-branch', 1, 'resolved', X'6d61696e', 'user_selected', 1735689600000000)",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-default-provenance', 1, 'resolved', 'main', X'757365725f73656c6563746564', 1735689600000000)",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-default-time', 1, 'unresolved', NULL, NULL, 'not-integer')",
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id) VALUES (X'73636f70652d74657874', 'repo-text')",
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id) VALUES ('scope-text', X'7265706f2d74657874')",
        ] {
            assert!(
                sqlx::query(statement).execute(&mut *connection).await.is_err(),
                "accepted non-text storage: {statement}"
            );
        }
    }

    #[tokio::test]
    async fn migration_065_rejects_nul_in_locator_paths_and_branch_names() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        stamp_migrations_except(&pool, 65).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO git_repositories (id) VALUES
             ('repo-nul-insert'), ('repo-nul-update')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let nul_path = "/repo\0nested";
        assert!(
            sqlx::query(
                "INSERT INTO git_repository_locator_observations (
                     repository_id, locator_kind, status, path, observed_at_unix_micros
                 ) VALUES ('repo-nul-insert', 'common_dir', 'present', ?1, 1735689600000000)",
            )
            .bind(nul_path)
            .execute(&pool)
            .await
            .is_err(),
            "locator path accepted embedded NUL"
        );
        sqlx::query(
            "INSERT INTO git_repository_locator_observations (
                 repository_id, locator_kind, status, path, observed_at_unix_micros
             ) VALUES ('repo-nul-update', 'common_dir', 'present', '/repo', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query(
                "UPDATE git_repository_locator_observations
                 SET path = ?1 WHERE repository_id = 'repo-nul-update'",
            )
            .bind(nul_path)
            .execute(&pool)
            .await
            .is_err(),
            "locator path update accepted embedded NUL"
        );

        let nul_branch = "ma\0in";
        assert!(
            sqlx::query(
                "INSERT INTO git_repository_default_branch_observations (
                     repository_id, generation, status, branch, provenance, observed_at_unix_micros
                 ) VALUES ('repo-nul-insert', 1, 'resolved', ?1, 'user_selected', 1735689600000000)",
            )
            .bind(nul_branch)
            .execute(&pool)
            .await
            .is_err(),
            "branch name accepted embedded NUL"
        );
        sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                 repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES ('repo-nul-update', 1, 'resolved', 'main', 'user_selected', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query(
                "UPDATE git_repository_default_branch_observations
                 SET branch = ?1 WHERE repository_id = 'repo-nul-update'",
            )
            .bind(nul_branch)
            .execute(&pool)
            .await
            .is_err(),
            "branch update accepted embedded NUL"
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn migration_065_enforces_unix_microsecond_observation_times() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        stamp_migrations_except(&pool, 65).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO git_repositories (id) VALUES
             ('repo-time-zero'),
             ('repo-time-later'),
             ('repo-time-invalid-locator-negative'),
             ('repo-time-invalid-locator-fraction'),
             ('repo-time-invalid-locator-text'),
             ('repo-time-invalid-branch-negative'),
             ('repo-time-invalid-branch-fraction'),
             ('repo-time-invalid-branch-text'),
             ('repo-time-update')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO git_repository_locator_observations (
                 repository_id, locator_kind, status, path, observed_at_unix_micros
             ) VALUES ('repo-time-zero', 'common_dir', 'present', '/zero', 0),
                      ('repo-time-later', 'common_dir', 'present', '/later', 1735689600000001)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                 repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES ('repo-time-zero', 1, 'unresolved', NULL, NULL, 0),
                      ('repo-time-later', 1, 'unresolved', NULL, NULL, 1735689600000001)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let ordered: Vec<i64> = sqlx::query_scalar(
            "SELECT observed_at_unix_micros
             FROM git_repository_locator_observations
             ORDER BY observed_at_unix_micros",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(ordered, vec![0, 1_735_689_600_000_001]);

        for (statement, failure_message) in [
            (
                "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-time-invalid-locator-negative', 'common_dir', 'present', '/invalid', -1)",
                "locator observation accepted a negative timestamp",
            ),
            (
                "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-time-invalid-locator-fraction', 'common_dir', 'present', '/invalid', 0.5)",
                "locator observation accepted a fractional timestamp",
            ),
            (
                "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-time-invalid-locator-text', 'common_dir', 'present', '/invalid', 'not-integer')",
                "locator observation accepted a text timestamp",
            ),
            (
                "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-time-invalid-branch-negative', 1, 'unresolved', NULL, NULL, -1)",
                "default-branch observation accepted a negative timestamp",
            ),
            (
                "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-time-invalid-branch-fraction', 1, 'unresolved', NULL, NULL, 0.5)",
                "default-branch observation accepted a fractional timestamp",
            ),
            (
                "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-time-invalid-branch-text', 1, 'unresolved', NULL, NULL, 'not-integer')",
                "default-branch observation accepted a text timestamp",
            ),
        ] {
            assert!(
                sqlx::query(statement).execute(&pool).await.is_err(),
                "{failure_message}"
            );
        }

        sqlx::query(
            "INSERT INTO git_repository_locator_observations (
                 repository_id, locator_kind, status, path, observed_at_unix_micros
             ) VALUES ('repo-time-update', 'common_dir', 'present', '/update', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE git_repository_locator_observations
                 SET observed_at_unix_micros = -1
                 WHERE repository_id = 'repo-time-update'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE git_repository_default_branch_observations
                 SET observed_at_unix_micros = 'not-integer'
                 WHERE repository_id = 'repo-time-zero'",
        )
        .execute(&pool)
        .await
        .is_err());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_065_enforces_git_repository_shadow_constraints() {
        let pool = test_pool().await;
        setup_pre_065_git_repository_schema(&pool).await;
        stamp_migrations_except(&pool, 65).await;
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);

        assert!(sqlx::query("INSERT INTO git_repositories (id) VALUES ('')")
            .execute(&pool)
            .await
            .is_err());

        sqlx::query("INSERT INTO git_repositories (id) VALUES ('repo-1'), ('repo-2')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO git_repositories (id) VALUES
             ('repo-generation-negative'), ('repo-generation-zero'),
             ('repo-generation-fraction'), ('repo-generation-text'),
             ('repo-shape-resolved'), ('repo-shape-unresolved')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO work_scopes (id) VALUES ('scope-1')")
            .execute(&pool)
            .await
            .unwrap();

        for (repository_id, locator_kind, status, path) in [
            ("repo-1", "common_dir", "present", "/repo/present"),
            ("repo-1", "management_root", "missing", "/repo/missing"),
            ("repo-2", "common_dir", "inaccessible", "/repo/inaccessible"),
        ] {
            sqlx::query(
                "INSERT INTO git_repository_locator_observations (
                     repository_id, locator_kind, status, path, observed_at_unix_micros
                 ) VALUES (?1, ?2, ?3, ?4, 1735689600000000)",
            )
            .bind(repository_id)
            .bind(locator_kind)
            .bind(status)
            .bind(path)
            .execute(&pool)
            .await
            .unwrap();
        }
        let locators: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT repository_id, locator_kind, status, path
             FROM git_repository_locator_observations
             ORDER BY repository_id, locator_kind",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            locators,
            vec![
                (
                    "repo-1".to_string(),
                    "common_dir".to_string(),
                    "present".to_string(),
                    "/repo/present".to_string()
                ),
                (
                    "repo-1".to_string(),
                    "management_root".to_string(),
                    "missing".to_string(),
                    "/repo/missing".to_string()
                ),
                (
                    "repo-2".to_string(),
                    "common_dir".to_string(),
                    "inaccessible".to_string(),
                    "/repo/inaccessible".to_string()
                ),
            ]
        );
        for statement in [
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-2', 'management_root', 'present', NULL, 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-2', 'management_root', 'present', '', 1735689600000000)",
            "INSERT INTO git_repository_locator_observations (repository_id, locator_kind, status, path, observed_at_unix_micros) VALUES ('repo-2', 'management_root', 'unknown', '/repo/unknown', 1735689600000000)",
        ] {
            assert!(sqlx::query(statement).execute(&pool).await.is_err(), "{statement}");
        }

        sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                 repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES ('repo-1', 3, 'resolved', 'main', 'remote_head_cache', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                 repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES ('repo-generation-negative', -1, 'unresolved', NULL, NULL, 1735689600000000)",
        )
        .execute(&pool)
        .await
        .is_err());
        for statement in [
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-generation-zero', 0, 'unresolved', NULL, NULL, 1735689600000000)",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-generation-fraction', 0.5, 'unresolved', NULL, NULL, 1735689600000000)",
            "INSERT INTO git_repository_default_branch_observations (repository_id, generation, status, branch, provenance, observed_at_unix_micros) VALUES ('repo-generation-text', 'not-a-generation', 'unresolved', NULL, NULL, 1735689600000000)",
        ] {
            assert!(
                sqlx::query(statement).execute(&pool).await.is_err(),
                "accepted invalid generation: {statement}"
            );
        }
        assert!(sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                 repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES ('repo-shape-resolved', 1, 'resolved', 'main', NULL, 1735689600000000)",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO git_repository_default_branch_observations (
                 repository_id, generation, status, branch, provenance, observed_at_unix_micros
             ) VALUES ('repo-shape-unresolved', 1, 'unresolved', 'main', 'remote_head_cache', 1735689600000000)",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::query(
            "INSERT INTO work_scope_git_repositories (work_scope_id, repository_id)
             VALUES ('scope-1', 'repo-2')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query("DELETE FROM git_repositories WHERE id = 'repo-2'")
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM work_scopes WHERE id = 'scope-1'")
            .execute(&pool)
            .await
            .unwrap();
        let attachment_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_scope_git_repositories WHERE work_scope_id = 'scope-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            attachment_count, 0,
            "scope delete must cascade attachment removal"
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn migration_064_creates_close_retirement_tables_and_preserves_chain_topology() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE work_scopes (
                 id TEXT PRIMARY KEY,
                 environment_kind TEXT NOT NULL DEFAULT 'none',
                 worktree_path TEXT
             );
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 work_scope_id TEXT,
                 continued_in_conv_id TEXT REFERENCES conversations(id),
                 parent_conversation_id TEXT,
                 user_initiated BOOLEAN NOT NULL DEFAULT 1,
                 state_kind TEXT,
                 runtime_role TEXT,
                 coordinator_head INTEGER NOT NULL DEFAULT 0,
                 archived BOOLEAN NOT NULL DEFAULT 0
             );
             CREATE TABLE conversation_creation_jobs (
                 conversation_id TEXT NOT NULL,
                 status TEXT NOT NULL,
                 deletion_requested_at TEXT
             );",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO work_scopes (id) VALUES ('scope-root'), ('scope-mid'), ('scope-next'), ('scope-solo')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, work_scope_id, continued_in_conv_id, state_kind, runtime_role, archived) VALUES
                ('root', 'scope-root', 'mid', 'idle', 'user', 1),
                ('mid', 'scope-mid', 'next', 'awaiting_continuation', 'sub_agent', 1),
                ('next', 'scope-next', NULL, 'completed', 'user', 0),
                ('solo', 'scope-solo', NULL, 'idle', 'user', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversation_creation_jobs (
                conversation_id, status, deletion_requested_at
             ) VALUES ('solo', 'deletion_pending', '2025-01-02T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 64).await;

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);

        let archived: Vec<(String, i64)> =
            sqlx::query_as("SELECT id, archived FROM conversations ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            archived,
            vec![
                ("mid".to_string(), 1),
                ("next".to_string(), 1),
                ("root".to_string(), 1),
                ("solo".to_string(), 0),
            ]
        );

        let topology: Vec<ConversationTopologyRow> = sqlx::query_as(
            "SELECT id, work_scope_id, continued_in_conv_id, state_kind, runtime_role FROM conversations ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            topology,
            vec![
                (
                    "mid".to_string(),
                    Some("scope-mid".to_string()),
                    Some("next".to_string()),
                    Some("awaiting_continuation".to_string()),
                    Some("sub_agent".to_string())
                ),
                (
                    "next".to_string(),
                    Some("scope-next".to_string()),
                    None,
                    Some("completed".to_string()),
                    Some("user".to_string())
                ),
                (
                    "root".to_string(),
                    Some("scope-root".to_string()),
                    Some("mid".to_string()),
                    Some("idle".to_string()),
                    Some("user".to_string())
                ),
                (
                    "solo".to_string(),
                    Some("scope-solo".to_string()),
                    None,
                    Some("idle".to_string()),
                    Some("user".to_string())
                ),
            ]
        );

        for table in [
            "close_obligations",
            "close_attempt_members",
            "close_attempt_scopes",
            "close_retirement_inspections",
            "close_retirement_losses",
            "close_retirement_inventories",
            "close_expected_retirement_resources",
            "close_retirement_resources",
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

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn migration_064_enforces_close_retirement_constraints() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE work_scopes (
                 id TEXT PRIMARY KEY,
                 environment_kind TEXT NOT NULL DEFAULT 'none',
                 worktree_path TEXT
             );
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 work_scope_id TEXT,
                 continued_in_conv_id TEXT REFERENCES conversations(id),
                 parent_conversation_id TEXT,
                 user_initiated BOOLEAN NOT NULL DEFAULT 1,
                 state_kind TEXT,
                 runtime_role TEXT,
                 coordinator_head INTEGER NOT NULL DEFAULT 0,
                 archived BOOLEAN NOT NULL DEFAULT 0
             );
             CREATE TABLE conversation_creation_jobs (
                 conversation_id TEXT NOT NULL,
                 status TEXT NOT NULL,
                 deletion_requested_at TEXT
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO work_scopes (id, environment_kind, worktree_path)
             VALUES
                 ('scope-a', 'allocated_worktree', '/tmp/wt'),
                 ('scope-b', 'none', NULL),
                 ('scope-c', 'none', NULL),
                 ('scope-nul', 'allocated_worktree', CAST(X'610062' AS TEXT)),
                 ('scope-partial', 'allocated_worktree', '/tmp/partial')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations (id, state_kind, runtime_role) VALUES
                 ('aaa-unsealed', 'idle', 'user'),
                 ('root', 'idle', 'user'),
                 ('other', 'completed', 'sub_agent'),
                 ('root-2', 'idle', 'user'),
                 ('other-2', 'completed', 'sub_agent'),
                 ('other-3', 'completed', 'sub_agent'),
                 ('other-4', 'completed', 'sub_agent'),
                 ('other-5', 'completed', 'sub_agent'),
                 ('other-6', 'completed', 'sub_agent'),
                 ('archived-owner', 'idle', 'user'),
                 ('nul-owner', 'idle', 'user'),
                 ('bulk-root-a', 'idle', 'user'),
                 ('bulk-root-b', 'idle', 'user'),
                 ('bulk-root-unsealed', 'idle', 'user'),
                 ('bulk-root-invalid', 'idle', 'user'),
                 ('preexisting-conflict', 'idle', 'user'),
                 ('partial-cancel-root', 'idle', 'user')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE conversations
             SET work_scope_id = 'scope-a', archived = 1
             WHERE id = 'archived-owner'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = 'scope-nul' WHERE id = 'nul-owner'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE conversations SET work_scope_id = 'scope-a' WHERE id = 'preexisting-conflict'",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 64).await;
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);

        sqlx::query(
            "INSERT INTO conversations (id, state_kind, runtime_role, work_scope_id)
             VALUES ('unattached-sub-agent', 'idle', 'sub_agent', NULL)",
        )
        .execute(&pool)
        .await
        .expect("typed unattached sub-agent must be representable");
        assert!(sqlx::query(
            "INSERT INTO conversations (id, state_kind, runtime_role, work_scope_id)
             VALUES ('unattached-user', 'idle', 'user', NULL)",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations SET runtime_role = 'coordinator', work_scope_id = 'scope-a'
             WHERE id = 'unattached-sub-agent'",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query("DELETE FROM conversations WHERE id = 'unattached-sub-agent'")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "UPDATE work_scopes
             SET worktree_id = id || '-worktree', worktree_fingerprint = id || '-fingerprint'
             WHERE environment_kind = 'allocated_worktree'",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_close_admission(&pool, "attempt-1", "root", "2025-01-01T00:00:00Z").await;
        insert_close_admission(
            &pool,
            "attempt-nul-scope",
            "nul-owner",
            "2025-01-01T00:00:00Z",
        )
        .await;
        insert_close_member(
            &pool,
            "attempt-nul-scope",
            "nul-owner",
            "root_latest",
            0,
            None,
            "idle",
            "user",
            Some("scope-nul"),
            "2025-01-01T00:00:01Z",
        )
        .await;
        let nul_scope_error = sqlx::query(
            "INSERT INTO close_attempt_scopes (
                 attempt_id, scope, captured_worktree_identity,
                 captured_worktree_fingerprint, captured_worktree_locator, captured_at
             ) VALUES (
                 'attempt-nul-scope', 'scope-nul',
                 (SELECT worktree_id FROM work_scopes WHERE id = 'scope-nul'),
                 (SELECT worktree_fingerprint FROM work_scopes WHERE id = 'scope-nul'),
                 'git_path_bytes_hex_v1:610062', '2025-01-01T00:00:02Z'
             )",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(nul_scope_error
            .to_string()
            .contains("Git path identity cannot contain a NUL byte"));
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 chronology_ordinal, attempt_id, root_conversation_id, phase,
                 created_at, updated_at
             ) VALUES (
                 9999, 'forged-chronology', 'bulk-root-a',
                 'awaiting_blocker_resolution',
                 '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        let bulk_admission_error = sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, topology_sealed,
                 created_at, updated_at
             ) VALUES
                 ('bulk-admission-valid', 'bulk-root-a', 'awaiting_blocker_resolution', 0,
                  '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'),
                 ('bulk-admission-invalid', 'bulk-root-invalid', 'completed', 0,
                  '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(bulk_admission_error
            .to_string()
            .contains("must begin at admission phase"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM close_obligations
                 WHERE attempt_id IN ('bulk-admission-valid', 'bulk-admission-invalid')",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
        for (attempt_id, root_id) in [
            ("bulk-attempt-a", "bulk-root-a"),
            ("bulk-attempt-b", "bulk-root-b"),
        ] {
            insert_close_admission(&pool, attempt_id, root_id, "2025-01-01T00:00:00Z").await;
            insert_close_member(
                &pool,
                attempt_id,
                root_id,
                "root_latest",
                0,
                None,
                "idle",
                "user",
                None,
                "2025-01-01T00:00:01Z",
            )
            .await;
            sqlx::query("UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = ?1")
                .bind(attempt_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        for phase in ["awaiting_stop_work_confirmation", "settling_active_work"] {
            sqlx::query(
                "UPDATE close_obligations SET phase = ?1 WHERE attempt_id = 'bulk-attempt-b'",
            )
            .bind(phase)
            .execute(&pool)
            .await
            .unwrap();
        }
        let bulk_phase_error = sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_stop_work_confirmation'
             WHERE attempt_id IN ('bulk-attempt-a', 'bulk-attempt-b')",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(bulk_phase_error
            .to_string()
            .contains("invalid close obligation phase transition"));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT phase FROM close_obligations WHERE attempt_id = 'bulk-attempt-a'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "awaiting_blocker_resolution"
        );
        insert_close_admission(
            &pool,
            "bulk-attempt-unsealed",
            "bulk-root-unsealed",
            "2025-01-01T00:00:00Z",
        )
        .await;
        let bulk_prerequisite_error = sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_stop_work_confirmation'
             WHERE attempt_id IN ('bulk-attempt-a', 'bulk-attempt-unsealed')",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(bulk_prerequisite_error
            .to_string()
            .contains("phase transition requires sealed topology"));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT phase FROM close_obligations WHERE attempt_id = 'bulk-attempt-a'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "awaiting_blocker_resolution"
        );
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (
                 'attempt-2', 'root', 'settling_active_work',
                 '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations SET root_conversation_id = 'other' WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT root_conversation_id FROM close_obligations WHERE attempt_id = 'attempt-1'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "root"
        );
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint,
                 created_at, updated_at, completed_at
             ) VALUES (
                 'bad-shape-1', 'other', 'awaiting_blocker_resolution', 'g1', 'fp1',
                 '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (
                 'bad-shape-2', 'other', 'retirement_requested',
                 '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "UPDATE conversations
             SET work_scope_id = 'scope-a', continued_in_conv_id = 'other-2'
             WHERE id = 'other'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET work_scope_id = 'scope-b' WHERE id = 'other-2'")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "UPDATE conversations
             SET state_kind = 'idle', runtime_role = 'user', user_initiated = 1
             WHERE id = 'other'",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_close_admission(&pool, "attempt-3", "other", "2025-01-01T00:00:00Z").await;
        insert_close_member(
            &pool,
            "attempt-3",
            "other",
            "root",
            0,
            Some("other-2"),
            "idle",
            "user",
            Some("scope-a"),
            "2025-01-01T00:00:20Z",
        )
        .await;
        insert_close_member(
            &pool,
            "attempt-3",
            "other-2",
            "latest",
            1,
            None,
            "completed",
            "sub_agent",
            Some("scope-b"),
            "2025-01-01T00:00:21Z",
        )
        .await;
        insert_close_scope(&pool, "attempt-3", "scope-a", "2025-01-01T00:00:30Z").await;
        insert_close_scope(&pool, "attempt-3", "scope-b", "2025-01-01T00:00:31Z").await;
        assert!(sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_state_kind, captured_runtime_role, captured_at
             ) VALUES (
                 'attempt-3', 'other-2', 'intermediate', 3,
                 'bogus', 'sub_agent', '2025-01-01T00:00:32Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query("UPDATE conversations SET state_kind = 'completed' WHERE id = 'other'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query("UPDATE conversations SET state_kind = 'idle' WHERE id = 'other'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let owner_reactivation_error =
            sqlx::query("UPDATE conversations SET archived = 0 WHERE id = 'archived-owner'")
                .execute(&pool)
                .await
                .unwrap_err();
        assert!(owner_reactivation_error
            .to_string()
            .contains("active Close prevents WorkScope owner reactivation"));
        sqlx::query(
            "UPDATE conversations SET runtime_role = 'sub_agent' WHERE id = 'archived-owner'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = 'archived-owner'")
            .execute(&pool)
            .await
            .unwrap();
        let owner_shape_bypass_error = sqlx::query(
            "UPDATE conversations SET runtime_role = 'user' WHERE id = 'archived-owner'",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(owner_shape_bypass_error
            .to_string()
            .contains("active Close prevents WorkScope owner reactivation"));
        for statement in [
            "UPDATE conversations SET user_initiated = 0 WHERE id = 'other'",
            "UPDATE conversations SET runtime_role = 'sub_agent' WHERE id = 'other'",
            "UPDATE conversations SET parent_conversation_id = 'other-2' WHERE id = 'other'",
        ] {
            let root_identity_error = sqlx::query(statement).execute(&pool).await.unwrap_err();
            assert!(root_identity_error
                .to_string()
                .contains("active Close preserves ProductConversation root identity"));
        }
        sqlx::query(
            "UPDATE conversations
             SET work_scope_id = 'scope-c'
             WHERE id IN ('aaa-unsealed', 'other')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let unsealed_scope: Option<String> =
            sqlx::query_scalar("SELECT work_scope_id FROM conversations WHERE id = 'aaa-unsealed'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unsealed_scope.as_deref(), Some("scope-c"));
        sqlx::query("UPDATE close_obligations SET phase = 'settling_active_work' WHERE attempt_id = 'attempt-3'")
            .execute(&pool)
            .await
            .unwrap();
        let settlement_fence_error =
            sqlx::query("UPDATE conversations SET work_scope_id = 'scope-b' WHERE id = 'other'")
                .execute(&pool)
                .await
                .unwrap_err();
        assert!(settlement_fence_error.to_string().contains("active Close"));
        for phase in ["awaiting_retirement_inspection"] {
            sqlx::query("UPDATE close_obligations SET phase = ?1 WHERE attempt_id = 'attempt-3'")
                .bind(phase)
                .execute(&pool)
                .await
                .unwrap();
        }

        insert_close_admission(
            &pool,
            "attempt-root-latest",
            "root-2",
            "2025-01-01T00:00:00Z",
        )
        .await;
        insert_close_member(
            &pool,
            "attempt-root-latest",
            "root-2",
            "root_latest",
            0,
            None,
            "idle",
            "user",
            None,
            "2025-01-01T00:01:00Z",
        )
        .await;
        sqlx::query("UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = 'attempt-root-latest'")
            .execute(&pool)
            .await
            .unwrap();

        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-root-latest', 'scope-a', 'g1', 'fp1', '2025-01-01T00:01:00Z')",
        )
        .execute(&pool)
        .await
        .is_err());

        insert_close_admission(
            &pool,
            "attempt-bad-scope-mismatch",
            "other-3",
            "2025-01-01T00:00:10Z",
        )
        .await;
        insert_close_member(
            &pool,
            "attempt-bad-scope-mismatch",
            "other-3",
            "root_latest",
            0,
            None,
            "completed",
            "sub_agent",
            Some("scope-a"),
            "2025-01-01T00:00:20Z",
        )
        .await;
        insert_close_scope(
            &pool,
            "attempt-bad-scope-mismatch",
            "scope-a",
            "2025-01-01T00:00:30Z",
        )
        .await;
        assert!(sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = 'attempt-bad-scope-mismatch'",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-3', 'scope-a', 'g1', 'fp1', '2025-01-01T00:01:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE close_retirement_inspections
             SET fingerprint = 'mutated-fp'
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_attempt_members
             SET member_role = 'latest'
             WHERE attempt_id = 'attempt-3' AND conversation_id = 'other'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_attempt_members
             SET captured_state_kind = 'completed', captured_at = '2030-01-01T00:00:00Z'
             WHERE attempt_id = 'attempt-3' AND conversation_id = 'other'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint,
                resource_kind, identity_kind, identity_codec, identity_value,
                proof_kind, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'stale-gen', 'stale-fp',
                'browser_session', 'opaque', 'opaque_string_v1', 'fabricated',
                'retired', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "DELETE FROM close_attempt_members
             WHERE attempt_id = 'attempt-3' AND conversation_id = 'other'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-3', 'scope-a', 'g2', 'fp2', '2025-01-01T00:02:00Z')",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-a', 'g1', 'staged_tracked_paths',
                 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:706174682f61'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_loss_confirmation',
                 inspection_generation = 'v17:scope-a2:g1',
                 inspection_fingerprint = 'v17:scope-a3:fp1'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_retirement_inspection'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE close_obligations
             SET phase = 'retirement_requested',
                 inspection_generation = 'v17:scope-a2:g1',
                 inspection_fingerprint = 'v17:scope-a3:fp1'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-3', 'scope-a', 'g1', 'fp1', '2025-01-01T00:02:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE close_retirement_inspections
             SET inspected_at = '2030-01-01T00:00:00Z'
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (
                 attempt_id, scope, generation, fingerprint, inspected_at
             ) VALUES ('attempt-3', 'scope-b', 'g1', 'fp1', '2025-01-01')",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-a', 'g1', 'staged_tracked_paths',
                 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:706174682f61'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let nul_loss_error = sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-a', 'g1', 'staged_tracked_paths',
                 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:610062'
             )",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(nul_loss_error
            .to_string()
            .contains("Git path identity cannot contain a NUL byte"));
        let nul_loss_update_error = sqlx::query(
            "UPDATE close_retirement_losses
             SET identity_value = 'git_path_bytes_hex_v1:00'
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a' AND generation = 'g1'",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(nul_loss_update_error
            .to_string()
            .contains("Git path identity cannot contain a NUL byte"));
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_loss_confirmation',
                 inspection_generation = 'v17:scope-a2:g1',
                 inspection_fingerprint = 'v17:scope-a3:fp1'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'retirement_requested'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            sqlx::query("DELETE FROM close_obligations WHERE attempt_id = 'attempt-3'",)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(sqlx::query(
            "DELETE FROM close_retirement_losses
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "DELETE FROM close_retirement_inspections
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations
             SET inspection_generation = 'v17:scope-a2:g0',
                 inspection_fingerprint = 'v17:scope-a3:fp0'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-a', 'g1', 'staged_tracked_paths',
                 'git_oid', 'hex', '1234567890123456789012345678901234567890'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-missing', 'g1', 'staged_tracked_paths',
                 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:706174682f62'
             )",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "UPDATE close_obligations SET phase = 'completed' WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_inventories (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 sealed, captured_at
             ) VALUES (
                 'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                 1, '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::query(
            "INSERT INTO close_retirement_inventories (
                 attempt_id, scope, inspection_generation, inspection_fingerprint, captured_at
             ) VALUES (
                 'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                 '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO close_expected_retirement_resources (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                 'worktree', 'git_path', 'git_path_bytes_hex_v1',
                 'git_path_bytes_hex_v1:A'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_retirement_inventories SET sealed = 1
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a' AND sealed = 0",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::raw_sql("SAVEPOINT duplicate_worktree")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO close_expected_retirement_resources (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
             ) VALUES
                 ('attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                  'worktree', 'worktree', 'worktree_id_v1',
                  'first-id'),
                 ('attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                  'worktree', 'worktree', 'worktree_id_v1',
                  'second-id'),
                 ('attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                  'equivalent_live_resource', 'opaque', 'opaque_string_v1',
                  'equivalent:1234567890123456789012345678901234567890')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql("ROLLBACK TO duplicate_worktree; RELEASE duplicate_worktree")
            .execute(&pool)
            .await
            .unwrap();
        assert!(sqlx::query(
            "UPDATE close_obligations SET phase = 'completed' WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::query(
            "INSERT INTO close_expected_retirement_resources (
                 attempt_id, scope, inspection_generation, inspection_fingerprint,
                 resource_kind, identity_kind, identity_codec, identity_value
             ) VALUES
                 ('attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                  'bash_process_group', 'opaque', 'opaque_string_v1', 'pg-9'),
                 ('attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1',
                  'worktree', 'worktree', 'worktree_id_v1', ?1)",
        )
        .bind(
            sqlx::query_scalar::<_, String>(
                "SELECT captured_worktree_identity FROM close_attempt_scopes
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
        )
        .execute(&pool)
        .await
        .unwrap();
        let distinct_owner_error = sqlx::query(
            "UPDATE close_retirement_inventories SET sealed = 1
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a' AND sealed = 0",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(distinct_owner_error
            .to_string()
            .contains("retained by distinct open aggregate"));
        sqlx::query("UPDATE conversations SET archived = 1 WHERE id = 'preexisting-conflict'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE close_retirement_inventories SET sealed = 1
             WHERE attempt_id = 'attempt-3' AND scope = 'scope-a' AND sealed = 0",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'worktree', 'worktree', 'worktree_id_v1', (SELECT captured_worktree_identity FROM close_attempt_scopes WHERE attempt_id = 'attempt-3' AND scope = 'scope-a'), 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'browser_session', 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:62726f77736572', 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'bash_process_group', 'opaque', 'opaque_string_v1', 'pg-9', 'residual', NULL, 'manual_repair_required', 'left alive',
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'browser_session', 'opaque', 'opaque_string_v1', 'browser-1', 'residual', NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'browser_session', 'opaque', 'opaque_string_v1', 'browser-2', 'retired', NULL, 'manual_repair_required',
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'browser_session', 'opaque', 'opaque_string_v1', 'browser-unproven',
                'absence_adopted', 'preexisting_exact_identity_evidence', NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_retirement_resources
             SET proof_kind = 'absence_adopted',
                 absence_basis = 'same_attempt_prior_retirement',
                 residual_reason = NULL
             WHERE attempt_id = 'attempt-3' AND identity_value = 'pg-9'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_retirement_resources
             SET inspection_generation = 'retagged', inspection_fingerprint = 'retagged'
             WHERE attempt_id = 'attempt-3' AND identity_value = 'pg-9'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_retirement_resources
             SET created_at = '2030-01-01T00:00:00Z'
             WHERE attempt_id = 'attempt-3' AND identity_value = 'pg-9'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_retirement_resources
             SET updated_at = 'not-rfc3339'
             WHERE attempt_id = 'attempt-3' AND identity_value = 'pg-9'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', completed_at = '2025-01-01T00:20:00Z'
             WHERE attempt_id = 'attempt-3'",
        )
        .execute(&pool)
        .await
        .is_err());

        insert_close_admission(
            &pool,
            "attempt-bad-role-shape",
            "other-4",
            "2025-01-01T00:01:00Z",
        )
        .await;
        insert_close_member(
            &pool,
            "attempt-bad-role-shape",
            "other-4",
            "root",
            0,
            Some("other"),
            "completed",
            "sub_agent",
            Some("scope-a"),
            "2025-01-01T00:01:00Z",
        )
        .await;
        insert_close_member(
            &pool,
            "attempt-bad-role-shape",
            "other",
            "latest",
            2,
            None,
            "completed",
            "sub_agent",
            None,
            "2025-01-01T00:01:01Z",
        )
        .await;
        assert!(sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal, captured_continued_in_conv_id,
                 captured_state_kind, captured_runtime_role, captured_work_scope_id, captured_at
             ) VALUES (
                 'attempt-bad-role-shape', 'other', 'root', 0, NULL, 'completed', 'sub_agent', NULL, '2025-01-01T00:01:02Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal, captured_continued_in_conv_id,
                 captured_state_kind, captured_runtime_role, captured_work_scope_id, captured_at
             ) VALUES (
                 'attempt-bad-role-shape', 'other', 'root', 0, NULL, 'completed', 'sub_agent', NULL, '2025-01-01T00:01:02Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal, captured_continued_in_conv_id,
                 captured_state_kind, captured_runtime_role, captured_work_scope_id, captured_at
             ) VALUES (
                 'attempt-root-latest', 'other-2', 'root', 0, NULL, 'completed', 'sub_agent', NULL, '2025-01-01T00:01:01Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal, captured_continued_in_conv_id,
                 captured_state_kind, captured_runtime_role, captured_work_scope_id, captured_at
             ) VALUES (
                 'attempt-root-latest', 'other-2', 'latest', 2, NULL, 'completed', 'sub_agent', NULL, '2025-01-01T00:01:02Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_attempt_scopes (
                 attempt_id, scope, captured_worktree_identity,
                 captured_worktree_fingerprint, captured_worktree_locator, captured_at
             ) VALUES (
                 'attempt-3', 'scope-a', 'wrong-id', 'wrong-fingerprint',
                 'git_path_bytes_hex_v1:2f746d702f7774', '2025-01-01T00:01:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (
                 'missing-created', 'other', 'awaiting_blocker_resolution',
                 NULL, '2025-01-01T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, created_at, updated_at, completed_at
             ) VALUES (
                 'missing-completed-at', 'other', 'completed',
                 '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "UPDATE conversations SET work_scope_id = 'scope-partial'
             WHERE id = 'partial-cancel-root'",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_close_admission(
            &pool,
            "partial-cancel",
            "partial-cancel-root",
            "2025-01-01T00:00:00Z",
        )
        .await;
        insert_close_member(
            &pool,
            "partial-cancel",
            "partial-cancel-root",
            "root_latest",
            0,
            None,
            "idle",
            "user",
            Some("scope-partial"),
            "2025-01-01T00:00:01Z",
        )
        .await;
        insert_close_scope(
            &pool,
            "partial-cancel",
            "scope-partial",
            "2025-01-01T00:00:02Z",
        )
        .await;
        sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1
             WHERE attempt_id = 'partial-cancel'",
        )
        .execute(&pool)
        .await
        .unwrap();
        for phase in ["awaiting_stop_work_confirmation", "settling_active_work"] {
            sqlx::query(
                "UPDATE close_obligations SET phase = ?1 WHERE attempt_id = 'partial-cancel'",
            )
            .bind(phase)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "UPDATE close_obligations SET phase = 'awaiting_retirement_inspection'
             WHERE attempt_id = 'partial-cancel'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_retirement_inspections (
                 attempt_id, scope, generation, fingerprint, inspected_at
             ) VALUES (
                 'partial-cancel', 'scope-partial', 'g1', 'fp1', '2025-01-01T00:00:03Z'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category,
                 identity_kind, identity_codec, identity_value
             ) VALUES (
                 'partial-cancel', 'scope-partial', 'g1', 'untracked_non_ignored_paths',
                 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:70617468'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_loss_confirmation',
                 inspection_generation = 'v113:scope-partial2:g1',
                 inspection_fingerprint = 'v113:scope-partial3:fp1'
             WHERE attempt_id = 'partial-cancel'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', close_outcome = 'cancelled',
                 completed_at = '2025-01-01T00:00:04Z',
                 inspection_generation = NULL, inspection_fingerprint = NULL
             WHERE attempt_id = 'partial-cancel'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let retained_inspections: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_retirement_inspections WHERE attempt_id = 'partial-cancel'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(retained_inspections, 1);

        insert_close_admission(&pool, "unsealed-phase", "other-6", "2025-01-04T00:00:00Z").await;
        assert!(sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_stop_work_confirmation'
             WHERE attempt_id = 'unsealed-phase'",
        )
        .execute(&pool)
        .await
        .is_err());
        insert_close_member(
            &pool,
            "attempt-1",
            "root",
            "root_latest",
            0,
            None,
            "idle",
            "user",
            None,
            "2025-01-01T00:09:00Z",
        )
        .await;
        sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_stop_work_confirmation'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE close_obligations SET chronology_ordinal = chronology_ordinal + 100
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations SET created_at = '2030-01-01T00:00:00Z'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(
            sqlx::query("UPDATE conversations SET archived = 1 WHERE id = 'root'")
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query(
            "UPDATE close_obligations SET phase = 'settling_active_work'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query("UPDATE conversations SET archived = 1 WHERE id = 'root'")
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query(
            "UPDATE close_obligations SET phase = 'awaiting_retirement_inspection'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', completed_at = '2025-01-01T00:10:00Z', close_outcome = 'cancelled'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE close_obligations SET completed_at = '2025-01-01T00:00:00 Z'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        for invalid_timestamp in ["2025-01-01T24:00:00Z", "2025-02-30T00:00:00Z"] {
            assert!(sqlx::query(
                "UPDATE close_obligations SET completed_at = ?1
                 WHERE attempt_id = 'attempt-1'",
            )
            .bind(invalid_timestamp)
            .execute(&pool)
            .await
            .is_err());
        }
        assert!(sqlx::query(
            "UPDATE close_obligations SET completed_at = '2025-01-01'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations SET completed_at = 'not-a-time'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE close_obligations
             SET completed_at = '2030-01-01T00:00:00Z'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT completed_at FROM close_obligations WHERE attempt_id = 'attempt-1'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "2025-01-01T00:10:00Z"
        );
        assert!(sqlx::query(
            "UPDATE close_obligations SET updated_at = 'malformed'
             WHERE attempt_id = 'attempt-1'",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "UPDATE conversations
             SET state_kind = 'idle', runtime_role = 'user', user_initiated = 1,
                 work_scope_id = 'scope-c'
             WHERE id = 'other-5'",
        )
        .execute(&pool)
        .await
        .unwrap();
        insert_close_admission(&pool, "attempt-4", "other-5", "2025-01-02T00:00:00Z").await;
        insert_close_member(
            &pool,
            "attempt-4",
            "other-5",
            "root_latest",
            0,
            None,
            "idle",
            "user",
            Some("scope-c"),
            "2025-01-02T00:00:20Z",
        )
        .await;
        insert_close_scope(&pool, "attempt-4", "scope-c", "2025-01-02T00:00:30Z").await;
        sqlx::query(
            "UPDATE close_obligations SET topology_sealed = 1 WHERE attempt_id = 'attempt-4'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'awaiting_stop_work_confirmation'
             WHERE attempt_id = 'attempt-4'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE close_obligations
             SET phase = 'completed', completed_at = '2025-01-02T00:05:00Z', close_outcome = 'cancelled'
             WHERE attempt_id = 'attempt-4'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint,
                 created_at, updated_at, completed_at
             ) VALUES (
                 'attempt-5', 'root', 'awaiting_retirement_inspection', 'g2', NULL,
                 '2025-01-03T00:00:00Z', '2025-01-03T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint,
                 created_at, updated_at, completed_at
             ) VALUES (
                 'early-with-inspection', 'other', 'awaiting_retirement_inspection', 'g9', 'fp9',
                 '2025-01-02T00:00:00Z', '2025-01-02T00:00:00Z', NULL
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, inspection_generation, inspection_fingerprint,
                 created_at, updated_at, completed_at
             ) VALUES (
                 'completed-with-inspection', 'other', 'completed', 'g10', 'fp10',
                 '2025-01-02T00:00:00Z', '2025-01-02T00:00:00Z', '2025-01-02T00:05:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-3', 'missing-scope', 'g1', 'fp1', '2025-01-02T00:01:00Z')",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-4', 'scope-b', NULL, NULL, '2025-01-02T00:01:00Z')",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-4', 'scope-a', 'g2', NULL, '2025-01-02T00:01:01Z')",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-missing', 'g1', 'fp1', 'worktree', 'opaque', 'opaque_string_v1', '/tmp/missing', 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'worktree', 'opaque', 'opaque_string_v1', '/tmp/empty-detail', 'retired', NULL, NULL, '',
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "UPDATE close_attempt_members
             SET captured_continued_in_conv_id = 'other', captured_at = '2025-01-01T00:01:03Z'
             WHERE attempt_id = 'attempt-3' AND conversation_id = 'other'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'worktree', 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:7372632f6c69622e7273', 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'equivalent_live_resource', 'opaque', 'opaque_string_v1', 'equivalent:1234567890123456789012345678901234567890', 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());

        for invalid_identity in [
            "git_path_bytes_hex_v1:",
            "git_path_bytes_hex_v1:/",
            "git_path_bytes_hex_v1:+",
            "git_path_bytes_hex_v1:",
            "git_path_bytes_hex_v1:a",
            "git_path_bytes_hex_v1:00",
        ] {
            assert!(sqlx::query(
                "INSERT INTO close_retirement_losses (
                     attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
                 ) VALUES (?1, ?2, ?3, 'unstaged_tracked_paths', 'git_path', 'git_path_bytes_hex_v1', ?4)",
            )
            .bind("attempt-3")
            .bind("scope-a")
            .bind("g1")
            .bind(invalid_identity)
            .execute(&pool)
            .await
            .is_err());

            assert!(sqlx::query(
                "INSERT INTO close_retirement_resources (
                    attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'worktree', 'git_path', 'git_path_bytes_hex_v1', ?3, 'retired', NULL, NULL, NULL,
                    '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
                 )",
            )
            .bind("attempt-3")
            .bind("scope-a")
            .bind(invalid_identity)
            .execute(&pool)
            .await
            .is_err());
        }

        let non_utf8_identity = "git_path_bytes_hex_v1:666f802fff";
        assert!(sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-a', 'g1', 'untracked_non_ignored_paths',
                 'git_path', 'git_path_bytes_hex_v1', ?1
             )",
        )
        .bind(non_utf8_identity)
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-a', 'v17:scope-a2:g1', 'v17:scope-a3:fp1', 'worktree', 'git_path', 'git_path_bytes_hex_v1', ?1, 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .bind(non_utf8_identity)
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "INSERT INTO close_retirement_resources (
                attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value, proof_kind, absence_basis, residual_reason, detail, created_at, updated_at
             ) VALUES (
                'attempt-3', 'scope-missing', 'g1', 'fp1', 'pty_session', 'opaque', 'opaque_string_v1', 'missing-target', 'retired', NULL, NULL, NULL,
                '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
             )",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
             VALUES ('attempt-3', 'scope-b', 'g9', 'fp9', '2025-01-01T00:02:00Z')",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "INSERT INTO close_retirement_losses (
                 attempt_id, scope, generation, category, identity_kind, identity_codec, identity_value
             ) VALUES (
                 'attempt-3', 'scope-b', 'g-missing', 'staged_tracked_paths',
                 'git_path', 'git_path_bytes_hex_v1', 'git_path_bytes_hex_v1:706174682d6d697373696e67'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn migration_061_projects_attachments_without_moving_scope_ownership() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE work_scopes (
                 id TEXT PRIMARY KEY,
                 worktree_path TEXT
             );
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 work_scope_id TEXT REFERENCES work_scopes(id),
                 continued_in_conv_id TEXT REFERENCES conversations(id)
             );
             CREATE TABLE bash_processes (
                 handle TEXT PRIMARY KEY,
                 work_scope_id TEXT NOT NULL REFERENCES work_scopes(id)
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO work_scopes (id, worktree_path) VALUES ('scope-a', '/repo/.phoenix/worktrees/a')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO conversations (id, work_scope_id) VALUES ('root', 'scope-a'), ('next', 'scope-a')")
            .execute(&pool).await.unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'next' WHERE id = 'root'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO bash_processes (handle, work_scope_id) VALUES ('bash-1', 'scope-a')",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 61).await;

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);

        let attachments: Vec<(String, String)> = sqlx::query_as(
            "SELECT conversation_id, work_scope_id FROM conversation_work_scope_attachments ORDER BY conversation_id",
        )
        .fetch_all(&pool).await.unwrap();
        assert_eq!(
            attachments,
            vec![
                ("next".to_string(), "scope-a".to_string()),
                ("root".to_string(), "scope-a".to_string()),
            ]
        );
        let successor: String =
            sqlx::query_scalar("SELECT continued_in_conv_id FROM conversations WHERE id = 'root'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(successor, "next");
        let scope: (String, String) =
            sqlx::query_as("SELECT id, worktree_path FROM work_scopes WHERE id = 'scope-a'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            scope,
            (
                "scope-a".to_string(),
                "/repo/.phoenix/worktrees/a".to_string()
            )
        );
        let bash_owner: String =
            sqlx::query_scalar("SELECT work_scope_id FROM bash_processes WHERE handle = 'bash-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(bash_owner, "scope-a");
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn migration_062_backfills_pending_steering_identities_as_legacy_unknown() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE conversations (id TEXT PRIMARY KEY);
             CREATE TABLE steering_messages (
                 message_id TEXT PRIMARY KEY,
                 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE
             );
             INSERT INTO conversations (id) VALUES ('legacy-conversation');
             INSERT INTO steering_messages (message_id, conversation_id)
             VALUES ('legacy-message', 'legacy-conversation');",
        )
        .execute(&pool)
        .await
        .unwrap();
        stamp_migrations_except(&pool, 62).await;

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);
        let fingerprint: Option<String> = sqlx::query_scalar(
            "SELECT request_fingerprint
             FROM steering_acceptance_receipts
             WHERE conversation_id = 'legacy-conversation'
               AND message_id = 'legacy-message'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fingerprint, None);
    }

    #[tokio::test]
    async fn migration_060_rejects_mismatched_state_kind_writes() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        stamp_migrations_except(&pool, 60).await;
        let applied = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(applied, 1);

        let mismatch = sqlx::query(
            "INSERT INTO conversations
             (id, state, state_kind, cwd, user_initiated, state_updated_at, created_at, updated_at)
             VALUES ('bad', '{\"type\":\"idle\"}', 'error', '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
        )
        .execute(&pool)
        .await;
        assert!(mismatch.is_err());
    }

    #[tokio::test]
    async fn migration_059_adds_and_backfills_state_kind() {
        let pool = test_pool().await;
        setup_conversations_table(&pool).await;
        sqlx::query("DROP INDEX IF EXISTS idx_conversations_state_kind")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE conversations DROP COLUMN state_kind")
            .execute(&pool)
            .await
            .unwrap();
        stamp_migrations_except(&pool, 59).await;

        for (id, state_json) in [
            ("idle", r#"{"type":"idle"}"#),
            (
                "awaiting-recovery",
                r#"{"type":"awaiting_recovery","resume_target":{"kind":"resume"}}"#,
            ),
            (
                "seeded",
                r#"{"type":"seeded_llm_requesting","seed_message_id":"m1","attempt":1}"#,
            ),
            ("terminal", r#"{"type":"terminal"}"#),
        ] {
            sqlx::query(
                "INSERT INTO conversations (id, state, cwd, user_initiated, state_updated_at, created_at, updated_at) \
                 VALUES (?1, ?2, '/tmp', 1, '2025-01-01', '2025-01-01', '2025-01-01')",
            )
            .bind(id)
            .bind(state_json)
            .execute(&pool)
            .await
            .unwrap();
        }

        let applied = run_pending_migrations(&pool).await.unwrap();
        assert_eq!(applied, 1);

        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, state_kind FROM conversations ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                ("awaiting-recovery".into(), "awaiting_recovery".into()),
                ("idle".into(), "idle".into()),
                ("seeded".into(), "seeded_llm_requesting".into()),
                ("terminal".into(), "terminal".into()),
            ]
        );

        let indexed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_conversations_state_kind'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(indexed, 1);
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
    async fn migration_066_quarantines_legacy_materialized_owners_without_fabricating_completion() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 state TEXT NOT NULL,
                 state_updated_at TEXT NOT NULL
             );
             CREATE TABLE durable_turns (
                 turn_id INTEGER PRIMARY KEY,
                 conversation_id TEXT NOT NULL,
                 generation INTEGER NOT NULL,
                 disposition TEXT NOT NULL,
                 canonical_message_id TEXT,
                 terminal_kind TEXT,
                 owns_conversation INTEGER NOT NULL
             );
             INSERT INTO conversations VALUES (
                 'legacy', '{\"type\":\"llm_requesting\",\"attempt\":1}', '2025-05-03T04:09:11Z'
             );
             INSERT INTO durable_turns VALUES (
                 661, 'legacy', 3, 'Runtime', 'canonical-user', NULL, 1
             );",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATION_066).execute(&pool).await.unwrap();

        let row: (String, String, String, i64) = sqlx::query_as(
            "SELECT terminal_kind, terminal_reason, target_state, expected_generation
             FROM direct_turn_terminal_obligations WHERE turn_id = 661",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "Failed");
        assert!(row.1.contains("exact terminal result"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&row.2).unwrap()["type"],
            "error"
        );
        assert_eq!(row.3, 3);
        let timestamp_us: i64 = sqlx::query_scalar(
            "SELECT target_state_updated_at_us FROM direct_turn_terminal_obligations WHERE turn_id = 661",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(timestamp_us, 1_746_245_351_000_000);
        let column: (String, i64) = sqlx::query_as(
            "SELECT type, \"notnull\" FROM pragma_table_info('direct_turn_terminal_obligations')
             WHERE name = 'target_state_updated_at_us'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(column, ("INTEGER".to_string(), 1));
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
    async fn migration_064_rooted_cycle_is_bounded_and_live_topology_still_rejects() {
        let pool = test_pool().await;
        sqlx::raw_sql(
            "CREATE TABLE work_scopes (
                 id TEXT PRIMARY KEY,
                 environment_kind TEXT NOT NULL DEFAULT 'none',
                 worktree_path TEXT
             );
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 continued_in_conv_id TEXT REFERENCES conversations(id),
                 state_kind TEXT,
                 runtime_role TEXT,
                 work_scope_id TEXT,
                 archived BOOLEAN NOT NULL DEFAULT 0
             );
             CREATE TABLE conversation_creation_jobs (
                 conversation_id TEXT NOT NULL,
                 status TEXT NOT NULL,
                 deletion_requested_at TEXT
             );",
        )
        .execute(&pool)
        .await
        .unwrap();
        for id in ["a", "b", "c", "d"] {
            sqlx::query(
                "INSERT INTO conversations (id, state_kind, runtime_role, archived)
                 VALUES (?1, 'idle', 'user', 0)",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'b' WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'c' WHERE id = 'b'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'b' WHERE id = 'c'")
            .execute(&pool)
            .await
            .unwrap();

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sqlx::raw_sql(MIGRATION_064).execute(&pool),
        )
        .await
        .expect("migration must not hang")
        .unwrap();

        let close_obligations_exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='close_obligations'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(
            close_obligations_exists.as_deref(),
            Some("close_obligations")
        );
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

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS _migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO _migrations (version, name)
             VALUES (70, 'temporarily_skip_product_conversation_migration'),
                    (71, 'temporarily_skip_product_conversation_lifecycle_correction'),
                    (72, 'temporarily_skip_product_conversation_lifecycle_retention'),
                    (73, 'temporarily_skip_recursive_subordinate_parent_invariant'),
                    (74, 'temporarily_skip_completed_continuation_handoffs'),
                    (91, 'temporarily_skip_product_creation_jobs'),
                    (92, 'temporarily_skip_creation_checkout_pin'),
                    (93, 'temporarily_skip_product_creation_ownership'),
                    (95, 'temporarily_skip_product_lifecycle_reconciliation')",
        )
        .execute(&pool)
        .await
        .unwrap();
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

    async fn setup_schema_before_migration_070(pool: &SqlitePool) {
        setup_legacy_conversations_table(pool).await;
        sqlx::raw_sql(
            "DROP TRIGGER IF EXISTS conversations_validate_product_parent_on_insert;
             DROP TRIGGER IF EXISTS conversations_validate_product_parent_on_update;",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE _migrations (
                 version INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             )",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO _migrations (version, name)
             VALUES (70, 'temporarily_skip_product_conversation_migration'),
                    (71, 'temporarily_skip_product_conversation_lifecycle_correction'),
                    (72, 'temporarily_skip_product_conversation_lifecycle_retention'),
                    (73, 'temporarily_skip_recursive_subordinate_parent_invariant'),
                    (74, 'temporarily_skip_completed_continuation_handoffs'),
                    (91, 'temporarily_skip_product_creation_jobs'),
                    (92, 'temporarily_skip_creation_checkout_pin'),
                    (93, 'temporarily_skip_product_creation_ownership'),
                    (95, 'temporarily_skip_product_lifecycle_reconciliation')",
        )
        .execute(pool)
        .await
        .unwrap();
        run_pending_migrations(pool).await.unwrap();
        sqlx::query("DELETE FROM _migrations WHERE version IN (70, 71, 72, 73)")
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_070_backfills_first_class_product_conversations_and_reanchors_close() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        sqlx::raw_sql(
            "INSERT INTO work_scopes (id, authority_kind, environment_kind, cwd, created_at, updated_at) VALUES ('scope-open', 'work', 'unowned_cwd', '/tmp', '2025-01-01', '2025-01-01'), ('scope-archived', 'work', 'unowned_cwd', '/tmp', '2025-01-01', '2025-01-01');
             INSERT INTO conversations (
                 id, slug, user_initiated, state, state_kind, state_updated_at,
                 created_at, updated_at, archived, runtime_role, parent_conversation_id,
                 continued_in_conv_id, work_scope_id
             ) VALUES
                 ('open-root', 'open-root', 1, '{\"type\":\"idle\"}', 'idle',
                  '2025-01-01', '2025-01-01', '2025-01-01', 1, 'user', NULL, 'open-latest', 'scope-open'),
                 ('open-latest', 'open-latest', 1, '{\"type\":\"idle\"}', 'idle',
                  '2025-01-01', '2025-01-01', '2025-01-01', 0, 'user', NULL, NULL, 'scope-open'),
                 ('worker', 'worker', 0, '{\"type\":\"idle\"}', 'idle',
                  '2025-01-01', '2025-01-01', '2025-01-01', 0, 'sub_agent', 'open-latest', NULL, 'scope-open'),
                 ('archived', 'archived', 1, '{\"type\":\"idle\"}', 'idle',
                  '2025-01-01', '2025-01-01', '2025-01-01', 1, 'user', NULL, NULL, 'scope-archived'),
                 ('coordinator', 'coordinator', 0, '{\"type\":\"idle\"}', 'idle',
                  '2025-01-01', '2025-01-01', '2025-01-01', 0, 'coordinator', NULL, NULL, NULL);
             INSERT INTO close_obligations (
                 attempt_id, root_conversation_id, phase, created_at, updated_at
             ) VALUES ('attempt-open', 'open-root', 'awaiting_blocker_resolution',
                       '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z');
             INSERT INTO close_attempt_members (
                 attempt_id, conversation_id, member_role, continuation_ordinal,
                 captured_continued_in_conv_id, captured_state_kind,
                 captured_runtime_role, captured_at
             ) VALUES
                 ('attempt-open', 'open-root', 'root', 0, 'open-latest', 'idle', 'user',
                  '2025-01-01T00:00:00Z'),
                 ('attempt-open', 'open-latest', 'latest', 1, NULL, 'idle', 'user',
                  '2025-01-01T00:00:00Z');",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 4);

        for blank_id in ["   ", "\t", "\n", " \t\n "] {
            assert!(sqlx::query(
                "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
                 VALUES (?1, 'ordinary', 'open')",
            )
            .bind(blank_id)
            .execute(&pool)
            .await
            .is_err());
        }

        let memberships: Vec<(String, String)> =
            sqlx::query_as("SELECT id, product_conversation_id FROM conversations ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(memberships.contains(&("open-root".into(), "open-root".into())));
        assert!(memberships.contains(&("open-latest".into(), "open-root".into())));
        assert!(memberships.contains(&("worker".into(), "open-root".into())));
        assert!(memberships.contains(&("archived".into(), "archived".into())));
        assert!(memberships.contains(&("coordinator".into(), "coordinator".into())));

        let aggregates: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, kind, ordinary_lifecycle FROM product_conversations ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(aggregates.contains(&("open-root".into(), "ordinary".into(), Some("open".into()))));
        assert!(aggregates.contains(&(
            "archived".into(),
            "ordinary".into(),
            Some("history".into())
        )));
        assert!(aggregates.contains(&("coordinator".into(), "coordinator".into(), None)));
        let synchronizers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'trigger' AND name LIKE 'product_conversation_lifecycle_%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(synchronizers, 0);
        let close_owner: String = sqlx::query_scalar(
            "SELECT product_conversation_id FROM close_obligations WHERE attempt_id = 'attempt-open'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(close_owner, "open-root");
        let member_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_attempt_members WHERE attempt_id = 'attempt-open'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(member_count, 2);
        let close_columns: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('close_obligations') ORDER BY cid",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(close_columns.contains(&"product_conversation_id".to_string()));
        assert!(!close_columns.contains(&"root_conversation_id".to_string()));
        sqlx::query("UPDATE conversations SET archived = 0 WHERE id = 'archived'")
            .execute(&pool)
            .await
            .unwrap();
        let dormant_lifecycle: String = sqlx::query_scalar(
            "SELECT ordinary_lifecycle FROM product_conversations WHERE id = 'archived'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dormant_lifecycle, "history");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_070_preserves_orphan_subagent_behind_hidden_tombstone() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        sqlx::raw_sql(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO conversations (
                 id, slug, user_initiated, state, state_kind, state_updated_at,
                 created_at, updated_at, archived, runtime_role, parent_conversation_id
             ) VALUES (
                 'orphan-worker', 'orphan-worker', 0, '{\"type\":\"idle\"}', 'idle',
                 '2025-01-01', '2025-01-01', '2025-01-01', 1, 'sub_agent', 'deleted-parent'
             );
             INSERT INTO messages (
                 message_id, conversation_id, sequence_id, message_type, content, created_at
             ) VALUES (
                 'orphan-message', 'orphan-worker', 1, 'assistant',
                 '{\"blocks\":[{\"type\":\"text\",\"text\":\"preserved\"}]}', '2025-01-01'
             );
             PRAGMA foreign_keys = ON;",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 4);
        let membership: String = sqlx::query_scalar(
            "SELECT product_conversation_id FROM conversations WHERE id = 'orphan-worker'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let tombstone_id = "legacy-orphan-parent-64656C657465642D706172656E74";
        assert_eq!(membership, tombstone_id);
        let parent: Option<String> = sqlx::query_scalar(
            "SELECT parent_conversation_id FROM conversations WHERE id = 'orphan-worker'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(parent.as_deref(), Some(tombstone_id));
        let role: (String, bool) = sqlx::query_as(
            "SELECT runtime_role, user_initiated FROM conversations
             WHERE id = 'orphan-worker'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role, ("sub_agent".to_string(), false));
        let aggregate: (String, Option<String>) = sqlx::query_as(
            "SELECT kind, ordinary_lifecycle FROM product_conversations
             WHERE id = ?1",
        )
        .bind(tombstone_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            aggregate,
            ("ordinary".to_string(), Some("history".to_string()))
        );
        let preserved_message: String =
            sqlx::query_scalar("SELECT content FROM messages WHERE message_id = 'orphan-message'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(preserved_message.contains("preserved"));
        let tombstone: (bool, String, String) = sqlx::query_as(
            "SELECT root.user_initiated, root.runtime_role, scope.lifecycle
             FROM legacy_orphan_subordinate_tombstones marker
             JOIN conversations root ON root.id = marker.root_conversation_id
             JOIN work_scopes scope ON scope.id = root.work_scope_id
             WHERE root.id = ?1",
        )
        .bind(tombstone_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            tombstone,
            (false, "user".to_string(), "retired".to_string())
        );
        let visible: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations
             WHERE product_conversation_id = ?1
               AND user_initiated = 1",
        )
        .bind(tombstone_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(visible, 0);
        let db = crate::Database::from_pool_for_tests(pool.clone(), String::new());
        assert!(db
            .list_conversations()
            .await
            .unwrap()
            .iter()
            .all(|conversation| conversation.product_conversation_id.as_str() != tombstone_id));
        assert!(db
            .list_archived_conversations()
            .await
            .unwrap()
            .iter()
            .all(|conversation| conversation.product_conversation_id.as_str() != tombstone_id));
    }

    #[tokio::test]
    async fn migration_070_rejects_legacy_forks_and_cycles_transactionally() {
        for invalid_topology in [
            "PRAGMA foreign_keys = OFF;
             INSERT INTO conversations (
                 id, runtime_role, continued_in_conv_id, work_scope_id, user_initiated,
                 state_updated_at, created_at, updated_at
             ) VALUES
                 ('fork-a', 'user', 'successor', 'scope-invalid', 1, '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('fork-b', 'user', 'successor', 'scope-invalid', 1, '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('successor', 'user', NULL, 'scope-invalid', 1, '2025-01-01', '2025-01-01', '2025-01-01');
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             INSERT INTO conversations (
                 id, runtime_role, continued_in_conv_id, work_scope_id, user_initiated,
                 state_updated_at, created_at, updated_at
             ) VALUES
                 ('cycle-a', 'user', 'cycle-b', 'scope-invalid', 1, '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('cycle-b', 'user', 'cycle-a', 'scope-invalid', 1, '2025-01-01', '2025-01-01', '2025-01-01');
             PRAGMA foreign_keys = ON;",
        ] {
            let pool = test_pool().await;
            setup_schema_before_migration_070(&pool).await;
            sqlx::query(
                "INSERT INTO work_scopes (
                     id, authority_kind, environment_kind, cwd, created_at, updated_at
                 ) VALUES (
                     'scope-invalid', 'work', 'unowned_cwd', '/tmp', '2025-01-01', '2025-01-01'
                 )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::raw_sql(invalid_topology)
                .execute(&pool)
                .await
                .unwrap();

            assert!(run_pending_migrations(&pool).await.is_err());
            assert!(!sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('conversations')
                     WHERE name = 'product_conversation_id'
                 )",
            )
            .fetch_one(&pool)
            .await
            .unwrap());
        }
    }

    #[tokio::test]
    async fn migration_070_rejects_invalid_membership_and_coordinator_lifecycle() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        run_pending_migrations(&pool).await.unwrap();

        assert!(sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES ('bad-coordinator', 'coordinator', 'open')",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES ('bad-ordinary', 'ordinary', NULL)",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES ('coordinator-only', 'coordinator', NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, runtime_role,
                 state_updated_at, created_at, updated_at
             ) VALUES (
                 'ordinary-in-coordinator', 'coordinator-only', 'user',
                 '2025-01-01', '2025-01-01', '2025-01-01'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO conversations (id, runtime_role) VALUES ('missing-member', 'user')",
        )
        .execute(&pool)
        .await
        .is_err());

        sqlx::raw_sql(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle) VALUES
                 ('ordinary-a', 'ordinary', 'open'),
                 ('ordinary-b', 'ordinary', 'open');
             INSERT INTO work_scopes (id, authority_kind, environment_kind, cwd, created_at, updated_at) VALUES ('scope-a', 'work', 'unowned_cwd', '/tmp/a', '2025-01-01', '2025-01-01'), ('scope-b', 'work', 'unowned_cwd', '/tmp/b', '2025-01-01', '2025-01-01');
             INSERT INTO conversations (
                 id, product_conversation_id, runtime_role, user_initiated, work_scope_id,
                 state_updated_at, created_at, updated_at
             ) VALUES
                 ('a', 'ordinary-a', 'user', 1, 'scope-a', '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('b', 'ordinary-b', 'user', 1, 'scope-b', '2025-01-01', '2025-01-01', '2025-01-01');",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query("UPDATE conversations SET continued_in_conv_id = 'b' WHERE id = 'a'",)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(sqlx::query(
            "UPDATE conversations SET runtime_role = 'coordinator' WHERE id = 'a'"
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE product_conversations SET kind = 'coordinator', ordinary_lifecycle = NULL
             WHERE id = 'ordinary-a'",
        )
        .execute(&pool)
        .await
        .is_err());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn migration_070_rejects_invalid_continuation_and_parent_topology() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        run_pending_migrations(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle) VALUES
                 ('ordinary-a', 'ordinary', 'open'),
                 ('ordinary-b', 'ordinary', 'open');
             INSERT INTO work_scopes (id, authority_kind, environment_kind, cwd, created_at, updated_at) VALUES ('scope-a', 'work', 'unowned_cwd', '/tmp/a', '2025-01-01', '2025-01-01'), ('scope-b', 'work', 'unowned_cwd', '/tmp/b', '2025-01-01', '2025-01-01');
             INSERT INTO conversations (
                 id, product_conversation_id, runtime_role, user_initiated, work_scope_id,
                 state_updated_at, created_at, updated_at
             ) VALUES
                 ('a', 'ordinary-a', 'user', 1, 'scope-a', '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('b', 'ordinary-b', 'user', 1, 'scope-b', '2025-01-01', '2025-01-01', '2025-01-01');",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, runtime_role, user_initiated, work_scope_id,
                 state_updated_at, created_at, updated_at
             ) VALUES (
                 'disconnected', 'ordinary-a', 'user', 1, 'scope-a',
                 '2025-01-01', '2025-01-01', '2025-01-01'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO product_continuation_reservations (
                 predecessor_conversation_id, successor_conversation_id,
                 product_conversation_id
             ) VALUES ('a', 'a-next', 'ordinary-a')",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("UPDATE conversations SET continued_in_conv_id = 'a-next' WHERE id = 'a'")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, runtime_role, user_initiated, work_scope_id,
                 state_updated_at, created_at, updated_at
             ) VALUES (
                 'a-next', 'ordinary-a', 'user', 1, 'scope-a',
                 '2025-01-01', '2025-01-01', '2025-01-01'
             )",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "DELETE FROM product_continuation_reservations
             WHERE predecessor_conversation_id = 'a'",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(
            sqlx::query("UPDATE conversations SET continued_in_conv_id = NULL WHERE id = 'a'",)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE conversations SET continued_in_conv_id = 'b' WHERE id = 'a'",)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(sqlx::query(
            "UPDATE conversations
             SET runtime_role = 'sub_agent', parent_conversation_id = 'a-next'
             WHERE id = 'a'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations
             SET runtime_role = 'sub_agent', parent_conversation_id = 'a'
             WHERE id = 'a-next'",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role,
                 user_initiated, state_updated_at, created_at, updated_at
             ) VALUES (
                 'a-fork', 'ordinary-a', 'a', 'sub_agent', 0,
                 '2025-01-01', '2025-01-01', '2025-01-01'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'a-next' WHERE id = 'a-fork'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'a' WHERE id = 'a-next'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = id WHERE id = 'a-next'",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role
             ) VALUES ('child', 'ordinary-b', 'a', 'sub_agent')",
        )
        .execute(&pool)
        .await
        .is_err());
        sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role,
                 user_initiated, state_updated_at, created_at, updated_at
             ) VALUES (
                 'valid-child', 'ordinary-a', 'a', 'sub_agent', 0,
                 '2025-01-01', '2025-01-01', '2025-01-01'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(sqlx::query(
            "UPDATE conversations
             SET runtime_role = 'sub_agent', parent_conversation_id = id
             WHERE id = 'b'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations SET continued_in_conv_id = 'a-fork'
             WHERE id = 'valid-child'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations
             SET runtime_role = 'sub_agent',
                 parent_conversation_id = 'a',
                 continued_in_conv_id = 'a-fork'
             WHERE id = 'a-next'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role,
                 user_initiated, state_updated_at, created_at, updated_at
             ) VALUES (
                 'recursive-child', 'ordinary-a', 'a-fork', 'sub_agent', 0,
                 '2025-01-01', '2025-01-01', '2025-01-01'
             )",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations SET parent_conversation_id = 'a-fork'
             WHERE id = 'valid-child'",
        )
        .execute(&pool)
        .await
        .is_err());

        assert!(sqlx::query(
            "UPDATE conversations SET parent_conversation_id = NULL
             WHERE id = 'valid-child'",
        )
        .execute(&pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE conversations SET parent_conversation_id = 'b'
             WHERE id = 'valid-child'",
        )
        .execute(&pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn migration_073_reparents_preexisting_recursive_subordinate_parent() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        sqlx::query(
            "INSERT INTO _migrations (version, name)
             VALUES (73, 'temporarily_skip_recursive_subordinate_parent_invariant')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 3);

        sqlx::raw_sql(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES ('ordinary-a', 'ordinary', 'open');
             INSERT INTO work_scopes (
                 id, authority_kind, environment_kind, cwd, created_at, updated_at
             ) VALUES ('scope-a', 'work', 'unowned_cwd', '/tmp/a', '2025-01-01', '2025-01-01');
             INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role,
                 user_initiated, work_scope_id, state_updated_at, created_at, updated_at
             ) VALUES
                 ('root', 'ordinary-a', NULL, 'user', 1, 'scope-a',
                  '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('first-subordinate', 'ordinary-a', 'root', 'sub_agent', 0, 'scope-a',
                  '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('recursive-subordinate', 'ordinary-a', 'first-subordinate', 'sub_agent', 0,
                  'scope-a', '2025-01-01', '2025-01-01', '2025-01-01');",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM _migrations WHERE version = 73")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);
        let parent: String = sqlx::query_scalar(
            "SELECT parent_conversation_id FROM conversations WHERE id = 'recursive-subordinate'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(parent, "root");
    }

    #[tokio::test]
    async fn migration_073_reparents_delimiter_bearing_legacy_ids() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        sqlx::query(
            "INSERT INTO _migrations (version, name)
             VALUES (73, 'temporarily_skip_recursive_subordinate_parent_invariant')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 3);

        sqlx::raw_sql(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES ('ordinary-a', 'ordinary', 'open');
             INSERT INTO work_scopes (
                 id, authority_kind, environment_kind, cwd, created_at, updated_at
             ) VALUES ('scope-a', 'work', 'unowned_cwd', '/tmp/a', '2025-01-01', '2025-01-01');
             INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role,
                 user_initiated, work_scope_id, state_updated_at, created_at, updated_at
             ) VALUES
                 ('root', 'ordinary-a', NULL, 'user', 1, 'scope-a',
                  '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('C|a', 'ordinary-a', 'root', 'sub_agent', 0, 'scope-a',
                  '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('a', 'ordinary-a', 'C|a', 'sub_agent', 0, 'scope-a',
                  '2025-01-01', '2025-01-01', '2025-01-01'),
                 ('C', 'ordinary-a', 'a', 'sub_agent', 0, 'scope-a',
                  '2025-01-01', '2025-01-01', '2025-01-01');",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM _migrations WHERE version = 73")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 1);
        let invalid_parents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM conversations child
             JOIN conversations parent ON parent.id = child.parent_conversation_id
             WHERE child.runtime_role = 'sub_agent'
               AND parent.runtime_role = 'sub_agent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(invalid_parents, 0);
    }

    #[tokio::test]
    async fn migration_073_rejects_unrecoverable_recursive_subordinate_cycle() {
        let pool = test_pool().await;
        setup_schema_before_migration_070(&pool).await;
        sqlx::query(
            "INSERT INTO _migrations (version, name)
             VALUES (73, 'temporarily_skip_recursive_subordinate_parent_invariant')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(run_pending_migrations(&pool).await.unwrap(), 3);

        sqlx::raw_sql(
            "INSERT INTO product_conversations (id, kind, ordinary_lifecycle)
             VALUES ('ordinary-a', 'ordinary', 'open');
             INSERT INTO work_scopes (
                 id, authority_kind, environment_kind, cwd, created_at, updated_at
             ) VALUES ('scope-a', 'work', 'unowned_cwd', '/tmp/a', '2025-01-01', '2025-01-01');
             INSERT INTO conversations (
                 id, product_conversation_id, parent_conversation_id, runtime_role,
                 user_initiated, work_scope_id, state_updated_at, created_at, updated_at
             ) VALUES ('self-cycle', 'ordinary-a', NULL, 'user', 1, 'scope-a',
                       '2025-01-01', '2025-01-01', '2025-01-01');
             UPDATE conversations
             SET runtime_role = 'sub_agent', parent_conversation_id = id
             WHERE id = 'self-cycle';",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM _migrations WHERE version = 73")
            .execute(&pool)
            .await
            .unwrap();
        assert!(run_pending_migrations(&pool).await.is_err());
        assert!(!sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM _migrations WHERE version = 73)",
        )
        .fetch_one(&pool)
        .await
        .unwrap());
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
