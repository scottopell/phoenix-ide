//! Migration 184 schema regressions. These start at the real shipped 183 schema;
//! no production guard is dropped to manufacture accepted capture authority.
use super::*;
use crate::close_foundation::{
    CaptureCloseRetirementInventoryRequest, CaptureCloseRetirementInventoryScopeRequest,
    RecordCloseRetirementEvidenceRequest, ReplaceCloseInspectionRequest,
};
use crate::Database;
use phoenix_core::domain::close::*;
use phoenix_core::work_scope::WorkScopeId;
use sqlx::sqlite::SqlitePoolOptions;

async fn predecessor() -> Database {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    setup_real_175_for_test(&pool).await;
    run_through_for_test(&pool, 183).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM _migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        183
    );
    Database::from_pool_for_tests(pool, String::new())
}

async fn trigger(db: &Database, name: &str) -> String {
    sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

fn assert_only_native_union_added(before: &str, after: &str, native: &str) {
    assert_eq!(before.matches(native).count(), 1);
    let (prefix, suffix) = before.split_once(native).unwrap();
    // Includes the complete original non-native authority arm, all independent
    // gates, and the unchanged RAISE body. The original native SELECT survives too.
    assert!(after.starts_with(&format!("{prefix}{native}\nUNION ALL\n")));
    assert!(after.ends_with(suffix));
}

#[tokio::test]
async fn real183_to184_preserves_original_branches_and_is_idempotent() {
    let db = predecessor().await;
    let native_queries = [
        (
            "close_retirement_resource_dispatches_require_authority",
            include_str!("native_close_retry_dispatch.sql").trim_end(),
        ),
        (
            "close_worktree_cleanup_plan_require_retry_authority",
            include_str!("native_close_retry_worktree_metadata.sql").trim_end(),
        ),
        (
            "close_worktree_tombstone_require_retry_authority",
            include_str!("native_close_retry_worktree_metadata.sql").trim_end(),
        ),
    ];
    let mut before = Vec::new();
    for (name, _) in native_queries {
        before.push(trigger(&db, name).await);
    }
    let untouched_names = [
        "close_run_retry_successes_require_running_plan",
        "close_runs_require_next_ordinal",
        "native_close_retry_receipts_require_plan",
        "native_close_source_fence_children_require_pending_plan",
    ];
    let mut untouched = Vec::new();
    for name in untouched_names {
        untouched.push(trigger(&db, name).await);
    }
    let source_before = trigger(&db, "native_close_source_fence_requires_exact_plan").await;
    let header_before = trigger(&db, "native_close_retry_plans_require_original").await;

    assert_eq!(run_through_for_test(&db.pool, 184).await.unwrap(), 1);
    for ((name, query), original) in native_queries.into_iter().zip(before) {
        assert_only_native_union_added(&original, &trigger(&db, name).await, query);
    }
    for (name, original) in untouched_names.into_iter().zip(untouched) {
        assert_eq!(trigger(&db, name).await, original, "{name}");
    }
    let source_query = source_before
        .split_once("    EXISTS (\n")
        .unwrap()
        .1
        .split_once("\n    )\n    OR EXISTS (")
        .unwrap()
        .0;
    assert_only_native_union_added(
        &source_before,
        &trigger(&db, "native_close_source_fence_requires_exact_plan").await,
        source_query,
    );
    let anchor_query = header_before
        .split_once("  OR NOT EXISTS (")
        .unwrap()
        .1
        .split_once(")\n  OR EXISTS (SELECT 1 FROM close_retirement_anchor_exclusions")
        .unwrap()
        .0;
    assert_only_native_union_added(
        &header_before,
        &trigger(&db, "native_close_retry_plans_require_original").await,
        anchor_query,
    );

    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN
        ('native_close_captured_loss_retry_plans', 'native_close_captured_loss_retry_inspections',
         'native_close_captured_loss_retry_resources')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(tables, 3);
    for view in [
        "native_close_retry_snapshot_authorities",
        "native_close_retry_resource_authorities",
        "native_close_retry_scope_authorities",
    ] {
        let sql: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'view' AND name = ?1")
                .bind(view)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(
            sql.contains("native_close_retry_plans")
                || sql.contains("native_close_retry_snapshot_authorities")
        );
        // Prepare and evaluate all projection definitions on the installed schema.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SELECT * FROM {view} LIMIT 1")))
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let schema: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema
        WHERE sql IS NOT NULL ORDER BY name",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(run_through_for_test(&db.pool, 184).await.unwrap(), 0);
    let again: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema
        WHERE sql IS NOT NULL ORDER BY name",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(schema, again);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn missing_or_changed_native_source_rolls_back184() {
    for missing in [true, false] {
        let db = predecessor().await;
        sqlx::raw_sql("DROP TRIGGER close_retirement_resource_dispatches_require_authority")
            .execute(&db.pool)
            .await
            .unwrap();
        if !missing {
            sqlx::raw_sql(
                "CREATE TRIGGER close_retirement_resource_dispatches_require_authority
                BEFORE INSERT ON close_retirement_resource_dispatches
                BEGIN SELECT RAISE(ABORT, 'changed source'); END;",
            )
            .execute(&db.pool)
            .await
            .unwrap();
        }
        let error = run_through_for_test(&db.pool, 184)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unexpected close_retirement_resource_dispatches_require_authority"),
            "{error}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM _migrations")
                .fetch_one(&db.pool)
                .await
                .unwrap(),
            183
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_schema
            WHERE name = 'native_close_captured_loss_retry_plans'"
            )
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA writable_schema")
                .fetch_one(&db.pool)
                .await
                .unwrap(),
            0
        );
    }
}

