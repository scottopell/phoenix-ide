use crate::close_foundation::{
    BindCloseWorktreeFinalTombstoneObjectRequest, BindCloseWorktreeFinalTombstoneRequest,
    CloseWorktreeFinalTombstone, RecordCloseWorktreeCleanupPlanRequest,
};

type DispatchRow = (String, String, String, String, String, String, String, String, i64);
async fn retry_dispatch_state(db: &Database, attempt: &CloseAttemptId) -> String {
    let rows: String = sqlx::query_scalar(
        "SELECT json_object(
          'dispatches', (SELECT json_group_array(json_array(attempt_id, scope,
              inspection_generation, inspection_fingerprint, resource_kind, identity_kind,
              identity_codec, identity_value, dispatched_at_us)) FROM close_retirement_resource_dispatches),
          'successes', (SELECT json_group_array(json_array(attempt_id, run_ordinal, ordinal,
              detail, observed_at_us)) FROM close_run_retry_successes),
          'failure_children', (SELECT json_group_array(json_array(failure_occurrence_id, ordinal,
              scope, resource_kind, identity_kind, identity_codec, identity_value, disposition))
              FROM close_cleanup_failure_resources))",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    format!(
        "{:?}",
        (
            migration_176_retained_state(db).await,
            lookup_metadata(db, attempt).await,
            native_failure_side_effect_counts(db).await,
            rows,
        )
    )
}

async fn insert_retry_dispatch_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &RecordCloseRetirementDispatchRequest,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    insert_retry_dispatch_at_tx(tx, request, Utc::now().timestamp_micros()).await
}

async fn insert_retry_dispatch_at_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &RecordCloseRetirementDispatchRequest,
    dispatched_at_us: i64,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    let identity = request.resource.identity();
    sqlx::query(
        "INSERT INTO close_retirement_resource_dispatches
         (attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind,
          identity_kind, identity_codec, identity_value, dispatched_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(request.attempt_id.as_str())
    .bind(request.scope.as_str())
    .bind(request.snapshot.generation())
    .bind(request.snapshot.fingerprint())
    .bind(request.resource.kind().as_str())
    .bind(identity.identity_kind())
    .bind(identity.codec())
    .bind(identity.value())
    .bind(dispatched_at_us)
    .execute(&mut **tx)
    .await
}

#[tokio::test]
async fn native_retry_dispatch_uses_original_g1_under_g2_and_replays_without_mutation() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let attempt = &receipt.admitted.attempt_id;
    let g2 = current_test_snapshot(&db, attempt.as_str()).await;
    assert_ne!(g2, assessment.proof.snapshot);
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let retained = migration_176_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let metadata = lookup_metadata(&db, attempt).await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_retirement_resource_dispatches")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(before, 0);

    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    let rows: Vec<DispatchRow> =
        sqlx::query_as(
            "SELECT attempt_id, scope, inspection_generation, inspection_fingerprint,
              resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us
             FROM close_retirement_resource_dispatches",
        ).fetch_all(db.pool()).await.unwrap();
    let identity = worktree.resource.identity();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], (
        attempt.as_str().to_owned(), worktree.scope.as_str().to_owned(),
        assessment.proof.snapshot.generation().to_owned(),
        assessment.proof.snapshot.fingerprint().to_owned(),
        "worktree".to_owned(), identity.identity_kind().to_owned(),
        identity.codec().to_owned(), identity.value().clone(), rows[0].8,
    ));
    assert!(rows[0].8 > 0);
    assert_eq!(migration_176_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
    assert_eq!(lookup_metadata(&db, attempt).await, metadata);
    assert_eq!(current_test_snapshot(&db, attempt.as_str()).await, g2);

    let dispatched = retry_dispatch_state(&db, attempt).await;
    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, dispatched);
}

