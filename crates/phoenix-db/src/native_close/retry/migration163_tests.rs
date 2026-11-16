async fn migration_163_pending_fixture() -> (Database, Assessment, NativeCloseReceipt) {
    migration_163_pending_fixture_with_options(true, false).await
}

async fn migration_163_pending_fixture_with_options(
    fresh_snapshot: bool,
    prior_dispatch: bool,
) -> (Database, Assessment, NativeCloseReceipt) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::migrations::setup_real_162_for_test(&pool).await;
    let db = Database::from_pool_for_tests(pool, String::new());
    let mut failure = initial_cleanup_failure_fixture_with_resources(
        &db,
        CloseStopCertainty::ShutdownUncertain,
        Vec::new(),
    )
    .await;
    let g1 = current_test_snapshot(&db, failure.attempt_id.as_str()).await;
    let effects = native_retry_request(&failure).remaining_effects;
    if prior_dispatch {
        let worktree = effects.iter()
            .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
        db.record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
            attempt_id: failure.attempt_id.clone(), scope: worktree.scope.clone(),
            snapshot: g1.clone(), resource: worktree.resource.clone(),
        }).await.unwrap();
    }
    for pending in &failure.remaining_resources {
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: failure.attempt_id.clone(),
            scope: pending.scope.clone(),
            snapshot: g1.clone(),
            resource: pending.resource.clone(),
            outcome: RetirementOutcome::Residual {
                residual_reason: RetirementFailureReason::RemovalFailed,
            },
            detail: Some("original G1 residual".into()),
        })
        .await
        .unwrap();
    }
    if fresh_snapshot {
        let scope = failure.authority.scope().unwrap().clone();
        db.retry_close_retirement(&failure.attempt_id).await.unwrap();
        db.replace_close_inspection(ReplaceCloseInspectionRequest {
            attempt_id: failure.attempt_id.clone(),
            scopes: vec![ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("migration163-g2", "migration163-g2-fp")
                    .unwrap(),
                losses: Vec::new(),
            }],
        })
        .await
        .unwrap();
        let g2 = current_test_snapshot(&db, failure.attempt_id.as_str()).await;
        capture_test_inventory(&db, failure.attempt_id.as_str(), &scope, &g2, Vec::new()).await;
        for pending in &failure.remaining_resources {
            db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: failure.attempt_id.clone(),
                scope: pending.scope.clone(),
                snapshot: g2.clone(),
                resource: pending.resource.clone(),
                outcome: if pending.resource.kind() == RetiredResourceKind::Worktree {
                    RetirementOutcome::Retired
                } else {
                    RetirementOutcome::Residual {
                        residual_reason: RetirementFailureReason::RemovalFailed,
                    }
                },
                detail: Some("current G2 evidence".into()),
            })
            .await
            .unwrap();
        }
        let resource = failure.remaining_resources.iter()
            .find(|pending| pending.scope == scope && pending.resource.kind() == RetiredResourceKind::WorkScope)
            .unwrap().resource.clone();
        failure.remaining_resources = db.expected_close_cleanup_failure_resources(
            failure.attempt_id.as_str(), &scope, &resource,
        ).await.unwrap();
        failure.authority = CloseCleanupFailureAuthority::ExpectedResource {
            scope, snapshot: g2, resource,
        };
        failure.detail = "current G2 evidence".into();
    } else {
        let CloseCleanupFailureAuthority::ExpectedResource { scope, resource, .. } = &failure.authority else {
            panic!("expected original inventory authority");
        };
        failure.remaining_resources = db.expected_close_cleanup_failure_resources(
            failure.attempt_id.as_str(), scope, resource,
        ).await.unwrap();
        failure.detail = "original G1 residual".into();
    }
    failure.occurred_at_us = Utc::now().timestamp_micros();
    db.terminalize_initial_close_cleanup_failure(&failure).await.unwrap();
    let mut assessment = assess(&db, &failure, "migration163").await;
    assessment.request.remaining_effects = effects;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    (db, assessment, receipt)
}

