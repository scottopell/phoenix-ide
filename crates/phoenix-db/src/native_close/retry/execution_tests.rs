use phoenix_core::domain::close::{GitPathIdentity, WorktreeFingerprint};

#[tokio::test]
async fn generic_resource_failure_still_terminalizes_and_replays_non_native_retry() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let run = db.admit_close_safe_retry(&safe_retry_request(&failure)).await.unwrap();
    let mut request = failure.clone();
    request.failure_occurrence_id = run.failure_occurrence_id();
    request.occurred_at_us = Utc::now().timestamp_micros();
    request.detail = "retained non-native retry failure".into();
    let obligation = db.terminalize_close_run_cleanup_failure(&run, &request).await.unwrap();
    let retained = migration_180_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    assert_eq!(db.terminalize_close_run_cleanup_failure(&run, &request).await.unwrap(), obligation);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_retry_worktree_metadata_is_source_bound_to_pending_original_g1_effect() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let effect = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap().clone();
    let make_request = || RecordCloseWorktreeCleanupPlanRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: effect.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(),
        resource: effect.resource.clone(),
        administrative_dir_incarnation: "native-retry-test".into(),
        administrative_dir: std::path::PathBuf::from("/tmp/native-retry-admin"),
    };
    assert_ne!(current_test_snapshot(&db, receipt.admitted.attempt_id.as_str()).await,
        assessment.proof.snapshot);

    // A fresh plan must not bypass its exact dispatch parent, even with live
    // native retry authority. The runtime must establish that parent first.
    let before_dispatch = retry_dispatch_state(&db, &receipt.admitted.attempt_id).await;
    let without_dispatch = db.record_native_close_retry_worktree_cleanup_plan(
        &assessment.command, &receipt.admitted, make_request(),
    ).await;
    assert!(matches!(without_dispatch,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::Sqlx(
            sqlx::Error::Database(ref error)
        ))) if error.is_foreign_key_violation()
    ));
    assert_eq!(retry_dispatch_state(&db, &receipt.admitted.attempt_id).await, before_dispatch);
    assert!(db.close_worktree_cleanup_plan(
        &receipt.admitted.attempt_id, &effect.scope, &assessment.proof.snapshot, &effect.resource,
    ).await.unwrap().is_none());

    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, &effect,
    ).await);
    let request = make_request();
    success_recorded(db.record_native_close_retry_worktree_cleanup_plan(
        &assessment.command, &receipt.admitted, request.clone(),
    ).await);
    success_recorded(db.record_native_close_retry_worktree_cleanup_plan(
        &assessment.command, &receipt.admitted, request,
    ).await);

    let tombstone_request = BindCloseWorktreeFinalTombstoneRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: effect.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(),
        resource: effect.resource.clone(),
        tombstone: CloseWorktreeFinalTombstone {
            root: std::path::PathBuf::from("/tmp/native-retry-tombstone"),
            device: 10,
            inode: 20,
            object_device: None,
            object_inode: None,
        },
    };
    success_recorded(db.bind_native_close_retry_worktree_final_tombstone(
        &assessment.command, &receipt.admitted, tombstone_request.clone(),
    ).await);
    success_recorded(db.bind_native_close_retry_worktree_final_tombstone(
        &assessment.command, &receipt.admitted, tombstone_request.clone(),
    ).await);
    let object_request = BindCloseWorktreeFinalTombstoneObjectRequest {
        attempt_id: receipt.admitted.attempt_id.clone(),
        scope: effect.scope.clone(),
        snapshot: assessment.proof.snapshot.clone(),
        resource: effect.resource.clone(),
        object_device: 10,
        object_inode: 20,
    };
    success_recorded(db.bind_native_close_retry_worktree_final_tombstone_object(
        &assessment.command, &receipt.admitted, object_request.clone(),
    ).await);
    success_recorded(db.bind_native_close_retry_worktree_final_tombstone_object(
        &assessment.command, &receipt.admitted, object_request.clone(),
    ).await);

    let g2 = current_test_snapshot(&db, receipt.admitted.attempt_id.as_str()).await;
    let mut g2_plan = make_request();
    g2_plan.snapshot = g2.clone();
    let mut g2_tombstone = tombstone_request.clone();
    g2_tombstone.snapshot = g2.clone();
    let mut g2_object = object_request.clone();
    g2_object.snapshot = g2;
    for result in [
        db.record_native_close_retry_worktree_cleanup_plan(
            &assessment.command, &receipt.admitted, g2_plan,
        ).await.established(),
        db.bind_native_close_retry_worktree_final_tombstone(
            &assessment.command, &receipt.admitted, g2_tombstone,
        ).await.established(),
        db.bind_native_close_retry_worktree_final_tombstone_object(
            &assessment.command, &receipt.admitted, g2_object,
        ).await.established(),
    ] {
        assert!(result.unwrap().is_err(), "current G2 cannot replace immutable G1 metadata identity");
    }

    assert_generic_native_metadata_refused(
        &db, make_request(), tombstone_request.clone(), object_request.clone(),
    ).await;

    let mut forged = make_request();
    forged.snapshot = current_test_snapshot(&db, receipt.admitted.attempt_id.as_str()).await;
    assert!(db.record_native_close_retry_worktree_cleanup_plan(
        &assessment.command, &receipt.admitted, forged,
    ).await.established().unwrap().is_err());

    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    // Replacing the live source durably stops its admitted successor. Compare
    // metadata refusal against that established state, not the prior running run.
    let retained = migration_180_retained_state(&db).await;
    let stale_results = [
        db.record_native_close_retry_worktree_cleanup_plan(
            &assessment.command, &receipt.admitted, make_request(),
        ).await,
        db.bind_native_close_retry_worktree_final_tombstone(
            &assessment.command, &receipt.admitted, tombstone_request,
        ).await,
        db.bind_native_close_retry_worktree_final_tombstone_object(
            &assessment.command, &receipt.admitted, object_request,
        ).await,
    ];
    for stale in stale_results {
        let LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(message))) = stale else {
            panic!("stale source must refuse metadata writes: {stale:?}");
        };
        assert!(message.contains("current executing source"), "{message}");
    }
    assert_eq!(migration_180_retained_state(&db).await, retained);
}

