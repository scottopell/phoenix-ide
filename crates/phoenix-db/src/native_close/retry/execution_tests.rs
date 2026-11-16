use phoenix_core::domain::close::{GitPathIdentity, WorktreeFingerprint};

#[tokio::test]
async fn generic_resource_failure_still_terminalizes_and_replays_non_native_retry() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let run = db.admit_close_safe_retry(&native_retry_request(&failure)).await.unwrap();
    let mut request = failure.clone();
    request.failure_occurrence_id = run.failure_occurrence_id();
    request.occurred_at_us = Utc::now().timestamp_micros();
    request.detail = "retained non-native retry failure".into();
    let obligation = db.terminalize_close_run_cleanup_failure(&run, &request).await.unwrap();
    let retained = migration_163_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    assert_eq!(db.terminalize_close_run_cleanup_failure(&run, &request).await.unwrap(), obligation);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    assert_eq!(migration_163_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, counts);
}

async fn native_resource_failure_fixture() -> (Database, Assessment, NativeCloseReceipt) {
    // This fixture installs 162 once, then upgrades that same DB to 163.
    let (db, assessment, receipt) = migration_163_pending_fixture().await;
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    (db, assessment, receipt)
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
        serde_json::from_str(&migration_163_retained_state(db).await).unwrap();
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
    let retained = migration_163_retained_state(&db).await;
    for _ in 0..2 {
        assert_eq!(retry_completed(db.terminalize_native_close_retry_resource_failure(
            &assessment.command, &receipt.admitted, request.clone(),
        ).await), obligation);
    }
    let mut changes = Vec::new();
    let mut changed = request.clone();
    changed.detail.push_str(" changed");
    changes.push(changed);
    let mut changed = request.clone();
    changed.reason = RetirementFailureReason::Interrupted;
    changes.push(changed);
    let mut changed = request.clone();
    changed.occurred_at_us -= 1;
    changes.push(changed);
    let mut changed = request.clone();
    changed.remaining_resources[failed_index].disposition = CloseCleanupResourceDisposition::Unattempted;
    changes.push(changed);
    let mut changed = request.clone();
    changed.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped {
        confirmed_at_us: request.occurred_at_us,
    };
    changes.push(changed);
    for changed in changes {
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
    assert_eq!(migration_163_retained_state(&db).await, retained);
    assert_eq!(native_failure_side_effect_counts(&db).await, after);

    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let retained = migration_163_retained_state(&db).await;
    let result = db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(message))) = result else {
        panic!("stale source must refuse before stopped occurrence replay: {result:?}");
    };
    assert!(message.contains("current executing source"), "{message}");
    assert_eq!(migration_163_retained_state(&db).await, retained);
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
    let retained = migration_163_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    let mut changes = Vec::new();
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
        changes.push(changed.clone());
        changed.remaining_resources[index].resource = forged.clone();
        changes.push(changed);
        let mut children_only = request.clone();
        children_only.remaining_resources[index].resource = forged;
        changes.push(children_only);
    }
    let mut changed = request.clone();
    let CloseCleanupFailureAuthority::ExpectedResource { snapshot, .. } = &mut changed.authority else { unreachable!() };
    *snapshot = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    changes.push(changed);
    let mut changed = request.clone();
    changed.failure_occurrence_id.push_str("-wrong");
    changes.push(changed);
    let mut changed = request.clone();
    changed.source_product_conversation_id = ProductConversationId::parse("wrong-target").unwrap();
    changes.push(changed);
    let mut changed = request.clone();
    changed.authority = CloseCleanupFailureAuthority::AttemptInterrupted;
    changes.push(changed);
    let mut changed = request.clone();
    changed.remaining_resources.reverse();
    changes.push(changed);
    for scope_index in pending.iter().enumerate().filter_map(|(index, effect)| {
        (effect.resource.kind() == RetiredResourceKind::WorkScope).then_some(index)
    }) {
        let mut changed = request.clone();
        changed.remaining_resources.remove(scope_index);
        changes.push(changed);
        let mut changed = request.clone();
        changed.remaining_resources[scope_index].scope = WorkScopeId::parse("foreign-scope").unwrap();
        changes.push(changed);
    }
    let mut changed = request.clone();
    changed.remaining_resources[index].disposition = CloseCleanupResourceDisposition::Residual;
    changes.push(changed);
    let mut changed = request.clone();
    changed.detail = " \n\t".into();
    changes.push(changed);
    for occurred in [-1, 0, i64::MAX] {
        let mut changed = request.clone();
        changed.occurred_at_us = occurred;
        changes.push(changed);
    }
    for confirmed in [-1, request.occurred_at_us + 1] {
        let mut changed = request.clone();
        changed.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped { confirmed_at_us: confirmed };
        changes.push(changed);
    }
    for changed in changes {
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
    assert_eq!(migration_163_retained_state(&db).await, retained);
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
    let retained = migration_163_retained_state(&db).await;
    let counts = native_failure_side_effect_counts(&db).await;
    completion_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
    assert_eq!(migration_163_retained_state(&db).await, retained);
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
    let retained = migration_163_retained_state(&db).await;
    completion_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
    assert_eq!(migration_163_retained_state(&db).await, retained);
    let tmux_effect = pending.iter().find(|effect| effect.resource == tmux).unwrap();
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, tmux_effect, "original tmux stopped",
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
        let retained = migration_163_retained_state(&db).await;
        let counts = native_failure_side_effect_counts(&db).await;
        let result = db.terminalize_native_close_retry_resource_failure(
            &assessment.command, &receipt.admitted, request.clone(),
        ).await;
        assert!(matches!(result, LocalAuthorityResult::DurableFactEstablished(Err(_))), "{result:?}");
        assert_eq!(migration_163_retained_state(&db).await, retained);
        assert_eq!(native_failure_side_effect_counts(&db).await, counts);
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
        sqlx::raw_sql("DROP TRIGGER native_failure_test_late_error").execute(db.pool()).await.unwrap();
    }
    retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await);
}