#[tokio::test]
async fn native_retry_dispatch_refuses_forged_identity_non_worktree_and_success() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let attempt = &receipt.admitted.attempt_id;
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let LossItemIdentity::Worktree(captured) = worktree.resource.identity() else {
        panic!("full captured worktree identity");
    };
    let before = retry_dispatch_state(&db, attempt).await;
    for (fingerprint, locator) in [
        (WorktreeFingerprint::parse("forged-dispatch-fingerprint").unwrap(), captured.locator().clone()),
        (captured.fingerprint().clone(), GitPathIdentity::from_bytes(b"/forged/dispatch/locator".to_vec())),
    ] {
        let mut forged = worktree.clone();
        forged.resource = RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                captured.id().clone(), fingerprint, locator,
            )),
        ).unwrap();
        success_refused(db.record_native_close_retry_retirement_dispatch(
            &assessment.command, &receipt.admitted, &forged,
        ).await);
        assert_eq!(retry_dispatch_state(&db, attempt).await, before);
    }
    let work_scope = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::WorkScope).unwrap();
    success_refused(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, work_scope,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);

    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "already retired",
    ).await);
    let successful = retry_dispatch_state(&db, attempt).await;
    success_refused(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, successful);
}

#[tokio::test]
async fn native_retry_dispatch_checks_stale_source_before_retained_dispatch_replay() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let before = retry_dispatch_state(&db, &receipt.admitted.attempt_id).await;
    let result = db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(message))) = result else {
        panic!("stale source must refuse before dispatch replay: {result:?}");
    };
    assert!(message.contains("current executing source"), "{message}");
    assert_eq!(retry_dispatch_state(&db, &receipt.admitted.attempt_id).await, before);
}

#[tokio::test]
async fn generic_retry_dispatch_refuses_native_current_g1_with_stale_source() {
    let (db, assessment, receipt) = migration_176_pending_fixture_with_options(false, false).await;
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    let attempt = &receipt.admitted.attempt_id;
    assert_eq!(current_test_snapshot(&db, attempt.as_str()).await, assessment.proof.snapshot);
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let before = retry_dispatch_state(&db, attempt).await;
    let result = db.record_close_retirement_dispatch(RecordCloseRetirementDispatchRequest {
        attempt_id: attempt.clone(), scope: worktree.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(), resource: worktree.resource.clone(),
    }).await;
    let Err(DbError::CloseFoundationPrecondition(message)) = result else {
        panic!("generic native retry dispatch must refuse: {result:?}");
    };
    assert!(message.contains("generic retry dispatch is unavailable for native-managed"), "{message}");
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
    success_refused(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
}

#[tokio::test]
async fn native_retry_dispatch_replays_historical_g1_without_retimestamping() {
    let (db, assessment, receipt) = migration_176_pending_fixture_with_options(true, true).await;
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    let attempt = &receipt.admitted.attempt_id;
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let (dispatched_at_us, created_at_us): (i64, i64) = sqlx::query_as(
        "SELECT dispatch.dispatched_at_us, run.created_at_us
         FROM close_retirement_resource_dispatches dispatch
         JOIN close_runs run ON run.attempt_id = dispatch.attempt_id
         WHERE run.attempt_id = ?1 AND run.run_ordinal = ?2",
    ).bind(attempt.as_str()).bind(receipt.admitted.run_ordinal.get())
        .fetch_one(db.pool()).await.unwrap();
    assert!(dispatched_at_us < created_at_us);
    let before = retry_dispatch_state(&db, attempt).await;
    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, worktree,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
}

#[tokio::test]
async fn native_retry_raw_dispatch_rejects_backdated_new_row() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let attempt = &receipt.admitted.attempt_id;
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let request = RecordCloseRetirementDispatchRequest {
        attempt_id: attempt.clone(), scope: worktree.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(), resource: worktree.resource.clone(),
    };
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_retirement_resource_dispatches")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(count, 0);
    let created_at_us: i64 = sqlx::query_scalar(
        "SELECT created_at_us FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2",
    ).bind(attempt.as_str()).bind(receipt.admitted.run_ordinal.get())
        .fetch_one(db.pool()).await.unwrap();
    assert!(created_at_us > 0);
    let before = retry_dispatch_state(&db, attempt).await;
    let mut tx = db.pool().begin().await.unwrap();
    assert!(insert_retry_dispatch_at_tx(&mut tx, &request, created_at_us - 1).await.is_err());
    assert_eq!(insert_retry_dispatch_at_tx(&mut tx, &request, created_at_us).await.unwrap().rows_affected(), 1);
    tx.rollback().await.unwrap();
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
}