async fn assert_generic_native_metadata_refused(
    db: &Database,
    plan: RecordCloseWorktreeCleanupPlanRequest,
    tombstone: BindCloseWorktreeFinalTombstoneRequest,
    object: BindCloseWorktreeFinalTombstoneObjectRequest,
) {
    for result in [
        db.record_close_worktree_cleanup_plan(plan).await,
        db.bind_close_worktree_final_tombstone(tombstone).await,
        db.bind_close_worktree_final_tombstone_object(object).await,
    ] {
        let Err(DbError::CloseFoundationPrecondition(message)) = result else {
            panic!("generic metadata API must reject completed native attempts: {result:?}");
        };
        assert!(message.contains("completed native close"), "{message}");
    }
}

async fn native_resource_failure_fixture() -> (Database, Assessment, NativeCloseReceipt) {
    let (db, assessment, receipt) = migration_180_pending_fixture().await;
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    (db, assessment, receipt)
}

#[tokio::test]
async fn native_retry_execution_context_reads_original_typed_plan_without_mutation() {
    use std::fmt::Write as _;

    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let g2 = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    assert_ne!(g2, assessment.proof.snapshot);
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "original worktree completed",
    ).await);
    let retained = migration_180_retained_state(&db).await;
    let metadata = lookup_metadata(&db, &run.attempt_id).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let context = db.read_native_close_retry_execution_context(
        &assessment.command, &receipt.admitted,
    ).await.unwrap();
    assert_eq!(context.snapshot, assessment.proof.snapshot);
    assert_eq!(context.scopes, assessment.proof.scopes);
    assert_eq!(context.resources, assessment.proof.resources);
    assert_eq!(context.effects, assessment.request.remaining_effects);
    assert_eq!(context.progress, db.close_safe_retry_progress(&run).await.unwrap());
    assert_eq!(context.progress.successful, vec![worktree.clone()]);
    assert_eq!(context.progress.pending, assessment.request.remaining_effects.iter()
        .filter(|effect| *effect != worktree).cloned().collect::<Vec<_>>());
    assert!(context.resources.iter().all(|resource| resource.snapshot == context.snapshot));
    assert!(context.scopes.iter().any(|scope| matches!(
        &scope.captured_worktree,
        Some(phoenix_core::domain::close::CapturedWorktreeIdentity::Resolved(_))
    )));
    let retained_g2 = db.list_close_retirement_inspections(run.attempt_id.as_str()).await.unwrap();
    assert!(!context.worktree_snapshots.is_empty());
    assert_eq!(context.worktree_snapshots.len(), context.scopes.iter()
        .filter(|scope| scope.captured_worktree.is_some()).count());
    let mut generation = String::from("v1");
    let mut fingerprint = String::from("v1");
    for (target, snapshot) in &context.worktree_snapshots {
        let scope = target.scope.as_str();
        write!(generation, "{}:{scope}{}:{}", scope.len(), snapshot.generation().len(), snapshot.generation()).unwrap();
        write!(fingerprint, "{}:{scope}{}:{}", scope.len(), snapshot.fingerprint().len(), snapshot.fingerprint()).unwrap();
        assert!(retained_g2.iter().any(|inspection| inspection.target == *target
            && inspection.snapshot != *snapshot), "retained G2 poison must not supply G1 inputs");
    }
    assert_eq!(generation, context.snapshot.generation());
    assert_eq!(fingerprint, context.snapshot.fingerprint());
    assert_eq!(current_test_snapshot(&db, run.attempt_id.as_str()).await, g2);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(lookup_metadata(&db, &run.attempt_id).await, metadata);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