async fn migration_163_retained_state(db: &Database) -> String {
    sqlx::query_scalar(
        "SELECT json_object(
          'obligation', (SELECT json_group_array(json_array(attempt_id, product_conversation_id,
              phase, close_outcome, inspection_generation, inspection_fingerprint, native_managed)) FROM close_obligations),
          'anchors', (SELECT json_group_array(json_array(attempt_id, inspection_generation, inspection_fingerprint))
              FROM close_original_retirement_anchors),
          'inventories', (SELECT json_group_array(json_array(attempt_id, scope, inspection_generation,
              inspection_fingerprint, sealed)) FROM close_retirement_inventories),
          'expected', (SELECT json_group_array(json_array(attempt_id, scope, inspection_generation,
              inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value))
              FROM close_expected_retirement_resources),
          'evidence', (SELECT json_group_array(json_array(attempt_id, scope, inspection_generation,
              inspection_fingerprint, resource_kind, identity_kind, identity_codec, identity_value,
              proof_kind, residual_reason, detail)) FROM close_retirement_resources),
          'header', (SELECT json_group_array(json_array(attempt_id, run_ordinal, request_id, request_fingerprint,
              failed_run_ordinal, evidence_kind, inspection_generation, inspection_fingerprint, plan_fingerprint, effect_count))
              FROM native_close_retry_plans),
          'receipts', (SELECT json_group_array(json_array(request_id, request_fingerprint, product_conversation_id,
              intent, attempt_id, run_ordinal, source_transcript_id, source_message_id, source_tool_use_id)) FROM native_close_initial_receipts),
          'effects', (SELECT json_group_array(json_array(attempt_id, run_ordinal, ordinal, scope,
              resource_kind, identity_kind, identity_codec, identity_value)) FROM close_run_retry_effects),
          'runs', (SELECT json_group_array(json_array(attempt_id, run_ordinal, status, created_at_us, ended_at_us)) FROM close_runs),
          'failures', (SELECT json_group_array(json_array(failure_occurrence_id, authority_kind,
              inspection_generation, inspection_fingerprint, occurred_at_us)) FROM close_cleanup_failures))",
    )
    .fetch_one(db.pool())
    .await
    .unwrap()
}

async fn migration_163_insert_parent(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    run: &CloseRunRef,
    effect: i64,
    snapshot: &CloseRetirementSnapshot,
    occurred: i64,
    confirmed: Option<i64>,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    sqlx::query(
        "INSERT INTO close_cleanup_failures
         (failure_occurrence_id, attempt_id, source_product_conversation_id, cleanup_run_ordinal,
          authority_kind, scope, inspection_generation, inspection_fingerprint, resource_kind,
          identity_kind, identity_codec, identity_value, stop_certainty, confirmed_at_us, reason, detail, occurred_at_us)
         SELECT ?1, plan.attempt_id, obligation.product_conversation_id, plan.run_ordinal,
          'expected_resource', effect.scope, ?4, ?5, effect.resource_kind,
          effect.identity_kind, effect.identity_codec, effect.identity_value, ?6, ?7,
          'removal_failed', 'native G1 retry failed', ?8
         FROM native_close_retry_plans plan
         JOIN close_obligations obligation ON obligation.attempt_id = plan.attempt_id
         JOIN close_run_retry_effects effect ON effect.attempt_id = plan.attempt_id AND effect.run_ordinal = plan.run_ordinal
         WHERE plan.attempt_id = ?2 AND plan.run_ordinal = ?3 AND effect.ordinal = ?9",
    )
    .bind(run.failure_occurrence_id().as_str())
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .bind(snapshot.generation())
    .bind(snapshot.fingerprint())
    .bind(if confirmed.is_some() { "conversation_and_processes_stopped" } else { "shutdown_uncertain" })
    .bind(confirmed)
    .bind(occurred)
    .bind(effect)
    .execute(&mut **tx)
    .await
}

async fn migration_163_insert_child(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    run: &CloseRunRef,
    effect: i64,
    ordinal: i64,
    disposition: &str,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    sqlx::query(
        "INSERT INTO close_cleanup_failure_resources
         (failure_occurrence_id, ordinal, scope, resource_kind, identity_kind, identity_codec, identity_value, disposition)
         SELECT ?1, ?2, scope, resource_kind, identity_kind, identity_codec, identity_value, ?3
         FROM close_run_retry_effects WHERE attempt_id = ?4 AND run_ordinal = ?5 AND ordinal = ?6",
    )
    .bind(run.failure_occurrence_id().as_str())
    .bind(ordinal)
    .bind(disposition)
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .bind(effect)
    .execute(&mut **tx)
    .await
}