#[tokio::test]
async fn native_retry_raw_dispatch_rejects_g2_and_malformed_plan_without_legacy_fallthrough() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let attempt = &receipt.admitted.attempt_id;
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let g1 = RecordCloseRetirementDispatchRequest {
        attempt_id: attempt.clone(), scope: worktree.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(), resource: worktree.resource.clone(),
    };
    let g2 = RecordCloseRetirementDispatchRequest {
        snapshot: current_test_snapshot(&db, attempt.as_str()).await,
        ..g1.clone()
    };
    assert_ne!(g1.snapshot, g2.snapshot);
    let before = retry_dispatch_state(&db, attempt).await;
    let mut tx = db.pool().begin().await.unwrap();
    assert!(insert_retry_dispatch_tx(&mut tx, &g2).await.is_err());
    assert_eq!(insert_retry_dispatch_tx(&mut tx, &g1).await.unwrap().rows_affected(), 1);
    tx.rollback().await.unwrap();
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);

    let mut tx = db.pool().begin().await.unwrap();
    sqlx::raw_sql("DROP TRIGGER native_close_retry_plans_immutable")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE native_close_retry_plans SET effect_count = effect_count + 1 WHERE attempt_id = ?1 AND run_ordinal = ?2")
        .bind(attempt.as_str()).bind(receipt.admitted.run_ordinal.get())
        .execute(&mut *tx).await.unwrap();
    assert!(insert_retry_dispatch_tx(&mut tx, &g1).await.is_err());
    assert!(insert_retry_dispatch_tx(&mut tx, &g2).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
}

#[tokio::test]
async fn migration_177_preserves_populated_163_rows_and_rewrites_only_dispatch_authority() {
    let (db, assessment, receipt) = migration_176_pending_fixture().await;
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 176).await.unwrap(), 1);
    let attempt = &receipt.admitted.attempt_id;
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let g2 = RecordCloseRetirementDispatchRequest {
        attempt_id: attempt.clone(), scope: worktree.scope.clone(),
        snapshot: current_test_snapshot(&db, attempt.as_str()).await,
        resource: worktree.resource.clone(),
    };
    assert_ne!(g2.snapshot, assessment.proof.snapshot);
    let mut tx = db.pool().begin().await.unwrap();
    assert_eq!(insert_retry_dispatch_tx(&mut tx, &g2).await.unwrap().rows_affected(), 1);
    tx.commit().await.unwrap();
    let before = retry_dispatch_state(&db, attempt).await;
    let definitions: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name",
    ).fetch_all(db.pool()).await.unwrap();
    let ledger: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT version, name, applied_at FROM _migrations ORDER BY version",
    ).fetch_all(db.pool()).await.unwrap();
    assert_eq!(ledger.last().unwrap().0, 176);
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 177).await.unwrap(), 1);
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
    for (name, installed) in definitions {
        let rewritten: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name = ?1")
            .bind(&name).fetch_one(db.pool()).await.unwrap();
        if name == "close_retirement_resource_dispatches_require_authority" {
            assert_eq!(installed.matches("obligation.phase = 'completed'").count(), 1);
            let expected = installed
                .replacen("obligation.phase = 'completed'", "obligation.native_managed = 0 AND obligation.phase = 'completed'", 1)
                .replacen("\nBEGIN", &format!("\nAND NOT EXISTS (\n{}\n)\nBEGIN",
                    include_str!("../../migrations/native_close_retry_dispatch.sql")), 1);
            assert_eq!(rewritten, expected);
        } else {
            assert_eq!(rewritten, installed, "unrelated trigger changed: {name}");
        }
    }
    let upgraded: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT version, name, applied_at FROM _migrations ORDER BY version",
    ).fetch_all(db.pool()).await.unwrap();
    assert_eq!(&upgraded[..ledger.len()], ledger.as_slice());
    assert_eq!(upgraded.len(), ledger.len() + 1);
    assert_eq!(upgraded.last().unwrap().0, 177);
    assert_eq!(upgraded.last().unwrap().1, "native_close_retry_dispatch");
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 177).await.unwrap(), 0);
    assert_eq!(retry_dispatch_state(&db, attempt).await, before);
    // Probe the historical 177 authority directly. Current application writers
    // require later schema and are covered by the current-schema tests above.
    let original = RecordCloseRetirementDispatchRequest {
        attempt_id: attempt.clone(), scope: worktree.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(), resource: worktree.resource.clone(),
    };
    let mut tx = db.pool().begin().await.unwrap();
    assert_eq!(insert_retry_dispatch_tx(&mut tx, &original).await.unwrap().rows_affected(), 1);
    tx.commit().await.unwrap();
    let dispatches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_retirement_resource_dispatches")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(dispatches, 2);
}