async fn native_retry_execution_context_checks_live_source_before_retained_receipt() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    assert!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap().is_some());
    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let retained = migration_180_retained_state(&db).await;
    let metadata = lookup_metadata(&db, &receipt.admitted.attempt_id).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let result = db.read_native_close_retry_execution_context(
        &assessment.command, &receipt.admitted,
    ).await;
    let Err(DbError::CloseFoundationPrecondition(message)) = result else {
        panic!("stale source must refuse before receipt replay: {result:?}");
    };
    assert!(message.contains("current executing source"), "{message}");
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(lookup_metadata(&db, &receipt.admitted.attempt_id).await, metadata);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
async fn native_retry_execution_context_refuses_forged_receipt_wrong_failed_run_and_stopped_latest() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let retained = migration_180_retained_state(&db).await;
    let metadata = lookup_metadata(&db, &receipt.admitted.attempt_id).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let mut forged = receipt.admitted.clone();
    forged.run_ordinal = forged.run_ordinal.checked_next().unwrap();
    assert!(db.read_native_close_retry_execution_context(&assessment.command, &forged).await.is_err());
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(lookup_metadata(&db, &receipt.admitted.attempt_id).await, metadata);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
    let pending = assessment.request.remaining_effects.clone();
    let request = native_resource_failure_request(&assessment, &receipt, &pending, 0);
    retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
    let retained = migration_180_retained_state(&db).await;
    let metadata = lookup_metadata(&db, &receipt.admitted.attempt_id).await;
    let counts = native_failure_side_effect_counts(&db).await;
    assert!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap().is_some());
    assert!(db.read_native_close_retry_execution_context(&assessment.command, &receipt.admitted).await.is_err());
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(lookup_metadata(&db, &receipt.admitted.attempt_id).await, metadata);
    let wrong_failed = retry_command(&db, "context-wrong-failed", NativeCloseIntent::ExplicitRetry {
        target: receipt.admitted.target.clone(),
        attempt_id: receipt.admitted.attempt_id.clone(),
        failed_run_ordinal: receipt.admitted.run_ordinal,
    }).await;
    // Installing another executing source may suppress the existing allowance.
    // Only the reader call below is required to be free of side effects.
    let counts = native_failure_side_effect_counts(&db).await;
    let metadata = lookup_metadata(&db, &receipt.admitted.attempt_id).await;
    assert!(db.read_native_close_retry_execution_context(&wrong_failed, &receipt.admitted).await.is_err());
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(lookup_metadata(&db, &receipt.admitted.attempt_id).await, metadata);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
async fn native_retry_pre_admission_context_reads_exact_stopped_g1_without_mutation() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let g2 = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    assert_ne!(g2, assessment.proof.snapshot);
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let request = native_resource_failure_request(&assessment, &receipt, &pending, 0);
    retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);

    let retained = migration_180_retained_state(&db).await;
    let metadata = lookup_metadata(&db, &run.attempt_id).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let context = db.read_native_close_retry_pre_admission_context(&run).await.unwrap();
    assert_eq!(context.original.snapshot, assessment.proof.snapshot);
    assert_eq!(context.original.scopes, assessment.proof.scopes);
    assert_eq!(context.original.resources, assessment.proof.resources);
    assert!(!context.worktree_snapshots.is_empty());
    assert!(context.worktree_snapshots.iter().all(|(_, snapshot)| snapshot != &g2));
    assert_eq!(current_test_snapshot(&db, run.attempt_id.as_str()).await, g2);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(lookup_metadata(&db, &run.attempt_id).await, metadata);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
async fn native_retry_pre_admission_context_keeps_historical_stopped_readable_and_rejects_running_successor() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let failed_run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let pending = db.close_safe_retry_progress(&failed_run).await.unwrap().pending;
    let request = native_resource_failure_request(&assessment, &receipt, &pending, 0);
    retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);

    let mut retry = assessment.request.clone();
    retry.failed_run = failed_run.clone();
    retry.remaining_effects = pending;
    retry.observed_at_us = Utc::now().timestamp_micros();
    let proof = db.read_stopped_close_original_retirement_proof(&failed_run).await.unwrap();
    let command = retry_command(&db, "preadmission-successor", NativeCloseIntent::ExplicitRetry {
        target: receipt.admitted.target.clone(),
        attempt_id: failed_run.attempt_id.clone(),
        failed_run_ordinal: failed_run.ordinal,
    }).await;
    let successor = Assessment { command, request: retry, proof };
    let admitted = admitted(db.apply_native_close_explicit_retry(&successor).await);
    // Restoring the predecessor tool state here would legitimately stop the
    // running successor through the native source fence.

    let retained = migration_180_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    assert_eq!(db.read_native_close_retry_pre_admission_context(&failed_run).await.unwrap()
        .original.snapshot, assessment.proof.snapshot);
    let running_successor = CloseRunRef {
        attempt_id: admitted.admitted.attempt_id.clone(),
        ordinal: admitted.admitted.run_ordinal,
    };
    assert!(db.read_native_close_retry_pre_admission_context(&running_successor).await.is_err());
    let wrong_predecessor = CloseRunRef {
        attempt_id: failed_run.attempt_id.clone(),
        ordinal: admitted.admitted.run_ordinal,
    };
    assert!(db.read_native_close_retry_pre_admission_context(&wrong_predecessor).await.is_err());
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