// An ordinary, non-native active retirement. In 183 its active-authority branch
// intentionally relies on the composite FK for identity membership. 184 must
// keep that branch literal AND replace every component of the FK's protection.
async fn original_active_inventory(db: &Database) -> (String, String, String, String) {
    let conversation = db
        .create_conversation(
            "original-active",
            "original-active",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let product = conversation.product_conversation_id;
    let transcript = TranscriptConversationId::parse("original-active").unwrap();
    let attempt = "original-active-attempt";
    db.begin_close_foundation(&product, &transcript, attempt)
        .await
        .unwrap();
    db.begin_close_idle_settlement(attempt).await.unwrap();
    db.advance_close_settlement_when_quiescent(attempt)
        .await
        .unwrap();
    db.replace_close_inspection(ReplaceCloseInspectionRequest {
        attempt_id: CloseAttemptId::parse(attempt).unwrap(),
        scopes: Vec::new(),
    })
    .await
    .unwrap();
    let obligation = db.get_close_obligation(attempt).await.unwrap();
    let scope: String =
        sqlx::query_scalar("SELECT scope FROM close_attempt_scopes WHERE attempt_id = ?1")
            .bind(attempt)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let scope = WorkScopeId::parse(scope).unwrap();
    let snapshot = obligation.snapshot().unwrap().clone();
    db.capture_close_retirement_inventory(CaptureCloseRetirementInventoryRequest {
        attempt_id: CloseAttemptId::parse(attempt).unwrap(),
        snapshot: snapshot.clone(),
        scopes: vec![CaptureCloseRetirementInventoryScopeRequest {
            scope: scope.clone(),
            inventory: CloseOwnedResourceInventory {
                worktree: None,
                work_scopes: Default::default(),
                bash_process_groups: Default::default(),
                tmux_servers: Default::default(),
                pty_sessions: Default::default(),
                browser_sessions: Default::default(),
                equivalent_live_resources: Default::default(),
            },
        }],
    })
    .await
    .unwrap();
    (
        attempt.to_owned(),
        scope.as_str().to_owned(),
        snapshot.generation().to_owned(),
        snapshot.fingerprint().to_owned(),
    )
}

async fn dispatch(
    db: &Database,
    attempt: &str,
    scope: &str,
    generation: &str,
    fingerprint: &str,
    codec: &str,
    identity: &str,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    sqlx::query(
        "INSERT INTO close_retirement_resource_dispatches
        (attempt_id, scope, inspection_generation, inspection_fingerprint,
         resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us)
        VALUES (?1, ?2, ?3, ?4, 'work_scope', 'opaque', ?5, ?6, 100)",
    )
    .bind(attempt)
    .bind(scope)
    .bind(generation)
    .bind(fingerprint)
    .bind(codec)
    .bind(identity)
    .execute(&db.pool)
    .await
}

#[tokio::test]
async fn original_active_inventory_still_rejects_arbitrary_dispatch_identity_and_codec() {
    let db = predecessor().await;
    let (attempt, scope, generation, fingerprint) = original_active_inventory(&db).await;
    let before = dispatch(
        &db,
        &attempt,
        &scope,
        &generation,
        &fingerprint,
        "opaque_string_v1",
        "not-in-inventory",
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        before.contains("FOREIGN KEY"),
        "183 must reach its composite FK: {before}"
    );
    run_through_for_test(&db.pool, 184).await.unwrap();
    for (candidate_generation, candidate_fingerprint, codec, identity) in [
        (
            generation.as_str(),
            fingerprint.as_str(),
            "opaque_string_v1",
            "not-in-inventory",
        ),
        (
            generation.as_str(),
            fingerprint.as_str(),
            "invented_codec",
            scope.as_str(),
        ),
        (
            "invented-generation",
            fingerprint.as_str(),
            "opaque_string_v1",
            scope.as_str(),
        ),
        (
            generation.as_str(),
            "invented-fingerprint",
            "opaque_string_v1",
            scope.as_str(),
        ),
    ] {
        let error = dispatch(
            &db,
            &attempt,
            &scope,
            candidate_generation,
            candidate_fingerprint,
            codec,
            identity,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("exact full resource and snapshot membership"),
            "{error}"
        );
    }
    dispatch(
        &db,
        &attempt,
        &scope,
        &generation,
        &fingerprint,
        "opaque_string_v1",
        &scope,
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resource_dispatches")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn original_dispatch_update_and_expected_resource_retention_survive184() {
    let db = predecessor().await;
    let (attempt, scope, generation, fingerprint) = original_active_inventory(&db).await;
    dispatch(
        &db,
        &attempt,
        &scope,
        &generation,
        &fingerprint,
        "opaque_string_v1",
        &scope,
    )
    .await
    .unwrap();
    let immutable_before = trigger(&db, "close_expected_retirement_resources_are_immutable").await;
    let retained_before = trigger(
        &db,
        "close_expected_retirement_resources_reject_standalone_delete",
    )
    .await;
    for migrated in [false, true] {
        if migrated {
            run_through_for_test(&db.pool, 184).await.unwrap();
        }
        for column in [
            "attempt_id",
            "scope",
            "inspection_generation",
            "inspection_fingerprint",
            "resource_kind",
            "identity_kind",
            "identity_value",
        ] {
            let error = sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "UPDATE close_retirement_resource_dispatches SET {column} = 'not-a-member'"
            )))
            .execute(&db.pool)
            .await
            .unwrap_err()
            .to_string();
            assert!(
                error.contains(if migrated {
                    "exact full resource and snapshot membership"
                } else {
                    "FOREIGN KEY"
                }) || error.contains("CHECK constraint"),
                "{column}: {error}"
            );
        }
        // Timestamp-only updates do not change membership and remain permitted.
        sqlx::query("UPDATE close_retirement_resource_dispatches SET dispatched_at_us = 101")
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(sqlx::query(
            "UPDATE close_expected_retirement_resources SET identity_value = 'changed'"
        )
        .execute(&db.pool)
        .await
        .is_err());
        assert!(
            sqlx::query("DELETE FROM close_expected_retirement_resources")
                .execute(&db.pool)
                .await
                .is_err()
        );
    }
    let error = sqlx::query(
        "UPDATE close_retirement_resource_dispatches SET identity_codec = 'invented_codec'",
    )
    .execute(&db.pool)
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("exact full resource and snapshot membership"),
        "{error}"
    );
    assert_eq!(
        trigger(&db, "close_expected_retirement_resources_are_immutable").await,
        immutable_before
    );
    assert_eq!(
        trigger(
            &db,
            "close_expected_retirement_resources_reject_standalone_delete"
        )
        .await,
        retained_before
    );
    // Reference retention, unlike standalone retention, must not disappear when
    // the product has been removed by a cascading aggregate cleanup.
    for suffix in ["update", "delete"] {
        let definition = trigger(
            &db,
            &format!("close_expected_retirement_resources_preserve_dispatched_{suffix}"),
        )
        .await;
        assert!(definition.contains("close_retirement_resource_dispatches"));
        assert!(!definition.contains("product_conversations"));
        assert!(!definition.contains("close_obligations"));
    }
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn delete_api_clears_dispatches_before_expected_resource_teardown() {
    let db = predecessor().await;
    let (attempt, scope, generation, fingerprint) = original_active_inventory(&db).await;
    run_through_for_test(&db.pool, 184).await.unwrap();
    dispatch(
        &db,
        &attempt,
        &scope,
        &generation,
        &fingerprint,
        "opaque_string_v1",
        &scope,
    )
    .await
    .unwrap();

    for expected in db
        .list_close_expected_retirement_resources(&attempt)
        .await
        .unwrap()
    {
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: CloseAttemptId::parse(&attempt).unwrap(),
            snapshot: CloseRetirementSnapshot::parse(&generation, &fingerprint).unwrap(),
            scope: expected.scope,
            resource: expected.resource,
            outcome: RetirementOutcome::Retired,
            detail: None,
        })
        .await
        .unwrap();
    }
    let completed = db
        .complete_close_retirement(&CloseAttemptId::parse(&attempt).unwrap())
        .await
        .unwrap();
    assert_eq!(completed.phase(), ClosePhase::Completed);
    assert_eq!(
        completed.close_outcome(),
        Some(CloseCompletionOutcome::Archived)
    );
    let dispatches_before_delete: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1",
    )
    .bind(&attempt)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(dispatches_before_delete, 1);

    db.delete_conversation("original-active").await.unwrap();

    for table in [
        "close_obligations",
        "close_retirement_resource_dispatches",
        "close_expected_retirement_resources",
        "close_attempt_scopes",
    ] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1"
        )))
        .bind(&attempt)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .is_empty());
}

