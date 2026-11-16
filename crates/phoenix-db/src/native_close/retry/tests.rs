use super::*;
use crate::native_close::{
    NativeCloseCommand, NativeCloseIntent, NativeCloseReceipt, NativeCloseRetryResult,
    NativeCloseRetrySourceAssessment,
};
use crate::workflow::LocalAuthorityResult;

fn opaque_resource(kind: RetiredResourceKind, value: &str) -> RetiredResourceIdentity {
    RetiredResourceIdentity::parse(
        kind,
        LossItemIdentity::Opaque(OpaqueIdentity::parse(value).unwrap()),
    )
    .unwrap()
}

#[derive(Clone)]
struct Assessment {
    command: NativeCloseCommand,
    proof: StoppedCloseOriginalRetirementProof,
    request: AdmitCloseSafeRetryRequest,
}

impl NativeCloseRetrySourceAssessment for Assessment {
    fn command(&self) -> &NativeCloseCommand {
        &self.command
    }

    fn original_proof(&self) -> &StoppedCloseOriginalRetirementProof {
        &self.proof
    }

    fn request(&self) -> &AdmitCloseSafeRetryRequest {
        &self.request
    }
}

async fn retry_command(
    db: &Database,
    suffix: &str,
    intent: NativeCloseIntent,
) -> NativeCloseCommand {
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let source = phoenix_core::domain::db_schema::SourceToolCall {
        message_id: format!("retry-source-{suffix}"),
        tool_use_id: format!("retry-call-{suffix}"),
    };
    let command = NativeCloseCommand::new(coordinator.id.clone(), source.clone(), intent).unwrap();
    let input = command.intent().source_input();
    db.update_conversation_state(
        &coordinator.id,
        &ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown { name: "close_conversation".into(), input: input.clone() },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                source.message_id,
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id,
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        },
    ).await.unwrap();
    command
}

fn native_retry_request(
    failure: &TerminalizeInitialCloseCleanupFailureRequest,
) -> AdmitCloseSafeRetryRequest {
    let mut request = safe_retry_request(failure);
    request
        .remaining_effects
        .sort_by_key(|effect| effect.resource.kind() == RetiredResourceKind::WorkScope);
    request
}

async fn assess(
    db: &Database,
    failure: &TerminalizeInitialCloseCleanupFailureRequest,
    suffix: &str,
) -> Assessment {
    let request = native_retry_request(failure);
    let proof = db
        .read_stopped_close_original_retirement_proof(&request.failed_run)
        .await
        .unwrap();
    let command = retry_command(
        db,
        suffix,
        NativeCloseIntent::ExplicitRetry {
            target: failure.source_product_conversation_id.clone(),
            attempt_id: failure.attempt_id.clone(),
            failed_run_ordinal: request.failed_run.ordinal,
        },
    ).await;
    Assessment { command, proof, request }
}

async fn fixture(
    db: &Database,
    resources: Vec<RetiredResourceIdentity>,
    complete_receipts: bool,
) -> TerminalizeInitialCloseCleanupFailureRequest {
    let mut failure = initial_cleanup_failure_fixture_with_resources(
        db, CloseStopCertainty::ShutdownUncertain, resources,
    ).await;
    if complete_receipts {
        let CloseCleanupFailureAuthority::ExpectedResource { scope, snapshot, resource } = &failure.authority else {
            panic!("expected inventory authority");
        };
        for remaining in &failure.remaining_resources {
            db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
                attempt_id: failure.attempt_id.clone(),
                scope: remaining.scope.clone(),
                snapshot: snapshot.clone(),
                resource: remaining.resource.clone(),
                outcome: RetirementOutcome::Residual { residual_reason: failure.reason },
                detail: Some(failure.detail.clone()),
            }).await.unwrap();
        }
        failure.remaining_resources = db.expected_close_cleanup_failure_resources(
            failure.attempt_id.as_str(), scope, resource,
        ).await.unwrap();
    }
    db.terminalize_initial_close_cleanup_failure(&failure).await.unwrap();
    failure
}