fn native_resource_failure_request(
    assessment: &Assessment,
    receipt: &NativeCloseReceipt,
    pending: &[CloseSafeRetryEffect],
    failed_index: usize,
) -> TerminalizeInitialCloseCleanupFailureRequest {
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let failed = &pending[failed_index];
    TerminalizeInitialCloseCleanupFailureRequest {
        attempt_id: run.attempt_id.clone(),
        failure_occurrence_id: run.failure_occurrence_id(),
        source_product_conversation_id: receipt.admitted.target.clone(),
        authority: CloseCleanupFailureAuthority::ExpectedResource {
            scope: failed.scope.clone(),
            snapshot: assessment.proof.snapshot.clone(),
            resource: failed.resource.clone(),
        },
        reason: RetirementFailureReason::RemovalFailed,
        detail: "native original-generation resource failure".into(),
        stop_certainty: CloseStopCertainty::ShutdownUncertain,
        occurred_at_us: Utc::now().timestamp_micros(),
        remaining_resources: pending.iter().enumerate().map(|(index, effect)| {
            CloseCleanupFailureResource {
                scope: effect.scope.clone(),
                resource: effect.resource.clone(),
                disposition: if index == failed_index {
                    CloseCleanupResourceDisposition::Failed
                } else {
                    CloseCleanupResourceDisposition::Unattempted
                },
            }
        }).collect(),
    }
}

async fn native_failure_side_effect_counts(db: &Database) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM messages),
          (SELECT COUNT(*) FROM coordinator_watch_events),
          (SELECT COUNT(*) FROM close_cleanup_failures),
          (SELECT COUNT(*) FROM close_cleanup_failure_resources),
          (SELECT COUNT(*) FROM close_run_retry_successes)",
    ).fetch_one(db.pool()).await.unwrap()
}

async fn native_failure_immutable_state(db: &Database) -> serde_json::Value {
    let mut state: serde_json::Value =
        serde_json::from_str(&migration_180_retained_state(db).await).unwrap();
    let object = state.as_object_mut().unwrap();
    object.remove("runs");
    object.remove("failures");
    state
}

#[tokio::test]
async fn native_resource_failure_uses_g1_pending_tail_and_exact_stopped_replay() {
    for failed_index in 0..2 {
        assert_native_resource_failure_pending_tail_replay(failed_index).await;
    }
}