#[tokio::test]
async fn migration_163_upgrades_populated_162_without_rebinding_g1_or_g2() {
    let (db, _, receipt) = migration_163_pending_fixture().await;
    let before = migration_163_retained_state(&db).await;
    let definitions: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name",
    ).fetch_all(db.pool()).await.unwrap();
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 163).await.unwrap(), 1);
    assert_eq!(migration_163_retained_state(&db).await, before);
    for (name, installed) in definitions {
        let rewritten: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name = ?1")
            .bind(&name).fetch_one(db.pool()).await.unwrap();
        if matches!(name.as_str(), "close_cleanup_failures_require_initial_authority" | "close_cleanup_failure_resources_require_authority") {
            let (_, body) = installed.split_once("WHEN ").unwrap();
            assert!(rewritten.ends_with(body), "installed body/exceptions changed: {name}");
        } else {
            assert_eq!(rewritten, installed, "unrelated trigger changed: {name}");
        }
    }
    let latest: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migrations")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(latest, 163);
    let run = CloseRunRef { attempt_id: receipt.admitted.attempt_id, ordinal: receipt.admitted.run_ordinal };
    assert_eq!(db.close_safe_retry_progress(&run).await.unwrap().pending.len(), 3);
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 163).await.unwrap(), 0);
}