fn admitted(result: LocalAuthorityResult<DbResult<NativeCloseRetryResult>>) -> NativeCloseReceipt {
    match result {
        LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::NewlyAdmitted(receipt))) => receipt,
        other @ (LocalAuthorityResult::DurableFactEstablished(_)
        | LocalAuthorityResult::DurableFactUnclassified) => {
            panic!("expected classified admission, got {other:?}");
        }
    }
}

fn refused(result: LocalAuthorityResult<DbResult<NativeCloseRetryResult>>) {
    assert!(matches!(&result, LocalAuthorityResult::DurableFactEstablished(Err(_))), "{result:?}");
}

fn success_recorded(result: LocalAuthorityResult<DbResult<()>>) {
    assert!(matches!(&result, LocalAuthorityResult::DurableFactEstablished(Ok(()))), "{result:?}");
}

fn success_refused(result: LocalAuthorityResult<DbResult<()>>) {
    assert!(matches!(&result, LocalAuthorityResult::DurableFactEstablished(Err(_))), "{result:?}");
}

fn retry_completed(result: LocalAuthorityResult<DbResult<CloseObligation>>) -> CloseObligation {
    let LocalAuthorityResult::DurableFactEstablished(Ok(obligation)) = result else {
        panic!("expected classified native retry completion: {result:?}");
    };
    obligation
}

fn completion_refused(result: LocalAuthorityResult<DbResult<CloseObligation>>) {
    assert!(matches!(&result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(_)))
    ), "{result:?}");
}

async fn assert_no_admission(db: &Database, attempt: &CloseAttemptId) {
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1 AND run_ordinal > 1),
          (SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM native_close_retry_plans WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM native_close_initial_receipts WHERE attempt_id = ?1),
          (SELECT native_managed FROM close_obligations WHERE attempt_id = ?1)",
    ).bind(attempt.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(counts, (0, 0, 0, 0, 0));
}

async fn lookup_metadata(
    db: &Database,
    attempt: &CloseAttemptId,
) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
          (SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM native_close_initial_receipts WHERE attempt_id = ?1),
          (SELECT native_managed FROM close_obligations WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM native_close_retry_plans WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM coordinator_watch_events),
          (SELECT COUNT(*) FROM close_completion_retry_allowances WHERE attempt_id = ?1),
          (SELECT consumed FROM close_completion_retry_allowances WHERE attempt_id = ?1),
          (SELECT suppressed FROM close_completion_retry_allowances WHERE attempt_id = ?1)",
    )
    .bind(attempt.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn explicit_retry_receipt_lookup_returns_none_without_admission_or_outbox_changes() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "lookup-empty").await;
    let before = lookup_metadata(&db, &failure.attempt_id).await;

    assert_eq!(
        db.read_native_close_explicit_retry_receipt(&assessment.command)
            .await
            .unwrap(),
        None
    );

    let after = lookup_metadata(&db, &failure.attempt_id).await;
    assert_eq!(after, before);
}

#[tokio::test]
async fn explicit_retry_receipt_lookup_replays_successor_without_predecessor_proof() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "lookup-replay").await;
    let admitted = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let before = lookup_metadata(&db, &failure.attempt_id).await;
    // Admission created run 2, so the predecessor proof used above is no longer fresh. Do not
    // obtain a new predecessor assessment merely to replay the already durable receipt.
    let found = db
        .read_native_close_explicit_retry_receipt(&assessment.command)
        .await
        .unwrap();
    assert_eq!(found, Some(admitted.admitted));
    let after = lookup_metadata(&db, &failure.attempt_id).await;
    assert_eq!(after, before);
    let latest: i64 = sqlx::query_scalar(
        "SELECT MAX(run_ordinal) FROM close_runs WHERE attempt_id = ?1",
    )
    .bind(failure.attempt_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(latest, 2);
}