#[allow(clippy::too_many_lines)]
async fn assert_native_resource_failure_pending_tail_replay(failed_index: usize) {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let original_run = db.get_close_run(&CloseRunRef::initial(run.attempt_id.clone())).await.unwrap();
    let g2 = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    assert_ne!(g2, assessment.proof.snapshot);
    let worktree = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "G1 worktree retired",
    ).await);
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    assert_eq!(pending.len(), 2);
    let expected_pending: Vec<_> = assessment.request.remaining_effects.iter()
        .filter(|effect| *effect != worktree).cloned().collect();
    assert_eq!(pending, expected_pending);
    assert!(pending.iter().all(|effect| effect.resource.kind() == RetiredResourceKind::WorkScope));
    assert_ne!(pending[0].scope, pending[1].scope);
    let request = native_resource_failure_request(&assessment, &receipt, &pending, failed_index);
    assert_eq!(request.remaining_resources.len(), 2);
    for (index, (child, effect)) in request.remaining_resources.iter().zip(&pending).enumerate() {
        assert_eq!(child.scope, effect.scope);
        assert_eq!(child.resource, effect.resource);
        assert_eq!(child.disposition, if index == failed_index {
            CloseCleanupResourceDisposition::Failed
        } else {
            CloseCleanupResourceDisposition::Unattempted
        });
    }
    let immutable = native_failure_immutable_state(&db).await;
    let metadata = lookup_metadata(&db, &run.attempt_id).await;
    let before = native_failure_side_effect_counts(&db).await;
    let completed_index = assessment.request.remaining_effects.iter()
        .position(|effect| effect == worktree).unwrap();
    let already_succeeded = native_resource_failure_request(
        &assessment, &receipt, &assessment.request.remaining_effects, completed_index,
    );
    completion_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, already_succeeded,
    ).await);
    assert_eq!(native_failure_side_effect_counts(&db).await, before);
    let obligation = retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request.clone(),
    ).await);
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    assert_eq!(obligation.snapshot(), Some(&g2));
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    assert_eq!(db.get_close_run(&CloseRunRef::initial(run.attempt_id.clone())).await.unwrap(), original_run);
    assert_eq!(native_failure_immutable_state(&db).await, immutable);
    let after = native_failure_side_effect_counts(&db).await;
    assert_eq!(after, (before.0 + 1, before.1 + 1, before.2 + 1, before.3 + 2, before.4));
    let mut expected_metadata = metadata;
    expected_metadata.4 += 1;
    assert_eq!(lookup_metadata(&db, &run.attempt_id).await, expected_metadata);
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.last().unwrap().run_ordinal, run.ordinal);
    assert_eq!(failures.last().unwrap().occurrence, request);
    let retained = migration_180_retained_state(&db).await;
    for _ in 0..2 {
        assert_eq!(retry_completed(db.terminalize_native_close_retry_resource_failure(
            &assessment.command, &receipt.admitted, request.clone(),
        ).await), obligation);
    }
    let mut conflicting_requests = Vec::new();
    let mut changed = request.clone();
    changed.detail.push_str(" changed");
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.reason = RetirementFailureReason::Interrupted;
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.occurred_at_us -= 1;
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.remaining_resources[failed_index].disposition = CloseCleanupResourceDisposition::Unattempted;
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped {
        confirmed_at_us: request.occurred_at_us,
    };
    conflicting_requests.push(changed);
    for changed in conflicting_requests {
        completion_refused(db.terminalize_native_close_retry_resource_failure(
            &assessment.command, &receipt.admitted, changed,
        ).await);
    }
    for effect in &pending {
        success_refused(db.record_native_close_retry_success(
            &assessment.command, &receipt.admitted, effect, "too late",
        ).await);
    }
    completion_refused(db.complete_native_close_retry(&assessment.command, &receipt.admitted).await);
    assert!(db.terminalize_close_run_cleanup_failure(&run, &request).await.is_err());
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, after);

    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let retained = migration_180_retained_state(&db).await;
    let result = db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(message))) = result else {
        panic!("stale source must refuse before stopped occurrence replay: {result:?}");
    };
    assert!(message.contains("current executing source"), "{message}");
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, after);
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn native_resource_failure_refuses_full_identity_and_caller_evidence_mismatches() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let index = pending.iter().position(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let request = native_resource_failure_request(&assessment, &receipt, &pending, index);
    assert_eq!(pending, assessment.request.remaining_effects);
    assert_eq!(pending.len(), 3);
    let retained = migration_180_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let mut conflicting_requests = Vec::new();
    let LossItemIdentity::Worktree(captured) = pending[index].resource.identity() else { panic!("worktree"); };
    for (fingerprint, locator) in [
        (WorktreeFingerprint::parse("changed-native-fingerprint").unwrap(), captured.locator().clone()),
        (captured.fingerprint().clone(), GitPathIdentity::from_bytes(b"/different/native/locator".to_vec())),
    ] {
        let mut changed = request.clone();
        let CloseCleanupFailureAuthority::ExpectedResource { resource, .. } = &mut changed.authority else { unreachable!() };
        *resource = RetiredResourceIdentity::parse(RetiredResourceKind::Worktree,
            LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                captured.id().clone(), fingerprint, locator,
            ))).unwrap();
        let forged = resource.clone();
        conflicting_requests.push(changed.clone());
        changed.remaining_resources[index].resource = forged.clone();
        conflicting_requests.push(changed);
        let mut children_only = request.clone();
        children_only.remaining_resources[index].resource = forged;
        conflicting_requests.push(children_only);
    }
    let mut changed = request.clone();
    let CloseCleanupFailureAuthority::ExpectedResource { snapshot, .. } = &mut changed.authority else { unreachable!() };
    *snapshot = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.failure_occurrence_id.push_str("-wrong");
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.source_product_conversation_id = ProductConversationId::parse("wrong-target").unwrap();
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.authority = CloseCleanupFailureAuthority::AttemptInterrupted;
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.remaining_resources.reverse();
    conflicting_requests.push(changed);
    for scope_index in pending.iter().enumerate().filter_map(|(index, effect)| {
        (effect.resource.kind() == RetiredResourceKind::WorkScope).then_some(index)
    }) {
        let mut changed = request.clone();
        changed.remaining_resources.remove(scope_index);
        conflicting_requests.push(changed);
        let mut changed = request.clone();
        changed.remaining_resources[scope_index].scope = WorkScopeId::parse("foreign-scope").unwrap();
        conflicting_requests.push(changed);
    }
    let mut changed = request.clone();
    changed.remaining_resources[index].disposition = CloseCleanupResourceDisposition::Residual;
    conflicting_requests.push(changed);
    let mut changed = request.clone();
    changed.detail = " \n\t".into();
    conflicting_requests.push(changed);
    for occurred in [-1, 0, i64::MAX] {
        let mut changed = request.clone();
        changed.occurred_at_us = occurred;
        conflicting_requests.push(changed);
    }
    for confirmed in [-1, request.occurred_at_us + 1] {
        let mut changed = request.clone();
        changed.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: confirmed };
        conflicting_requests.push(changed);
    }
    for changed in conflicting_requests {
        completion_refused(db.terminalize_native_close_retry_resource_failure(
            &assessment.command, &receipt.admitted, changed,
        ).await);
    }
    let mut wrong_admitted = receipt.admitted.clone();
    wrong_admitted.run_ordinal = run.ordinal.checked_next().unwrap();
    completion_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &wrong_admitted, request.clone(),
    ).await);
    assert!(db.terminalize_close_run_cleanup_failure(&run, &request).await.is_err());
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
async fn native_resource_failure_replay_refuses_a_stopped_predecessor_after_successor_admission() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let request = native_resource_failure_request(&assessment, &receipt, &pending, 0);
    retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request.clone(),
    ).await);
    let original_source = db.get_conversation(assessment.command.source_transcript_id())
        .await.unwrap().state;
    let mut retry = assessment.request.clone();
    retry.failed_run = run.clone();
    retry.remaining_effects = pending;
    retry.observed_at_us = Utc::now().timestamp_micros();
    let proof = db.read_stopped_close_original_retirement_proof(&run).await.unwrap();
    let command = retry_command(&db, "failure-successor", NativeCloseIntent::ExplicitRetry {
        target: receipt.admitted.target.clone(),
        attempt_id: run.attempt_id.clone(),
        failed_run_ordinal: run.ordinal,
    }).await;
    let successor = Assessment { command, request: retry, proof };
    let admitted_successor = admitted(db.apply_native_close_explicit_retry(&successor).await);
    assert_eq!(admitted_successor.admitted.run_ordinal.get(), run.ordinal.get() + 1);
    db.update_conversation_state(assessment.command.source_transcript_id(), &original_source)
        .await.unwrap();
    let retained = migration_180_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    completion_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

