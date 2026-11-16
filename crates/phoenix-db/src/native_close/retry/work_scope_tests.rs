use phoenix_core::work_scope::{
    WorkScopeId, WorkScopeRetirementBlocker, WorkScopeRetirementOutcome,
    WorkScopeRetirementPrecondition,
};

fn native_scope_precondition(scope: &WorkScopeId) -> WorkScopeRetirementPrecondition {
    WorkScopeRetirementPrecondition::after_runtime_inventory_found_no_live_resource(scope.clone())
}

fn native_scope_effects(
    assessment: &Assessment,
) -> (&CloseSafeRetryEffect, &CloseSafeRetryEffect, &CloseSafeRetryEffect) {
    let effects = &assessment.request.remaining_effects;
    let worktree = effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let scope = effects.iter().find(|effect| {
        effect.resource.kind() == RetiredResourceKind::WorkScope && effect.scope == worktree.scope
    }).unwrap();
    let independent = effects.iter().find(|effect| {
        effect.resource.kind() == RetiredResourceKind::WorkScope && effect.scope != scope.scope
    }).unwrap();
    (worktree, scope, independent)
}

fn native_scope_outcome(
    result: LocalAuthorityResult<DbResult<WorkScopeRetirementOutcome>>,
) -> WorkScopeRetirementOutcome {
    let LocalAuthorityResult::DurableFactEstablished(Ok(outcome)) = result else {
        panic!("expected classified scope retirement outcome: {result:?}");
    };
    outcome
}

fn native_scope_refused(result: LocalAuthorityResult<DbResult<WorkScopeRetirementOutcome>>) {
    assert!(matches!(&result, LocalAuthorityResult::DurableFactEstablished(Err(_))), "{result:?}");
}

async fn native_scope_lifecycle(db: &Database, scope: &WorkScopeId) -> String {
    sqlx::query_scalar("SELECT lifecycle FROM work_scopes WHERE id = ?1")
        .bind(scope.as_str()).fetch_one(db.pool()).await.unwrap()
}

async fn native_scope_durable_state(db: &Database, attempt: &CloseAttemptId) -> serde_json::Value {
    let scopes: String = sqlx::query_scalar(
        "SELECT json_group_array(json_array(id, lifecycle, retired_at, retired_reason, updated_at))
         FROM (SELECT * FROM work_scopes ORDER BY id)",
    ).fetch_one(db.pool()).await.unwrap();
    serde_json::json!({
        "scopes": scopes,
        "retry": retry_dispatch_state(db, attempt).await,
    })
}

#[tokio::test]
async fn native_scope_retirement_requires_g1_worktree_success_and_separate_scope_success() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let (worktree, scope, independent) = native_scope_effects(&assessment);
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal,
    };
    let g2 = current_test_snapshot(&db, run.attempt_id.as_str()).await;
    assert_ne!(g2, assessment.proof.snapshot);
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "active");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
    let before = native_scope_durable_state(&db, &run.attempt_id).await;
    native_scope_refused(db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, scope, native_scope_precondition(&scope.scope),
        "G2 evidence cannot discharge the original worktree",
    ).await);
    assert_eq!(native_scope_durable_state(&db, &run.attempt_id).await, before);

    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "exact G1 worktree retired",
    ).await);
    let retry_state = retry_dispatch_state(&db, &run.attempt_id).await;
    let progress = db.close_safe_retry_progress(&run).await.unwrap();
    assert_eq!(progress.successful, vec![worktree.clone()]);
    assert!(progress.pending.contains(scope));
    assert!(progress.pending.contains(independent));
    assert_eq!(native_scope_outcome(db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, scope, native_scope_precondition(&scope.scope),
        "retire original captured participant scope",
    ).await), WorkScopeRetirementOutcome::Retired);
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "retired");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
    assert_eq!(db.close_safe_retry_progress(&run).await.unwrap(), progress);
    assert_eq!(retry_dispatch_state(&db, &run.attempt_id).await, retry_state);
    assert_eq!(current_test_snapshot(&db, run.attempt_id.as_str()).await, g2);

    let retired = native_scope_durable_state(&db, &run.attempt_id).await;
    assert_eq!(native_scope_outcome(db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, scope, native_scope_precondition(&scope.scope),
        "repeat must retain the original retirement reason and timestamp",
    ).await), WorkScopeRetirementOutcome::AlreadyRetired);
    assert_eq!(native_scope_durable_state(&db, &run.attempt_id).await, retired);
    assert_eq!(db.close_safe_retry_progress(&run).await.unwrap(), progress);

    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, scope, "scope lifecycle retirement completed",
    ).await);
    let successful = native_scope_durable_state(&db, &run.attempt_id).await;
    native_scope_refused(db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, scope, native_scope_precondition(&scope.scope),
        "a successful effect is no longer pending",
    ).await);
    assert_eq!(native_scope_durable_state(&db, &run.attempt_id).await, successful);
    let progress = db.close_safe_retry_progress(&run).await.unwrap();
    assert_eq!(progress.successful, vec![worktree.clone(), scope.clone()]);
    assert_eq!(progress.pending, vec![independent.clone()]);
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
}