#[tokio::test]
async fn explicit_retry_receipt_lookup_revalidates_source_before_existing_receipt() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "lookup-stale").await;
    admitted(db.apply_native_close_explicit_retry(&assessment).await);
    db.update_conversation_state(
        assessment.command.source_transcript_id(),
        &ConvState::Idle,
    )
    .await
    .unwrap();
    let before = lookup_metadata(&db, &failure.attempt_id).await;

    assert!(db
        .read_native_close_explicit_retry_receipt(&assessment.command)
        .await
        .is_err());
    let after = lookup_metadata(&db, &failure.attempt_id).await;
    assert_eq!(after, before);
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM native_close_initial_receipts WHERE attempt_id = ?1",
    )
    .bind(failure.attempt_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipt_count, 1);
}

#[tokio::test]
async fn explicit_retry_receipt_lookup_refuses_nonretry_and_conflicting_source_binding() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let nonretry = retry_command(
        &db,
        "lookup-nonretry",
        NativeCloseIntent::Initial {
            target: failure.source_product_conversation_id.clone(),
        },
    )
    .await;
    assert!(db
        .read_native_close_explicit_retry_receipt(&nonretry)
        .await
        .is_err());
    assert_no_admission(&db, &failure.attempt_id).await;

    let receipt_assessment = assess(&db, &failure, "lookup-conflict").await;
    admitted(db.apply_native_close_explicit_retry(&receipt_assessment).await);
    // Reuse the source occurrence with a different predecessor binding, not another
    // fixed-ID foundation fixture in the same database.
    let conflicting = retry_command(
        &db,
        "lookup-conflict",
        NativeCloseIntent::ExplicitRetry {
            target: failure.source_product_conversation_id.clone(),
            attempt_id: failure.attempt_id.clone(),
            failed_run_ordinal: CloseRunOrdinal::INITIAL.checked_next().unwrap(),
        },
    )
    .await;
    assert!(matches!(
        db.read_native_close_explicit_retry_receipt(&conflicting)
            .await,
        Err(DbError::CloseFoundationConflict(_))
    ));
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM native_close_initial_receipts WHERE source_tool_use_id = 'retry-call-lookup-conflict'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipt_count, 1);
}

#[tokio::test]
async fn native_retry_atomically_binds_original_plan_and_replays_after_progress() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let mut assessment = assess(&db, &failure, "atomic").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    assert_eq!(receipt.admitted.run_ordinal.get(), 2);
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT native_managed FROM close_obligations WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM native_close_retry_plans WHERE attempt_id = ?1),
          (SELECT COUNT(*) FROM native_close_initial_receipts WHERE attempt_id = ?1 AND intent = 'explicit_retry'),
          (SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id = ?1 AND run_ordinal = 2)",
    ).bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(counts, (1, 1, 1, i64::try_from(assessment.request.remaining_effects.len()).unwrap()));
    let bound: (String, String, String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint, request_id, request_fingerprint
         FROM native_close_retry_plans WHERE attempt_id = ?1 AND run_ordinal = 2",
    ).bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(bound, (
        assessment.proof.snapshot.generation().into(), assessment.proof.snapshot.fingerprint().into(),
        assessment.command.request_id().to_string(), assessment.command.fingerprint().into(),
    ));
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, &assessment.request.remaining_effects[0], "exact identity retired",
    ).await);
    assessment.proof.outcomes.clear();
    assessment.request.remaining_effects.clear();
    match db.apply_native_close_explicit_retry(&assessment).await {
        LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::Replay(replay))) => assert_eq!(replay, receipt),
        other @ (LocalAuthorityResult::DurableFactEstablished(_)
        | LocalAuthorityResult::DurableFactUnclassified) => {
            panic!("expected exact replay, got {other:?}");
        }
    }
    db.update_conversation_state(assessment.command.source_transcript_id(), &ConvState::Idle)
        .await.unwrap();
    refused(db.apply_native_close_explicit_retry(&assessment).await);
}