// Focused schema regression, not admission coverage: the actual installed capture
// DDL and mutation guards run against a minimal obligation-owned parent graph.
// The private native admission fixture is outside this migration test module.
#[tokio::test]
async fn captured_authority_schema_is_immutable_but_cascades_with_its_obligation() {
    let db = predecessor().await;
    run_through_for_test(&db.pool, 184).await.unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::raw_sql(
        "PRAGMA foreign_keys = ON;
        CREATE TABLE close_obligations (attempt_id TEXT PRIMARY KEY);
        CREATE TABLE native_close_retry_plans (
            attempt_id TEXT NOT NULL REFERENCES close_obligations(attempt_id) ON DELETE CASCADE,
            run_ordinal INTEGER NOT NULL, PRIMARY KEY (attempt_id, run_ordinal));
        CREATE TABLE close_attempt_scopes (
            attempt_id TEXT NOT NULL REFERENCES close_obligations(attempt_id) ON DELETE CASCADE,
            scope TEXT NOT NULL, PRIMARY KEY (attempt_id, scope));",
    )
    .execute(&pool)
    .await
    .unwrap();
    let tables = [
        "native_close_captured_loss_retry_plans",
        "native_close_captured_loss_retry_inspections",
        "native_close_captured_loss_retry_resources",
    ];
    for table in tables {
        let ddl: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1")
                .bind(table)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&pool)
            .await
            .unwrap();
        let delete_actions: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT on_delete FROM pragma_foreign_key_list('{table}')"
        )))
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            delete_actions.len(),
            if table.ends_with("_plans") { 2 } else { 4 }
        );
        assert!(
            delete_actions.iter().all(|action| action == "CASCADE"),
            "{table}"
        );
        for suffix in ["immutable", "retained"] {
            let guard = trigger(&db, &format!("{table}_{suffix}")).await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(guard))
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    sqlx::raw_sql(
        "INSERT INTO close_obligations VALUES ('captured-attempt');
        INSERT INTO native_close_retry_plans VALUES ('captured-attempt', 2);
        INSERT INTO close_attempt_scopes VALUES ('captured-attempt', 'captured-scope');
        INSERT INTO native_close_captured_loss_retry_plans VALUES
            ('captured-attempt', 2, 'origin-generation', 'origin-fingerprint',
             'retry-generation', 'retry-fingerprint', 100);
        INSERT INTO native_close_captured_loss_retry_inspections VALUES
            ('captured-attempt', 2, 'captured-scope', 'fresh-generation', 'fresh-fingerprint');
        INSERT INTO native_close_captured_loss_retry_resources VALUES
            ('captured-attempt', 2, 0, 'captured-scope', 'work_scope', 'opaque',
             'opaque_string_v1', 'captured-scope');",
    )
    .execute(&pool)
    .await
    .unwrap();
    for table in tables {
        let error = sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DELETE FROM {table}")))
            .execute(&pool)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("captured-loss retry authority must be retained"),
            "{error}"
        );
        let error = sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET attempt_id = attempt_id"
        )))
        .execute(&pool)
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("captured-loss retry authority is immutable"),
            "{error}"
        );
    }
    for statement in [
        "DELETE FROM native_close_retry_plans",
        "DELETE FROM close_attempt_scopes",
    ] {
        assert!(sqlx::raw_sql(statement).execute(&pool).await.is_err());
    }
    sqlx::query("DELETE FROM close_obligations WHERE attempt_id = 'captured-attempt'")
        .execute(&pool)
        .await
        .unwrap();
    for table in tables {
        let count: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap()
        .is_empty());
}