#[tokio::test]
async fn native_scope_retirement_rejects_forged_identity_kind_precondition_and_empty_reason() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let (worktree, scope, independent) = native_scope_effects(&assessment);
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "original worktree prerequisite met",
    ).await);
    let forged = CloseSafeRetryEffect {
        scope: scope.scope.clone(),
        resource: opaque_resource(RetiredResourceKind::WorkScope, independent.scope.as_str()),
    };
    let before = native_scope_durable_state(&db, &receipt.admitted.attempt_id).await;
    for (effect, precondition_scope, reason) in [
        (&forged, &scope.scope, "forged scope identity"),
        (worktree, &worktree.scope, "not a WorkScope effect"),
        (scope, &independent.scope, "precondition belongs to another real scope"),
        (scope, &scope.scope, " \n\t "),
    ] {
        native_scope_refused(db.retire_work_scope_for_native_close_retry(
            &assessment.command, &receipt.admitted, effect,
            native_scope_precondition(precondition_scope), reason,
        ).await);
        assert_eq!(native_scope_durable_state(&db, &receipt.admitted.attempt_id).await, before);
    }
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "active");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
}

#[tokio::test]
async fn native_scope_retirement_rechecks_source_before_lifecycle_mutation() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let (worktree, scope, independent) = native_scope_effects(&assessment);
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "original worktree prerequisite met",
    ).await);
    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    let before = native_scope_durable_state(&db, &receipt.admitted.attempt_id).await;
    let result = db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, scope, native_scope_precondition(&scope.scope),
        "stale source cannot retire its retained pending scope",
    ).await;
    let LocalAuthorityResult::DurableFactEstablished(Err(
        DbError::CloseFoundationPrecondition(message),
    )) = result else {
        panic!("expected classified stale-source refusal: {result:?}");
    };
    assert!(message.contains("current executing source"), "{message}");
    assert_eq!(native_scope_durable_state(&db, &receipt.admitted.attempt_id).await, before);
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "active");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
}

#[tokio::test]
async fn generic_scope_retirement_refuses_native_completed_retry_even_with_current_g1() {
    let (db, assessment, receipt) = migration_163_pending_fixture_with_options(false, false).await;
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    let (worktree, scope, independent) = native_scope_effects(&assessment);
    assert_eq!(current_test_snapshot(&db, receipt.admitted.attempt_id.as_str()).await,
        assessment.proof.snapshot);
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "exact original worktree retired",
    ).await);
    let obligation: (String, i64) = sqlx::query_as(
        "SELECT phase, native_managed FROM close_obligations WHERE attempt_id = ?1",
    ).bind(receipt.admitted.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(obligation, ("completed".into(), 1));
    let before = native_scope_durable_state(&db, &receipt.admitted.attempt_id).await;
    let result = db.retire_work_scope_for_close_attempt(
        &receipt.admitted.attempt_id, native_scope_precondition(&scope.scope),
        "generic API must not borrow native completed retry authority",
    ).await;
    assert!(matches!(result, Err(DbError::CloseFoundationPrecondition(_))), "{result:?}");
    assert_eq!(native_scope_durable_state(&db, &receipt.admitted.attempt_id).await, before);
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "active");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
}

#[tokio::test]
async fn native_scope_retirement_preserves_unresolved_ownership_repair_blocker() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let (_, scope, independent) = native_scope_effects(&assessment);
    sqlx::query(
        "INSERT INTO product_conversation_work_scope_missing_owners (work_scope_id, state)
         VALUES (?1, 'needs_repair')",
    ).bind(independent.scope.as_str()).execute(db.pool()).await.unwrap();
    let before = native_scope_durable_state(&db, &receipt.admitted.attempt_id).await;
    assert_eq!(native_scope_outcome(db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, independent,
        native_scope_precondition(&independent.scope), "repair must not be exempted as a participant",
    ).await), WorkScopeRetirementOutcome::Blocked(
        WorkScopeRetirementBlocker::UnresolvedProductConversationOwnership,
    ));
    assert_eq!(native_scope_durable_state(&db, &receipt.admitted.attempt_id).await, before);
    assert!(db.work_scope_has_unresolved_product_ownership(&independent.scope).await.unwrap());
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "active");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
}

#[tokio::test]
async fn native_scope_retirement_refuses_stopped_latest_run_without_mutation() {
    let (db, assessment, receipt) = native_resource_failure_fixture().await;
    let (worktree, scope, independent) = native_scope_effects(&assessment);
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, worktree, "original worktree prerequisite met",
    ).await);
    let run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal,
    };
    db.classify_interrupted_close_run(&run).await.unwrap();
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let before = native_scope_durable_state(&db, &run.attempt_id).await;
    native_scope_refused(db.retire_work_scope_for_native_close_retry(
        &assessment.command, &receipt.admitted, scope, native_scope_precondition(&scope.scope),
        "stopped latest plan cannot retire a pending scope",
    ).await);
    assert_eq!(native_scope_durable_state(&db, &run.attempt_id).await, before);
    assert_eq!(native_scope_lifecycle(&db, &scope.scope).await, "active");
    assert_eq!(native_scope_lifecycle(&db, &independent.scope).await, "active");
}