#[tokio::test]
async fn native_resource_failure_requires_truthful_original_process_success() {
    let db = Database::open_in_memory().await.unwrap();
    let tmux = opaque_resource(RetiredResourceKind::TmuxServer, "native-failure-tmux");
    let initial = fixture(&db, vec![tmux.clone()], true).await;
    let assessment = assess(&db, &initial, "failure-process-proof").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let run = CloseRunRef { attempt_id: initial.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let index = pending.iter().position(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let mut request = native_resource_failure_request(&assessment, &receipt, &pending, index);
    request.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped {
        confirmed_at_us: request.occurred_at_us,
    };
    let retained = migration_180_retained_state(&db).await;
    completion_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    let tmux_effect = pending.iter().find(|effect| effect.resource == tmux).unwrap();
    use crate::native_close::NativeRetryTmuxProof;
    success_refused(db.record_native_close_retry_tmux_success(
        &assessment.command, &receipt.admitted, tmux_effect, NativeRetryTmuxProof::Dispatched, || async { Ok(()) },
    ).await);
    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, tmux_effect,
    ).await);
    success_refused(db.record_native_close_retry_tmux_success(
        &assessment.command, &receipt.admitted, tmux_effect, NativeRetryTmuxProof::Dispatched, || async { Ok(()) },
    ).await);
    assert!(db.close_safe_retry_progress(&run).await.unwrap().pending.contains(tmux_effect));
    let (dispatch, lease) = db.confirm_native_retry_tmux_dispatch_with_entry(
        &assessment.command, &receipt.admitted, tmux_effect, || Ok(()),
    ).await;
    assert!(matches!(dispatch, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    assert_eq!(lease, Some(()));
    success_recorded(db.record_native_close_retry_tmux_success(
        &assessment.command, &receipt.admitted, tmux_effect, NativeRetryTmuxProof::Dispatched, || async { Ok(()) },
    ).await);
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let index = pending.iter().position(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let mut request = native_resource_failure_request(&assessment, &receipt, &pending, index);
    request.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped {
        confirmed_at_us: request.occurred_at_us,
    };
    let obligation = retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request.clone(),
    ).await);
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::ArchivedCleanupAttention));
    let children = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(children.last().unwrap().occurrence, request);
    assert!(request.remaining_resources.iter().all(|child| child.resource != tmux));
}

#[tokio::test]
async fn native_resource_failure_rolls_back_late_outbox_and_outcome_message_errors() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let request = native_resource_failure_request(&assessment, &receipt, &pending, 0);
    for trigger in [
        "CREATE TRIGGER native_failure_test_late_error BEFORE INSERT ON coordinator_watch_events
         WHEN NEW.event_id LIKE 'close-failure:2:%'
         BEGIN SELECT RAISE(ABORT, 'late native failure outbox error'); END",
        "CREATE TRIGGER native_failure_test_late_error BEFORE INSERT ON messages
         WHEN NEW.message_id LIKE 'close-run-outcome:2:%'
         BEGIN SELECT RAISE(ABORT, 'late native failure outcome message error'); END",
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(trigger)).execute(db.pool()).await.unwrap();
        let retained = migration_180_retained_state(&db).await;
        let counts = native_failure_side_effect_counts(&db).await;
        let result = db.terminalize_native_close_retry_resource_failure(
            &assessment.command, &receipt.admitted, request.clone(),
        ).await;
        assert!(matches!(result, LocalAuthorityResult::DurableFactEstablished(Err(_))), "{result:?}");
        assert_eq!(migration_180_retained_state(&db).await, retained);
        assert_eq!(native_failure_side_effect_counts(&db).await, counts);
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
        sqlx::raw_sql("DROP TRIGGER native_failure_test_late_error").execute(db.pool()).await.unwrap();
    }
    retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
}

async fn native_tmux_retry_fixture() -> (Database, Assessment, NativeCloseReceipt, CloseSafeRetryEffect) {
    // The generic failure fixture becomes native only after `assess` installs a
    // real executing source and native explicit-retry admission succeeds.
    let db = Database::open_in_memory().await.unwrap();
    let tmux = opaque_resource(
        RetiredResourceKind::TmuxServer,
        "tmux-v1:L3RtcC9uYXRpdmUtcmV0cnk:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    );
    let failure = fixture(&db, vec![tmux], true).await;
    let assessment = assess(&db, &failure, "tmux-retry").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let effect = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::TmuxServer)
        .expect("native retry plan includes a pending tmux effect").clone();
    (db, assessment, receipt, effect)
}