#[tokio::test]
async fn migration_163_g1_failure_guards_preserve_g2_and_exact_pending_children() {
    let (db, _, receipt) = migration_163_pending_fixture().await;
    crate::migrations::run_through_for_test(db.pool(), 163).await.unwrap();
    let before = migration_163_retained_state(&db).await;
    let run = CloseRunRef { attempt_id: receipt.admitted.attempt_id, ordinal: receipt.admitted.run_ordinal };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    assert_eq!(pending.len(), 3);
    assert_eq!(pending[0].resource.kind(), RetiredResourceKind::Worktree);
    assert!(pending[1..].iter().all(|effect| effect.resource.kind() == RetiredResourceKind::WorkScope));
    assert_ne!(pending[1].scope, pending[2].scope);
    let original: (String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint FROM native_close_retry_plans",
    ).fetch_one(db.pool()).await.unwrap();
    let g1 = CloseRetirementSnapshot::parse(original.0, original.1).unwrap();
    let g2 = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    assert_ne!(g1, g2);
    let created: i64 = sqlx::query_scalar("SELECT created_at_us FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2")
        .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(db.pool()).await.unwrap();
    for confirmed in [false, true] {
        let mut tx = db.pool().begin().await.unwrap();
        for (snapshot, occurred, confirmation) in [
            (&g2, created + 10, None),
            (&g1, created - 1, None),
            (&g1, created + 10, Some(created + 11)),
        ] {
            assert!(migration_163_insert_parent(&mut tx, &run, 0, snapshot, occurred, confirmation).await.is_err());
        }
        let occurred = created + 10;
        let result = migration_163_insert_parent(&mut tx, &run, 0, &g1, occurred, confirmed.then_some(occurred)).await.unwrap();
        assert_eq!(result.rows_affected(), 1);
        assert!(migration_163_insert_child(&mut tx, &run, 1, 0, "unattempted").await.is_err(), "cannot skip the pending head");
        assert!(migration_163_insert_child(&mut tx, &run, 0, 0, "unattempted").await.is_err(), "designated effect must be failed");
        assert_eq!(migration_163_insert_child(&mut tx, &run, 0, 0, "failed").await.unwrap().rows_affected(), 1);
        assert!(migration_163_insert_child(&mut tx, &run, 1, 1, "residual").await.is_err(), "G1 residual evidence is not a new attempt");
        assert_eq!(migration_163_insert_child(&mut tx, &run, 1, 1, "unattempted").await.unwrap().rows_affected(), 1);
        assert!(migration_163_insert_child(&mut tx, &run, 2, 2, "failed").await.is_err(), "independent scope was not the failed effect");
        assert!(migration_163_insert_child(&mut tx, &run, 2, 2, "residual").await.is_err(), "independent scope is still unattempted");
        assert_eq!(migration_163_insert_child(&mut tx, &run, 2, 2, "unattempted").await.unwrap().rows_affected(), 1);
        let children: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT ordinal, scope, disposition FROM close_cleanup_failure_resources
             WHERE failure_occurrence_id = ?1 ORDER BY ordinal",
        ).bind(run.failure_occurrence_id().as_str()).fetch_all(&mut *tx).await.unwrap();
        assert_eq!(children, vec![
            (0, pending[0].scope.as_str().to_owned(), "failed".to_owned()),
            (1, pending[1].scope.as_str().to_owned(), "unattempted".to_owned()),
            (2, pending[2].scope.as_str().to_owned(), "unattempted".to_owned()),
        ]);
        tx.rollback().await.unwrap();
    }
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO close_run_retry_successes (attempt_id, run_ordinal, ordinal, detail, observed_at_us) VALUES (?1, ?2, 0, 'retired', ?3)")
        .bind(run.attempt_id.as_str()).bind(run.ordinal.get()).bind(created + 1).execute(&mut *tx).await.unwrap();
    assert!(migration_163_insert_parent(&mut tx, &run, 0, &g1, created + 10, None).await.is_err(), "successful effect cannot fail");
    migration_163_insert_parent(&mut tx, &run, 1, &g1, created + 10, None).await.unwrap();
    assert!(migration_163_insert_child(&mut tx, &run, 0, 0, "unattempted").await.is_err(), "successful child is not pending");
    assert_eq!(migration_163_insert_child(&mut tx, &run, 1, 0, "failed").await.unwrap().rows_affected(), 1);
    assert!(migration_163_insert_child(&mut tx, &run, 2, 1, "failed").await.is_err(), "only the designated scope failed");
    assert!(migration_163_insert_child(&mut tx, &run, 2, 1, "residual").await.is_err(), "other scope remains unattempted");
    assert_eq!(migration_163_insert_child(&mut tx, &run, 2, 1, "unattempted").await.unwrap().rows_affected(), 1);
    let children: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT ordinal, scope, disposition FROM close_cleanup_failure_resources
         WHERE failure_occurrence_id = ?1 ORDER BY ordinal",
    ).bind(run.failure_occurrence_id().as_str()).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(children, vec![
        (0, pending[1].scope.as_str().to_owned(), "failed".to_owned()),
        (1, pending[2].scope.as_str().to_owned(), "unattempted".to_owned()),
    ]);
    tx.rollback().await.unwrap();
    assert_eq!(migration_163_retained_state(&db).await, before);
}

#[tokio::test]
async fn migration_163_upgrade_preserves_installed_source_fence_behavior() {
    let (db, assessment, receipt) = migration_163_pending_fixture().await;
    crate::migrations::run_through_for_test(db.pool(), 163).await.unwrap();
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id,
        ordinal: receipt.admitted.run_ordinal,
    };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    assert_source_fenced(&db, &assessment, &run, &pending).await;
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation);
}

#[tokio::test]
async fn migration_163_unknown_installed_prefix_rolls_back_both_trigger_rewrites() {
    let (db, _, _) = migration_163_pending_fixture().await;
    let original: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name = 'close_cleanup_failure_resources_require_authority'")
        .fetch_one(db.pool()).await.unwrap();
    let unknown = original.replacen("WHEN NOT EXISTS", "WHEN  NOT EXISTS", 1);
    sqlx::raw_sql("DROP TRIGGER close_cleanup_failure_resources_require_authority")
        .execute(db.pool()).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(unknown)).execute(db.pool()).await.unwrap();
    let before: Vec<(String, String)> = sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name")
        .fetch_all(db.pool()).await.unwrap();
    let state = migration_163_retained_state(&db).await;
    assert!(crate::migrations::run_through_for_test(db.pool(), 163).await.is_err());
    let after: Vec<(String, String)> = sqlx::query_as("SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name")
        .fetch_all(db.pool()).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(migration_163_retained_state(&db).await, state);
    let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migrations")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(version, 162);
}