#[tokio::test]
async fn native_retry_receipt_failure_rolls_back_adoption_successor_header_and_effects() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "rollback").await;
    sqlx::raw_sql("CREATE TRIGGER reject_test_retry_receipt BEFORE INSERT ON native_close_initial_receipts
        WHEN NEW.intent = 'explicit_retry' BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;")
        .execute(db.pool()).await.unwrap();
    refused(db.apply_native_close_explicit_retry(&assessment).await);
    assert_no_admission(&db, &failure.attempt_id).await;
    sqlx::query("DROP TRIGGER reject_test_retry_receipt").execute(db.pool()).await.unwrap();
    admitted(db.apply_native_close_explicit_retry(&assessment).await);
}

#[tokio::test]
async fn native_retry_revalidates_proof_and_refuses_missing_receipts_or_completion_only() {
    for mode in ["stale", "missing", "completion"] {
        let db = Database::open_in_memory().await.unwrap();
        let failure = fixture(&db, Vec::new(), mode != "missing").await;
        let mut assessment = assess(&db, &failure, mode).await;
        if mode == "stale" {
            assessment.proof.outcomes.clear();
        } else if mode == "completion" {
            assessment.request.remaining_effects.clear();
        }
        refused(db.apply_native_close_explicit_retry(&assessment).await);
        assert_no_admission(&db, &failure.attempt_id).await;
    }
}

#[tokio::test]
async fn native_retry_requires_complete_exact_scope_identity_and_rejects_process_local_resources() {
    for mode in ["missing-scope", "wrong-scope", "wrong-identity", "wrong-order", "process-local"] {
        let db = Database::open_in_memory().await.unwrap();
        let resources = if mode == "process-local" {
            vec![opaque_resource(RetiredResourceKind::BashProcessGroup, "epoch-bound-process")]
        } else { Vec::new() };
        let failure = fixture(&db, resources, true).await;
        let mut assessment = assess(&db, &failure, mode).await;
        let scope_index = assessment.request.remaining_effects.iter()
            .position(|effect| effect.resource.kind() == RetiredResourceKind::WorkScope).unwrap();
        match mode {
            "missing-scope" => { assessment.request.remaining_effects.remove(scope_index); }
            "wrong-scope" => {
                assessment.request.remaining_effects[scope_index].scope = WorkScopeId::parse("foreign-scope").unwrap();
            }
            "wrong-identity" => {
                assessment.request.remaining_effects[scope_index].resource = opaque_resource(RetiredResourceKind::WorkScope, "foreign-scope");
            }
            "wrong-order" => {
                let effects = &mut assessment.request.remaining_effects;
                let (resource_index, dependent_scope_index) = effects
                    .iter()
                    .enumerate()
                    .filter(|(_, effect)| effect.resource.kind() != RetiredResourceKind::WorkScope)
                    .find_map(|(index, effect)| {
                        effects
                            .iter()
                            .position(|candidate| {
                                candidate.scope == effect.scope
                                    && candidate.resource.kind() == RetiredResourceKind::WorkScope
                            })
                            .map(|scope_index| (index, scope_index))
                    })
                    .expect("fixture has a resource and its dependent scope retirement");
                assert!(resource_index < dependent_scope_index);
                effects.swap(resource_index, dependent_scope_index);
            }
            _ => {}
        }
        let result = db.apply_native_close_explicit_retry(&assessment).await;
        assert!(
            matches!(result, LocalAuthorityResult::DurableFactEstablished(Err(_))),
            "{mode}: {result:?}"
        );
        assert_no_admission(&db, &failure.attempt_id).await;
    }
}

