use super::*;
use crate::native_close::{
    NativeCloseCommand, NativeCloseIntent, NativeCloseReceipt, NativeCloseRetryResult,
    NativeCloseRetrySourceAssessment,
};
use crate::workflow::LocalAuthorityResult;

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

async fn assess(
    db: &Database,
    failure: &TerminalizeInitialCloseCleanupFailureRequest,
    suffix: &str,
) -> Assessment {
    let request = safe_retry_request(failure);
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
    let failure = initial_cleanup_failure_fixture_with_resources(
        db, CloseStopCertainty::ShutdownUncertain, resources,
    ).await;
    if complete_receipts {
        let CloseCleanupFailureAuthority::ExpectedResource { snapshot, .. } = &failure.authority else {
            panic!("expected inventory authority");
        };
        for remaining in &failure.remaining_resources {
            insert_receipt(db, &failure.attempt_id, &remaining.scope, snapshot, &remaining.resource, "residual").await;
        }
    }
    db.terminalize_initial_close_cleanup_failure(&failure).await.unwrap();
    failure
}

async fn insert_receipt(
    db: &Database,
    attempt: &CloseAttemptId,
    scope: &WorkScopeId,
    snapshot: &CloseRetirementSnapshot,
    resource: &RetiredResourceIdentity,
    proof_kind: &str,
) {
    sqlx::query("INSERT INTO close_retirement_resources
        (attempt_id, scope, inspection_generation, inspection_fingerprint, resource_kind,
         identity_kind, identity_codec, identity_value, proof_kind, residual_reason, detail, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                CASE WHEN ?9 = 'residual' THEN 'removal_failed' END, 'fixture receipt', datetime('now'), datetime('now'))")
        .bind(attempt.as_str()).bind(scope.as_str())
        .bind(snapshot.generation()).bind(snapshot.fingerprint())
        .bind(resource.kind().as_str()).bind(resource.identity().identity_kind())
        .bind(resource.identity().codec()).bind(resource.identity().value()).bind(proof_kind)
        .execute(db.pool()).await.unwrap();
}

fn admitted(result: LocalAuthorityResult<DbResult<NativeCloseRetryResult>>) -> NativeCloseReceipt {
    match result {
        LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::NewlyAdmitted(receipt))) => receipt,
        other => panic!("expected classified admission, got {other:?}"),
    }
}

fn refused(result: LocalAuthorityResult<DbResult<NativeCloseRetryResult>>) {
    assert!(matches!(&result, LocalAuthorityResult::DurableFactEstablished(Err(_))), "{result:?}");
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
    assert_eq!(counts, (1, 1, 1, assessment.request.remaining_effects.len() as i64));
    let bound: (String, String, String, String) = sqlx::query_as(
        "SELECT inspection_generation, inspection_fingerprint, request_id, request_fingerprint
         FROM native_close_retry_plans WHERE attempt_id = ?1 AND run_ordinal = 2",
    ).bind(failure.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(bound, (
        assessment.proof.snapshot.generation().into(), assessment.proof.snapshot.fingerprint().into(),
        assessment.command.request_id().to_string(), assessment.command.fingerprint().into(),
    ));
    let run = CloseRunRef { attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal };
    db.record_close_safe_retry_success(&run, &assessment.request.remaining_effects[0], "exact identity retired")
        .await.unwrap();
    assessment.proof.outcomes.clear();
    assessment.request.remaining_effects.clear();
    match db.apply_native_close_explicit_retry(&assessment).await {
        LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::Replay(replay))) => assert_eq!(replay, receipt),
        other => panic!("expected exact replay, got {other:?}"),
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
            "wrong-order" => { assessment.request.remaining_effects.swap(0, scope_index); }
            _ => {}
        }
        refused(db.apply_native_close_explicit_retry(&assessment).await);
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

#[tokio::test]
async fn native_retry_uses_g1_residuals_not_g2_success() {
    let db = Database::open_in_memory().await.unwrap();
    let mut failure = initial_cleanup_failure_fixture_with_resources(
        &db, CloseStopCertainty::ShutdownUncertain, Vec::new(),
    ).await;
    let original = current_test_snapshot(&db, failure.attempt_id.as_str()).await;
    let original_effects = safe_retry_request(&failure).remaining_effects;
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
        .find(|remaining| remaining.resource.kind() == RetiredResourceKind::WorkScope)
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
}

#[tokio::test]
async fn native_second_retry_discharges_only_prior_native_bound_success() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let first = assess(&db, &failure, "first-success").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&first).await);
    let run = CloseRunRef { attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal };
    let succeeded = first.request.remaining_effects[0].clone();
    db.record_close_safe_retry_success(&run, &succeeded, "native-bound retirement succeeded")
        .await.unwrap();
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