#[tokio::test]
async fn native_retry_tmux_replacement_settles_without_dispatch_and_replays_exactly() {
    use crate::native_close::NativeRetryTmuxProof;

    let (db, assessment, receipt, effect) = native_tmux_retry_fixture().await;
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let sealed_identity = effect.resource.identity().value().to_string();
    let replacement = |sealed_identity: String, token: &str| NativeRetryTmuxProof::Replacement {
        sealed_identity,
        token: uuid::Uuid::parse_str(token).unwrap(),
    };
    let record = |proof| db.record_native_close_retry_tmux_success(
        &assessment.command, &receipt.admitted, &effect, proof, || async { Ok(()) },
    );
    let original = "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA";
    let new = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let wrong_identity = sealed_identity.replace("L3RtcC9uYXRpdmUtcmV0cnk", "L3RtcC93cm9uZw");
    let before = db.close_safe_retry_progress(&run).await.unwrap();
    success_refused(record(NativeRetryTmuxProof::Dispatched).await);
    success_refused(record(replacement(sealed_identity.clone(), original)).await);
    success_refused(record(replacement(wrong_identity, new)).await);
    assert_eq!(db.close_safe_retry_progress(&run).await.unwrap(), before);

    let rechecked = std::cell::Cell::new(false);
    success_refused(
        db.record_native_close_retry_tmux_success(
            &assessment.command,
            &receipt.admitted,
            &effect,
            replacement(sealed_identity.clone(), new),
            || async {
                rechecked.set(true);
                Err(crate::DbError::CloseFoundationPrecondition(
                    "replacement changed at writer entry".into(),
                ))
            },
        )
        .await,
    );
    assert!(rechecked.get());
    assert_eq!(db.close_safe_retry_progress(&run).await.unwrap(), before);
    let replacement_history: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resource_history
         WHERE attempt_id = ?1 AND tmux_replacement_token IS NOT NULL",
    )
    .bind(run.attempt_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(replacement_history, 0);

    success_recorded(record(replacement(sealed_identity.clone(), new)).await);
    let history_sequence = || {
        sqlx::query_scalar::<_, i64>(
            "SELECT seq FROM sqlite_sequence WHERE name = 'close_retirement_resource_history'",
        )
        .fetch_one(db.pool())
    };
    let sequence = history_sequence().await.unwrap();
    success_recorded(record(replacement(sealed_identity.clone(), new)).await);
    assert_eq!(history_sequence().await.unwrap(), sequence);
    assert!(db.close_safe_retry_progress(&run).await.unwrap().successful.contains(&effect));
    success_refused(record(replacement(
        sealed_identity,
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    )).await);
    success_refused(record(NativeRetryTmuxProof::Dispatched).await);
    let proofs: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint, proof_kind, tmux_replacement_token
         FROM close_retirement_resource_history
         WHERE attempt_id = ?1 AND tmux_replacement_token IS NOT NULL",
    )
    .bind(run.attempt_id.as_str())
    .fetch_all(db.pool()).await.unwrap();
    assert_eq!(proofs, vec![(
        assessment.proof.snapshot.generation().to_owned(),
        assessment.proof.snapshot.fingerprint().to_owned(),
        "absence_adopted".to_owned(),
        new.to_owned(),
    )]);
    assert!(!db.close_retirement_resource_was_dispatched(
        &run.attempt_id, &effect.scope, &assessment.proof.snapshot, &effect.resource,
    ).await.unwrap());
}

#[tokio::test]
async fn native_retry_tmux_dispatch_records_exact_pending_identity_and_refuses_mismatches() {
    use std::cell::Cell;

    let (db, assessment, receipt, effect) = native_tmux_retry_fixture().await;
    let attempt = &receipt.admitted.attempt_id;
    let before = retry_dispatch_state(&db, attempt).await;
    let dispatch_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1",
    ).bind(attempt.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(dispatch_count, 0);

    let entry_calls = Cell::new(0);
    let (entry_result, lease) = db.record_native_close_retry_retirement_dispatch_with_entry(
        &assessment.command, &receipt.admitted, &effect, || {
            entry_calls.set(entry_calls.get() + 1);
            Ok("entered")
        },
    ).await;
    success_recorded(entry_result);
    assert_eq!(lease, Some("entered"));
    assert_eq!(entry_calls.get(), 1);
    let dispatched = retry_dispatch_state(&db, attempt).await;
    assert_ne!(dispatched, before);
    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, &effect,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, dispatched);

    let mut wrong_identity = effect.clone();
    wrong_identity.resource = opaque_resource(RetiredResourceKind::TmuxServer, "forged-tmux");
    let retained_before_refusal = migration_180_retained_state(&db).await;
    let (wrong_identity_result, wrong_identity_lease) =
        db.record_native_close_retry_retirement_dispatch_with_entry(
            &assessment.command, &receipt.admitted, &wrong_identity, || {
                entry_calls.set(entry_calls.get() + 1);
                Ok("must not enter")
            },
        ).await;
    success_refused(wrong_identity_result);
    assert_eq!(wrong_identity_lease, None);
    assert_eq!(entry_calls.get(), 1, "refused identity must not enter");
    assert_eq!(migration_180_retained_state(&db).await, retained_before_refusal);
    success_refused(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, &wrong_identity,
    ).await);
    let mut wrong_kind = effect.clone();
    wrong_kind.resource = opaque_resource(RetiredResourceKind::WorkScope, "wrong-kind-tmux");
    success_refused(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, &wrong_kind,
    ).await);
    assert_eq!(retry_dispatch_state(&db, attempt).await, dispatched);

    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let retained = migration_180_retained_state(&db).await;
    let fenced_dispatch = retry_dispatch_state(&db, attempt).await;
    assert_ne!(fenced_dispatch, dispatched, "source loss terminalizes the running retry");
    success_refused(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, &effect,
    ).await);
    assert_eq!(migration_180_retained_state(&db).await, retained);
    assert_eq!(retry_dispatch_state(&db, attempt).await, fenced_dispatch);
}