#[tokio::test]
async fn native_retry_freezes_header_and_effect_plan_and_keeps_generic_fence() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "immutable").await;
    admitted(db.apply_native_close_explicit_retry(&assessment).await);
    for sql in [
        "UPDATE native_close_retry_plans SET plan_fingerprint = 'changed'",
        "DELETE FROM native_close_retry_plans",
        "INSERT OR REPLACE INTO native_close_retry_plans SELECT * FROM native_close_retry_plans",
        "UPDATE close_run_retry_effects SET identity_value = 'changed' WHERE run_ordinal = 2",
        "DELETE FROM close_run_retry_effects WHERE run_ordinal = 2",
        "INSERT OR REPLACE INTO close_run_retry_effects SELECT * FROM close_run_retry_effects WHERE run_ordinal = 2",
        "INSERT INTO close_run_retry_effects SELECT attempt_id, run_ordinal, ordinal + 100, scope,
           resource_kind, identity_kind, identity_codec, identity_value FROM close_run_retry_effects WHERE run_ordinal = 2",
    ] {
        assert!(sqlx::query(sqlx::AssertSqlSafe(sql)).execute(db.pool()).await.is_err(), "{sql}");
    }
    assert!(db.admit_close_safe_retry(&assessment.request).await.is_err());
    let stale = assess(&db, &failure, "stale-predecessor").await;
    refused(db.apply_native_close_explicit_retry(&stale).await);
    let violations = sqlx::query("PRAGMA foreign_key_check").fetch_all(db.pool()).await.unwrap();
    assert!(violations.is_empty());
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn native_retry_finalizes_exact_g1_successes_without_rebinding_current_g2() {
    let db = Database::open_in_memory().await.unwrap();
    let mut failure = initial_cleanup_failure_fixture_with_resources(
        &db, CloseStopCertainty::ShutdownUncertain, Vec::new(),
    ).await;
    let original = current_test_snapshot(&db, failure.attempt_id.as_str()).await;
    let original_effects = native_retry_request(&failure).remaining_effects;
    for remaining in &failure.remaining_resources {
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: failure.attempt_id.clone(),
            scope: remaining.scope.clone(),
            snapshot: original.clone(),
            resource: remaining.resource.clone(),
            outcome: RetirementOutcome::Residual { residual_reason: RetirementFailureReason::RemovalFailed },
            detail: Some("original residual".into()),
        }).await.unwrap();
    }
    let scope = failure.authority.scope().unwrap().clone();
    let worktree = failure.remaining_resources.iter()
        .find(|remaining| remaining.resource.kind() == RetiredResourceKind::Worktree)
        .unwrap().resource.clone();
    let work_scope = failure.remaining_resources.iter()
        .find(|remaining| remaining.scope == scope && remaining.resource.kind() == RetiredResourceKind::WorkScope)
        .unwrap().resource.clone();
    db.retry_close_retirement(&failure.attempt_id).await.unwrap();
    db.replace_close_inspection(ReplaceCloseInspectionRequest {
        attempt_id: failure.attempt_id.clone(),
        scopes: vec![ReplaceCloseInspectionScopeRequest {
            scope: scope.clone(),
            snapshot: CloseRetirementSnapshot::parse("g2-generation", "g2-fingerprint").unwrap(),
            losses: Vec::new(),
        }],
    }).await.unwrap();
    let g2 = current_test_snapshot(&db, failure.attempt_id.as_str()).await;
    capture_test_inventory(&db, failure.attempt_id.as_str(), &scope, &g2, Vec::new()).await;
    for (resource, outcome) in [
        (worktree.clone(), RetirementOutcome::Retired),
        (work_scope.clone(), RetirementOutcome::Residual { residual_reason: RetirementFailureReason::RemovalFailed }),
    ] {
        db.record_close_retirement_evidence(RecordCloseRetirementEvidenceRequest {
            attempt_id: failure.attempt_id.clone(),
            scope: scope.clone(),
            snapshot: g2.clone(),
            resource,
            outcome,
            detail: Some("G2 evidence is not original authority".into()),
        }).await.unwrap();
    }
    failure.authority = CloseCleanupFailureAuthority::ExpectedResource {
        scope: scope.clone(), snapshot: g2, resource: work_scope.clone(),
    };
    failure.remaining_resources = db.expected_close_cleanup_failure_resources(
        failure.attempt_id.as_str(), &scope, &work_scope,
    ).await.unwrap();
    failure.detail = "G2 evidence is not original authority".into();
    failure.occurred_at_us = Utc::now().timestamp_micros();
    db.terminalize_initial_close_cleanup_failure(&failure).await.unwrap();
    let mut assessment = assess(&db, &failure, "g2").await;
    assessment.request.remaining_effects = original_effects;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let bound: (String, String) = sqlx::query_as("SELECT inspection_generation, inspection_fingerprint FROM native_close_retry_plans")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(bound, (original.generation().into(), original.fingerprint().into()));
    let run = CloseRunRef { attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal };
    assert!(db.close_safe_retry_progress(&run).await.unwrap().pending.iter().any(|effect| effect.resource == worktree));
    let effect = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource == worktree).unwrap();
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, effect, "G1 worktree retired under native plan",
    ).await);
    let progress = db.close_safe_retry_progress(&run).await.unwrap();
    assert_eq!(progress.successful, vec![effect.clone()]);
    assert!(!progress.pending.contains(effect));
    let current = current_test_snapshot(&db, failure.attempt_id.as_str()).await;
    assert_ne!(current, original);
    let retained: (String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint FROM native_close_retry_plans",
    ).fetch_one(db.pool()).await.unwrap();
    assert_eq!(retained, bound);

    // A retired worktree is not retirement of its independently planned WorkScope.
    assert_eq!(progress.pending.len(), 1);
    assert_eq!(progress.pending[0].resource, work_scope);
    let incomplete = db.get_close_obligation(failure.attempt_id.as_str()).await.unwrap();
    let before_messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool()).await.unwrap();
    completion_refused(db.complete_native_close_retry(&assessment.command, &receipt.admitted).await);
    assert_eq!(db.get_close_obligation(failure.attempt_id.as_str()).await.unwrap(), incomplete);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(messages, before_messages);

    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, &progress.pending[0], "G1 scope independently retired",
    ).await);
    assert!(db.close_safe_retry_progress(&run).await.unwrap().pending.is_empty());
    // Even a completely successful native plan cannot use the unfenced legacy entry point.
    assert!(db.complete_close_safe_retry(&run).await.is_err());
    let predecessor = CloseRunRef::initial(failure.attempt_id.clone());
    let stopped = db.get_close_run(&predecessor).await.unwrap();
    let failures = db.list_close_cleanup_failures(failure.attempt_id.as_str()).await.unwrap();
    let metadata = lookup_metadata(&db, &failure.attempt_id).await;
    let completed = retry_completed(db.complete_native_close_retry(&assessment.command, &receipt.admitted).await);
    assert_eq!(completed.attempt_id(), &failure.attempt_id);
    assert_eq!(completed.close_outcome(), Some(CloseCompletionOutcome::Archived));
    assert_eq!(completed.snapshot(), Some(&current));
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Completed);
    assert_eq!(db.get_close_run(&predecessor).await.unwrap(), stopped);
    assert_eq!(stopped.status, CloseRunStatus::Stopped);
    assert_eq!(db.list_close_cleanup_failures(failure.attempt_id.as_str()).await.unwrap(), failures);
    assert_eq!(lookup_metadata(&db, &failure.attempt_id).await, metadata);
    let all_archived: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM conversations conversation
         JOIN close_attempt_participants participant ON participant.conversation_id = conversation.id
         WHERE participant.attempt_id = ?1 AND conversation.archived != 1)",
    ).bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert!(all_archived);
    let lifecycle: String = sqlx::query_scalar("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
        .bind(failure.source_product_conversation_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(lifecycle, "history");
    let message_id = format!("close-run-outcome:{}:{}", run.ordinal.get(), failure.attempt_id);
    let message: (String, String) = sqlx::query_as(
        "SELECT conversation_id, content FROM messages WHERE message_id = ?1",
    ).bind(&message_id).fetch_one(db.pool()).await.unwrap();
    let latest: String = sqlx::query_scalar(
        "SELECT conversation_id FROM close_attempt_members
         WHERE attempt_id = ?1 AND member_role IN ('latest', 'root_latest')",
    ).bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(message.0, latest);
    assert!(message.1.contains("completed; cleanup retired and conversation archived in History"));

    // The historical command receipt is observationally replayable. Finalization is
    // intentionally not: the completed run is no longer execution authority.
    assert_eq!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap(), Some(receipt.admitted.clone()));
    assert!(matches!(db.apply_native_close_explicit_retry(&assessment).await,
        LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::Replay(replay)))
            if replay.admitted == receipt.admitted));
    for _ in 0..2 {
        completion_refused(db.complete_native_close_retry(&assessment.command, &receipt.admitted).await);
    }
    assert_eq!(db.get_close_obligation(failure.attempt_id.as_str()).await.unwrap(), completed);
    assert_eq!(lookup_metadata(&db, &failure.attempt_id).await, metadata);
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(messages, before_messages + 1);
    let retained: (String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint FROM native_close_retry_plans",
    ).fetch_one(db.pool()).await.unwrap();
    assert_eq!(retained, bound);
}