#[tokio::test]
async fn non_native_retry_dispatch_retains_legacy_current_snapshot_authority() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let run = db.admit_close_safe_retry(&safe_retry_request(&failure)).await.unwrap();
    let worktree = db.close_safe_retry_progress(&run).await.unwrap().pending.into_iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let request = RecordCloseRetirementDispatchRequest {
        attempt_id: run.attempt_id.clone(), scope: worktree.scope,
        snapshot: current_test_snapshot(&db, run.attempt_id.as_str()).await,
        resource: worktree.resource,
    };
    let native: i64 = sqlx::query_scalar("SELECT native_managed FROM close_obligations WHERE attempt_id = ?1")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(native, 0);
    db.record_close_retirement_dispatch(request.clone()).await.unwrap();
    let before = retry_dispatch_state(&db, &run.attempt_id).await;
    db.record_close_retirement_dispatch(request).await.unwrap();
    assert_eq!(retry_dispatch_state(&db, &run.attempt_id).await, before);
}


#[tokio::test]
async fn migration_178_unknown_second_authority_prefix_rolls_back_first_rewrite_and_ledger() {
    let (db, _, _) = migration_176_pending_fixture().await;
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 177).await.unwrap(), 2);
    let second: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE name = 'close_worktree_tombstone_require_retry_authority'",
    )
    .fetch_one(db.pool()).await.unwrap();
    let unknown = second.replacen("\nBEGIN", "\nAND 1 = 1\nBEGIN", 1);
    sqlx::raw_sql("DROP TRIGGER close_worktree_tombstone_require_retry_authority")
        .execute(db.pool()).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(unknown)).execute(db.pool()).await.unwrap();
    let before: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name",
    ).fetch_all(db.pool()).await.unwrap();
    let ledger: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT version, name, applied_at FROM _migrations ORDER BY version",
    ).fetch_all(db.pool()).await.unwrap();

    assert!(crate::migrations::run_through_for_test(db.pool(), 178).await.is_err());
    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger' ORDER BY name",
    ).fetch_all(db.pool()).await.unwrap();
    let after_ledger: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT version, name, applied_at FROM _migrations ORDER BY version",
    ).fetch_all(db.pool()).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(after_ledger, ledger);
    assert_eq!(ledger.last().unwrap().0, 177);
}

#[tokio::test]
async fn migration_178_refuses_native_g2_legacy_metadata_authority() {
    let (db, assessment, receipt) = migration_176_pending_fixture().await;
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 177).await.unwrap(), 2);
    assert_eq!(crate::migrations::run_through_for_test(db.pool(), 178).await.unwrap(), 1);
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let snapshot = current_test_snapshot(&db, receipt.admitted.attempt_id.as_str()).await;
    let plan = RecordCloseWorktreeCleanupPlanRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: worktree.scope.clone(),
        snapshot: snapshot.clone(),
        resource: worktree.resource.clone(),
        administrative_dir_incarnation: "g2-forbidden".into(),
        administrative_dir: std::path::PathBuf::from("/tmp/g2-forbidden"),
    };
    let tombstone = BindCloseWorktreeFinalTombstoneRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: worktree.scope.clone(),
        snapshot: snapshot.clone(),
        resource: worktree.resource.clone(),
        tombstone: CloseWorktreeFinalTombstone {
            root: std::path::PathBuf::from("/tmp/g2-forbidden-tombstone"),
            device: 1,
            inode: 2,
            object_device: None,
            object_inode: None,
        },
    };
    let object = BindCloseWorktreeFinalTombstoneObjectRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: worktree.scope.clone(),
        snapshot,
        resource: worktree.resource.clone(),
        object_device: 1,
        object_inode: 2,
    };
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_worktree_cleanup_plans")
        .fetch_one(db.pool()).await.unwrap();
    for result in [
        db.record_close_worktree_cleanup_plan(plan).await,
        db.bind_close_worktree_final_tombstone(tombstone).await,
        db.bind_close_worktree_final_tombstone_object(object).await,
    ] {
        let Err(crate::DbError::CloseFoundationPrecondition(message)) = result else {
            panic!("generic G2 metadata API must refuse native authority: {result:?}");
        };
        assert!(message.contains("completed native close"), "{message}");
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_worktree_cleanup_plans")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(after, before);
}