#[tokio::test]
async fn native_retry_tmux_first_effect_requires_exact_source_dispatch_identity_and_snapshot() {
    use std::cell::Cell;
    use crate::native_close::{NativeCloseInitialEntryRelease, NativeCloseInitialFirstEffectError};

    let (db, assessment, receipt, effect) = native_tmux_retry_fixture().await;

    let live_source = db.get_conversation(assessment.command.source_transcript_id())
        .await.unwrap().state;
    success_recorded(db.record_native_close_retry_retirement_dispatch(
        &assessment.command, &receipt.admitted, &effect,
    ).await);
    let callback_calls = Cell::new(0);
    let premature = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &live_source, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &effect.resource, || panic!("preclaim cannot finalize"),
    ).await;
    assert!(matches!(premature, Err(NativeCloseInitialFirstEffectError::Rejected { .. })));
    let (confirmed, lease) = db.confirm_native_retry_tmux_dispatch_with_entry(
        &assessment.command, &receipt.admitted, &effect, || Ok(()),
    ).await;
    success_recorded(confirmed);
    assert_eq!(lease, Some(()));
    let duplicate_approval = db.with_native_close_retry_tmux_approval(
        &assessment.command, &receipt.admitted, &live_source, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &effect.resource, || panic!("must not dispatch twice"),
    ).await;
    assert!(matches!(duplicate_approval, Err(NativeCloseInitialFirstEffectError::Rejected { .. })));
    let (result, release) = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &live_source, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &effect.resource, || {
            callback_calls.set(callback_calls.get() + 1);
            "handed off"
        },
    ).await.unwrap();
    assert_eq!(result, "handed off");
    assert!(matches!(release, NativeCloseInitialEntryRelease::Confirmed));
    assert_eq!(callback_calls.get(), 1);

    let before = migration_180_retained_state(&db).await;
    let wrong_identity = opaque_resource(RetiredResourceKind::TmuxServer, "forged-tmux");
    let wrong_identity_result = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &live_source, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &wrong_identity, || {
            callback_calls.set(callback_calls.get() + 1);
        },
    ).await;
    assert!(matches!(wrong_identity_result,
        Err(NativeCloseInitialFirstEffectError::Rejected { .. })));

    let wrong_source = ConvState::Idle;
    let wrong_source_result = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &wrong_source, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &effect.resource, || {
            callback_calls.set(callback_calls.get() + 1);
        },
    ).await;
    assert!(matches!(wrong_source_result,
        Err(NativeCloseInitialFirstEffectError::Rejected { .. })));

    let wrong_kind = opaque_resource(RetiredResourceKind::WorkScope, "wrong-kind-tmux");
    let wrong_kind_result = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &live_source, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &wrong_kind, || {
            callback_calls.set(callback_calls.get() + 1);
        },
    ).await;
    assert!(matches!(wrong_kind_result,
        Err(NativeCloseInitialFirstEffectError::Rejected { .. })));

    let wrong_snapshot = CloseRetirementSnapshot::parse("forged-generation", "forged-fingerprint").unwrap();
    let wrong_snapshot_result = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &live_source, effect.scope.clone(),
        wrong_snapshot, &effect.resource, || {
            callback_calls.set(callback_calls.get() + 1);
        },
    ).await;
    assert!(matches!(wrong_snapshot_result,
        Err(NativeCloseInitialFirstEffectError::Rejected { .. })));
    assert_eq!(callback_calls.get(), 1);
    assert_eq!(migration_180_retained_state(&db).await, before);

    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let retained = migration_180_retained_state(&db).await;
    let stopped_source_result = db.with_native_close_retry_tmux_first_effect(
        &assessment.command, &receipt.admitted, &ConvState::Idle, effect.scope.clone(),
        assessment.proof.snapshot.clone(), &effect.resource, || {
            callback_calls.set(callback_calls.get() + 1);
        },
    ).await;
    assert!(matches!(stopped_source_result,
        Err(NativeCloseInitialFirstEffectError::Rejected { .. })));
    assert_eq!(callback_calls.get(), 1, "source Stop must not invoke the callback");
    assert_eq!(migration_180_retained_state(&db).await, retained);
}