#[tokio::test]
async fn native_retry_success_requires_exact_live_receipt_effect_and_evidence_before_replay() {
    use phoenix_core::domain::close::{GitPathIdentity, WorktreeFingerprint};

    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "success-binding").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let run = CloseRunRef {
        attempt_id: failure.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let effect = assessment.request.remaining_effects.iter()
        .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();
    let LossItemIdentity::Worktree(worktree) = effect.resource.identity() else {
        panic!("expected captured worktree");
    };
    // These share the persisted identity value but not the complete captured identity.
    let forged_effects: Vec<_> = [
        WorktreeIdentity::from_parts(
            worktree.id().clone(), WorktreeFingerprint::parse("wrong-fingerprint").unwrap(),
            worktree.locator().clone(),
        ),
        WorktreeIdentity::from_parts(
            worktree.id().clone(), worktree.fingerprint().clone(),
            GitPathIdentity::from_bytes(b"/wrong/worktree".to_vec()),
        ),
    ].into_iter().map(|identity| CloseSafeRetryEffect {
        scope: effect.scope.clone(),
        resource: RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree, LossItemIdentity::Worktree(identity),
        ).unwrap(),
    }).collect();
    let detail = "exact native worktree retired";
    for forged in &forged_effects {
        success_refused(db.record_native_close_retry_success(
            &assessment.command, &receipt.admitted, forged, detail,
        ).await);
    }
    success_refused(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, effect, " ",
    ).await);
    assert!(db.record_close_safe_retry_success(&run, effect, detail).await.is_err());
    assert!(db.close_safe_retry_progress(&run).await.unwrap().successful.is_empty());
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, effect, detail,
    ).await);
    let before: (i64, String, i64) = sqlx::query_as(
        "SELECT ordinal, detail, observed_at_us FROM close_run_retry_successes
         WHERE attempt_id = ?1 AND run_ordinal = ?2",
    ).bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_one(db.pool()).await.unwrap();
    success_recorded(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, effect, detail,
    ).await);
    success_refused(db.record_native_close_retry_success(
        &assessment.command, &receipt.admitted, effect, "conflicting evidence",
    ).await);
    for forged in &forged_effects {
        success_refused(db.record_native_close_retry_success(
            &assessment.command, &receipt.admitted, forged, detail,
        ).await);
    }
    let mut wrong_run = receipt.admitted.clone();
    wrong_run.run_ordinal = wrong_run.run_ordinal.checked_next().unwrap();
    success_refused(db.record_native_close_retry_success(
        &assessment.command, &wrong_run, effect, detail,
    ).await);
    let wrong_source = NativeCloseCommand::new(
        assessment.command.source_transcript_id().into(),
        phoenix_core::domain::db_schema::SourceToolCall {
            message_id: "foreign-message".into(), tool_use_id: "foreign-tool".into(),
        },
        assessment.command.intent().clone(),
    ).unwrap();
    success_refused(db.record_native_close_retry_success(
        &wrong_source, &receipt.admitted, effect, detail,
    ).await);
    assert!(db.record_close_safe_retry_success(&run, effect, detail).await.is_err());
    let after: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT ordinal, detail, observed_at_us FROM close_run_retry_successes
         WHERE attempt_id = ?1 AND run_ordinal = ?2",
    ).bind(run.attempt_id.as_str()).bind(run.ordinal.get()).fetch_all(db.pool()).await.unwrap();
    assert_eq!(after, vec![before]);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    assert_eq!(db.close_safe_retry_progress(&run).await.unwrap().successful, vec![effect.clone()]);
}

#[tokio::test]
async fn native_second_retry_discharges_only_prior_native_bound_success() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let first = assess(&db, &failure, "first-success").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&first).await);
    let run = CloseRunRef { attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal };
    let succeeded = first.request.remaining_effects[0].clone();
    success_recorded(db.record_native_close_retry_success(
        &first.command, &receipt.admitted, &succeeded, "native-bound retirement succeeded",
    ).await);
    db.classify_interrupted_close_run(&run).await.unwrap();
    let proof = db.read_stopped_close_original_retirement_proof(&run).await.unwrap();
    let command = retry_command(&db, "second-success", NativeCloseIntent::ExplicitRetry {
        target: failure.source_product_conversation_id.clone(),
        attempt_id: failure.attempt_id.clone(),
        failed_run_ordinal: run.ordinal,
    }).await;
    let mut request = first.request.clone();
    request.failed_run = run;
    request.observed_at_us = Utc::now().timestamp_micros();
    let mut next = Assessment { command, proof, request };
    refused(db.apply_native_close_explicit_retry(&next).await);
    next.request.remaining_effects.remove(0);
    assert!(!next.request.remaining_effects.is_empty());
    let receipt = admitted(db.apply_native_close_explicit_retry(&next).await);
    assert_eq!(receipt.admitted.run_ordinal.get(), 3);
    let third = CloseRunRef { attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal };
    let pending = db.close_safe_retry_progress(&third).await.unwrap().pending;
    assert_eq!(pending, next.request.remaining_effects);
    assert!(!pending.contains(&succeeded));
    let bindings: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM native_close_retry_plans plan
         JOIN native_close_initial_receipts receipt
           ON receipt.request_id = plan.request_id AND receipt.request_fingerprint = plan.request_fingerprint
          AND receipt.attempt_id = plan.attempt_id AND receipt.run_ordinal = plan.run_ordinal
         WHERE plan.attempt_id = ?1 AND receipt.intent = 'explicit_retry'",
    ).bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(bindings, 2);
}

#[tokio::test]
async fn native_retry_header_refuses_unmanaged_obligation() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "unmanaged-header").await;
    let error = sqlx::query(
        "INSERT INTO native_close_retry_plans
         (attempt_id, failed_run_ordinal, run_ordinal, request_id, request_fingerprint,
          inspection_generation, inspection_fingerprint, plan_fingerprint, evidence_kind, effect_count)
         VALUES (?1, 1, 2, ?2, ?3, ?4, ?5, 'test-plan', 'resource_plan', ?6)",
    ).bind(failure.attempt_id.as_str())
        .bind(assessment.command.request_id().to_string())
        .bind(assessment.command.fingerprint())
        .bind(assessment.proof.snapshot.generation())
        .bind(assessment.proof.snapshot.fingerprint())
        .bind(i64::try_from(assessment.request.remaining_effects.len()).unwrap())
        .execute(db.pool()).await.unwrap_err();
    assert!(error.to_string().contains("native retry header requires exact stopped original authority"), "{error}");
    assert_no_admission(&db, &failure.attempt_id).await;
    admitted(db.apply_native_close_explicit_retry(&assessment).await);
}

#[tokio::test]
async fn migration_161_preserves_existing_receipts_and_is_idempotent() {
    let db = Database::open_in_memory().await.unwrap();
    let ledger: String = sqlx::query_scalar("SELECT name FROM _migrations WHERE version = 161")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(ledger, "native_close_explicit_retry_admission");
    assert_eq!(crate::migrations::run_pending_migrations(db.pool()).await.unwrap(), 0);
    for name in ["native_close_retry_plans_require_original", "close_run_retry_effects_immutable", "native_close_retry_effects_no_direct_delete"] {
        let present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'trigger' AND name = ?1)")
            .bind(name).fetch_one(db.pool()).await.unwrap();
        assert!(present, "{name}");
    }
}
