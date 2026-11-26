use crate::native_close::{NativeCloseCapturedLossRetryAssessment, NativeCloseRetryResult};
use phoenix_core::domain::close::{CloseRetirementTarget, CloseRunStatus};

struct CapturedLossAssessment {
    command: NativeCloseCommand,
    context: NativeCloseCapturedLossRetryContext,
    inspection: crate::ReplaceCloseInspectionRequest,
    observed_at_us: i64,
    evidence: String,
}

impl NativeCloseCapturedLossRetryAssessment for CapturedLossAssessment {
    fn command(&self) -> &NativeCloseCommand {
        &self.command
    }

    fn context(&self) -> &NativeCloseCapturedLossRetryContext {
        &self.context
    }

    fn inspection(&self) -> &crate::ReplaceCloseInspectionRequest {
        &self.inspection
    }

    fn observed_at_us(&self) -> i64 {
        self.observed_at_us
    }

    fn safety_evidence(&self) -> &str {
        &self.evidence
    }
}

async fn captured_loss_assessment() -> (Database, CapturedLossAssessment) {
    captured_loss_assessment_with_wire_shape(false).await
}

async fn captured_loss_assessment_with_wire_shape(
    legacy_flat_origin: bool,
) -> (Database, CapturedLossAssessment) {
    let (db, initial, failed_run) = if legacy_flat_origin {
        retained_flat_loss_origin().await
    } else {
        retained_loss_origin().await
    };
    // Finish the original tool round through its owning API before installing a
    // distinct retry occurrence. Replacing an unacknowledged source suppresses it.
    let (assistant, result) = initial_result_round(&initial, &failed_run);
    db.persist_tool_round_and_state(
        initial.source_transcript_id(),
        &assistant,
        &[result],
        &ConvState::Idle,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    let acknowledgement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM native_close_initial_result_acknowledgements WHERE attempt_id = ?1",
    )
    .bind(failed_run.attempt_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(acknowledgement_count, 1);
    let retry_intent = NativeCloseIntent::ExplicitRetry {
        target: initial.intent().target().clone(),
        attempt_id: failed_run.attempt_id.clone(),
        failed_run_ordinal: failed_run.ordinal,
    };
    let input = if legacy_flat_origin {
        retry_intent.legacy_source_input()
    } else {
        retry_intent.source_input()
    };
    let command = NativeCloseCommand::new_with_source_input(
        initial.source_transcript_id().to_owned(),
        SourceToolCall {
            message_id: "captured-retry-message".into(),
            tool_use_id: "captured-retry-call".into(),
        },
        retry_intent,
        &input,
    )
    .unwrap();
    let executing = ConvState::ToolExecuting {
        current_tool: ToolCall::new(
            &command.source().tool_use_id,
            ToolInput::Unknown {
                name: "close_conversation".into(),
                input: input.clone(),
            },
        ),
        remaining_tools: Vec::new(),
        completed_results: Vec::new(),
        pending_sub_agents: Vec::new(),
        assistant_message: AssistantMessage::new(
            command.source().message_id.clone(),
            vec![ContentBlock::ToolUse {
                id: command.source().tool_use_id.clone(),
                name: "close_conversation".into(),
                input,
            }],
            None,
            None,
        ),
    };
    db.update_conversation_state(command.source_transcript_id(), &executing)
        .await
        .unwrap();
    let context = db
        .read_native_close_captured_loss_retry_context(command.intent().target(), &failed_run)
        .await
        .unwrap();
    let inspection = crate::ReplaceCloseInspectionRequest {
        attempt_id: failed_run.attempt_id,
        scopes: context
            .inspections()
            .iter()
            .map(|old| crate::ReplaceCloseInspectionScopeRequest {
                scope: old.target.scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("retry-clean-generation", "retry-clean-fingerprint")
                    .unwrap(),
                losses: Vec::new(),
            })
            .collect(),
    };
    let observed_at_us = chrono::Utc::now().timestamp_micros();
    assert!(observed_at_us > context.failure().occurred_at_us);
    assert_ne!(command.request_id(), initial.request_id());
    (db, CapturedLossAssessment {
        command,
        context,
        inspection,
        observed_at_us,
        evidence: "fresh finite captured worktrees clean; processes and durable members quiescent".into(),
    })
}

fn captured_successor(context: &NativeCloseCapturedLossRetryContext) -> CloseRunRef {
    CloseRunRef {
        attempt_id: context.failed_run().attempt_id.clone(),
        ordinal: CloseRunOrdinal::parse(2).unwrap(),
    }
}

async fn assert_captured_origin_preserved(db: &Database, origin: &NativeCloseCapturedLossRetryContext) {
    let attempt = origin.failed_run().attempt_id.as_str();
    assert_eq!(db.get_close_run(origin.failed_run()).await.unwrap().status, CloseRunStatus::Stopped);
    assert_eq!(db.list_close_attempt_scopes(attempt).await.unwrap(), origin.scopes());
    assert_eq!(db.list_close_retirement_inspections(attempt).await.unwrap(), origin.inspections());
    assert_eq!(db.list_close_retirement_losses(attempt).await.unwrap(), origin.losses());
    let failures = db.list_close_cleanup_failures(attempt).await.unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].run_ordinal, CloseRunOrdinal::INITIAL);
    assert_eq!(&failures[0].occurrence, origin.failure());
    assert!(matches!(
        db.read_stopped_close_original_retirement_proof(origin.failed_run()).await,
        Err(DbError::CloseFoundationPrecondition(message))
            if message == "original retirement provenance is not anchored"
    ));
    // An effect-free loss stops before sealing inventory; the inventory reader
    // requires a seal even when there are no expected-resource rows.
    let original_rows: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM close_retirement_inventories WHERE attempt_id = ?1),
                (SELECT COUNT(*) FROM close_expected_retirement_resources WHERE attempt_id = ?1)",
    )
    .bind(attempt)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(original_rows, (0, 0));
    assert!(db.list_close_retirement_evidence(attempt).await.unwrap().is_empty());
}

fn captured_admission_refused(
    result: LocalAuthorityResult<DbResult<NativeCloseRetryResult>>,
    expected: &str,
) {
    assert!(matches!(result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CloseFoundationPrecondition(message)))
            if message.contains(expected)
    ));
}

#[tokio::test]
async fn captured_loss_origin_reader_does_not_require_a_sealed_inventory() {
    let (db, assessment) = captured_loss_assessment().await;
    let failed_run = assessment.context.failed_run();
    let target = assessment.command.intent().target();
    for _ in 0..2 {
        assert_eq!(
            db.read_native_close_retry_origin_kind(target, failed_run)
                .await
                .unwrap(),
            crate::native_close::NativeCloseRetryOriginKind::CapturedLoss,
        );
    }
    assert!(db
        .read_native_close_retry_origin_kind(&ProductConversationId::parse("wrong-target").unwrap(), failed_run)
        .await
        .is_err());
    let wrong_run = CloseRunRef {
        attempt_id: failed_run.attempt_id.clone(),
        ordinal: CloseRunOrdinal::parse(2).unwrap(),
    };
    assert!(db
        .read_native_close_retry_origin_kind(target, &wrong_run)
        .await
        .is_err());
    assert_captured_origin_preserved(&db, &assessment.context).await;
}

#[tokio::test]
async fn captured_loss_admits_distinct_successor_without_rewriting_original_evidence() {
    let (db, assessment) = captured_loss_assessment().await;
    let origin = &assessment.context;
    assert_captured_origin_preserved(&db, origin).await;
    let members = db.list_close_attempt_members(origin.failed_run().attempt_id.as_str()).await.unwrap();
    let NativeCloseRetryResult::NewlyAdmitted(receipt) =
        established(db.apply_native_close_captured_loss_retry(&assessment).await)
    else {
        panic!("expected a newly admitted captured-loss successor");
    };
    let successor = captured_successor(origin);
    assert_eq!(receipt.request_id, assessment.command.request_id());
    assert_eq!(receipt.admitted.target, *assessment.command.intent().target());
    assert_eq!(receipt.admitted.attempt_id, successor.attempt_id);
    assert_eq!(receipt.admitted.run_ordinal, successor.ordinal);
    assert_eq!(db.get_close_run(&successor).await.unwrap().status, CloseRunStatus::Running);
    assert_eq!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap(), Some(receipt.admitted.clone()));
    assert_eq!(db.list_close_attempt_members(origin.failed_run().attempt_id.as_str()).await.unwrap(), members);
    assert_eq!(
        established(db.apply_native_close_captured_loss_retry(&assessment).await),
        NativeCloseRetryResult::Replay(receipt),
    );
    let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1")
        .bind(origin.failed_run().attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(run_count, 2);
    assert_captured_origin_preserved(&db, origin).await;
}

#[tokio::test]
async fn captured_loss_admits_legacy_flat_origin_after_initial_acknowledgement() {
    let (db, assessment) = captured_loss_assessment_with_wire_shape(true).await;
    assert_eq!(
        assessment.command.source_input(),
        assessment.command.intent().legacy_source_input()
    );
    let successor = captured_successor(&assessment.context);
    let NativeCloseRetryResult::NewlyAdmitted(receipt) =
        established(db.apply_native_close_captured_loss_retry(&assessment).await)
    else {
        panic!("expected a newly admitted captured-loss successor");
    };
    assert_eq!(receipt.admitted.run_ordinal, successor.ordinal);
    assert_eq!(
        db.get_close_run(&successor).await.unwrap().status,
        CloseRunStatus::Running
    );
    assert_eq!(
        established(db.apply_native_close_captured_loss_retry(&assessment).await),
        NativeCloseRetryResult::Replay(receipt)
    );
}

async fn accept_captured_loss_failure(db: &Database, failure_occurrence_id: &str, receipt: &str) {
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    db.append_steering_entry(
        &coordinator.id,
        &phoenix_core::domain::sm_event::SteerEntry {
            text: "Close failure".into(),
            origin: phoenix_core::domain::db_schema::InputOrigin::SubscriptionEvent {
                event_id: failure_occurrence_id.to_owned(),
            },
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: format!("accepted-captured-loss-{receipt}"),
            user_agent: None,
            skill_invocation: None,
        },
        &format!("captured-loss-failure-receipt-{receipt}"),
    )
    .await
    .unwrap();
}

#[allow(
    clippy::too_many_lines,
    reason = "One aggregate must survive refused Delete until failure custody is accepted, then cascade on successful Delete"
)]
#[tokio::test]
async fn captured_loss_admitted_authority_cascades_on_permanent_member_delete() {
    let (db, assessment) = captured_loss_assessment().await;
    established(db.apply_native_close_captured_loss_retry(&assessment).await);
    let attempt = assessment.context.failed_run().attempt_id.as_str();
    let members = db.list_close_attempt_members(attempt).await.unwrap();
    assert!(!members.is_empty());

    // Stop through the owning source writer before permanent aggregate removal.
    db.update_conversation_state(
        assessment.command.source_transcript_id(),
        &ConvState::Idle,
    )
    .await
    .unwrap();
    for table in [
        "native_close_completion_retry_origins",
        "native_close_completion_retry_source_revocations",
        "native_close_initial_result_acknowledgements",
    ] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1"
        )))
        .bind(attempt)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 1, "fixture must retain {table}");
    }
    for table in [
        "native_close_completion_retry_origins",
        "native_close_completion_retry_source_revocations",
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE attempt_id = ?1"
        )))
        .bind(attempt)
        .execute(db.pool())
        .await
        .unwrap_err();
        assert!(error.to_string().contains("retained"), "{error}");
    }
    let automatic_admissions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM native_close_completion_retry_admissions WHERE attempt_id = ?1",
    )
    .bind(attempt)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(automatic_admissions, 0);
    // The native tool-result acknowledgement does not accept mandatory failure
    // custody. Permanent Delete must refuse without partially removing the member.
    let target_member = &members[0];
    let members_before = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM conversations WHERE product_conversation_id = ?1",
    )
    .bind(assessment.command.intent().target().as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let error = db
        .delete_conversation(target_member.conversation_id.as_str())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("FOREIGN KEY constraint failed"),
        "{error}"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM conversations WHERE product_conversation_id = ?1",
        )
        .bind(assessment.command.intent().target().as_str())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        members_before,
        "failed permanent Delete must roll back the member aggregate"
    );
    assert!(db
        .pending_coordinator_watch_events(16)
        .await
        .unwrap()
        .iter()
        .any(|event| event.source_occurrence_id == assessment.context.failure().failure_occurrence_id));

    let original_failure = &assessment.context.failure().failure_occurrence_id;
    accept_captured_loss_failure(&db, original_failure, "original").await;

    // The owning source write can establish another mandatory failure. Settle
    // every remaining fixture failure through the same canonical receipt API.
    let pending = db.pending_coordinator_watch_events(64).await.unwrap();
    for failure in db.list_close_cleanup_failures(attempt).await.unwrap() {
        let occurrence_id = failure.occurrence.failure_occurrence_id.as_str();
        if pending
            .iter()
            .any(|event| event.source_occurrence_id == occurrence_id)
        {
            accept_captured_loss_failure(&db, occurrence_id, occurrence_id).await;
        }
    }
    assert!(db
        .pending_coordinator_watch_events(64)
        .await
        .unwrap()
        .is_empty());

    for failure in db.list_close_cleanup_failures(attempt).await.unwrap() {
        let state: String = sqlx::query_scalar(
            "SELECT delivery_state FROM coordinator_watch_events WHERE event_id = ?1",
        )
        .bind(failure.occurrence.failure_occurrence_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(state, "accepted");
    }

    for member in members {
        db.delete_conversation(member.conversation_id.as_str())
            .await
            .unwrap();
    }
    for table in [
        "native_close_captured_loss_retry_plans",
        "native_close_captured_loss_retry_inspections",
        "native_close_captured_loss_retry_resources",
        "native_close_completion_retry_origins",
        "native_close_completion_retry_source_revocations",
        "native_close_completion_retry_admissions",
        "native_close_initial_result_acknowledgements",
        "native_close_initial_receipts",
        "native_close_retry_plans",
        "close_runs",
        "close_obligations",
    ] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE attempt_id = ?1"
        )))
        .bind(attempt)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0, "permanent Delete retained {table}");
    }
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert!(violations.is_empty());
}

#[tokio::test]
async fn captured_loss_exact_replay_preserves_successor_plan_and_all_visible_facts() {
    let (db, assessment) = captured_loss_assessment().await;
    let NativeCloseRetryResult::NewlyAdmitted(receipt) =
        established(db.apply_native_close_captured_loss_retry(&assessment).await)
    else {
        panic!("expected admission before replay");
    };
    let successor = captured_successor(&assessment.context);
    let projection = db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap();
    let execution = db.read_native_close_retry_execution_context(&assessment.command, &receipt.admitted).await.unwrap();
    let effects = db.list_close_safe_retry_effects(&successor).await.unwrap();
    assert!(!effects.is_empty());
    assert_eq!(
        established(db.apply_native_close_captured_loss_retry(&assessment).await),
        NativeCloseRetryResult::Replay(receipt.clone()),
    );
    assert_eq!(db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap(), projection);
    assert_eq!(db.read_native_close_retry_execution_context(&assessment.command, &receipt.admitted).await.unwrap(), execution);
    assert_eq!(db.list_close_safe_retry_effects(&successor).await.unwrap(), effects);
    assert_eq!(db.get_close_run(&successor).await.unwrap().status, CloseRunStatus::Running);
    assert!(db.get_close_run(&CloseRunRef {
        attempt_id: successor.attempt_id,
        ordinal: CloseRunOrdinal::parse(3).unwrap(),
    }).await.is_err());
    assert_captured_origin_preserved(&db, &assessment.context).await;
}

#[tokio::test]
async fn captured_loss_refuses_wrong_scope_dirty_and_stale_assessments_atomically() {
    for defect in ["scope", "dirty", "generation", "observation", "evidence"] {
        let (db, mut assessment) = captured_loss_assessment().await;
        let projection = db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap();
        let expected = match defect {
            "scope" => {
                assessment.inspection.scopes[0].scope = WorkScopeId::parse("not-captured").unwrap();
                "exact ordered full fresh clean inspection"
            }
            "dirty" => {
                assessment.inspection.scopes[0].losses.push(CloseLossItem::UntrackedNonIgnoredPath(
                    GitPathIdentity::from_bytes(b"still-dirty".to_vec()),
                ));
                "exact ordered full fresh clean inspection"
            }
            "generation" => {
                assessment.inspection.scopes[0].snapshot = CloseRetirementSnapshot::parse(
                    assessment.context.inspections()[0].snapshot.generation(),
                    "changed-fingerprint-with-stale-generation",
                ).unwrap();
                "exact ordered full fresh clean inspection"
            }
            "observation" => {
                assessment.observed_at_us = assessment.context.failure().occurred_at_us;
                "fresh safety evidence after the stopped run"
            }
            "evidence" => {
                assessment.evidence = " \t".into();
                "fresh safety evidence after the stopped run"
            }
            _ => unreachable!(),
        };
        captured_admission_refused(db.apply_native_close_captured_loss_retry(&assessment).await, expected);
        assert_eq!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap(), None);
        assert!(db.get_close_run(&captured_successor(&assessment.context)).await.is_err());
        assert_eq!(db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap(), projection);
        assert_eq!(db.read_native_close_captured_loss_retry_context(
            assessment.command.intent().target(), assessment.context.failed_run(),
        ).await.unwrap(), assessment.context);
        assert_captured_origin_preserved(&db, &assessment.context).await;
    }
}

#[tokio::test]
async fn captured_loss_source_stop_refuses_before_admission() {
    let (db, assessment) = captured_loss_assessment().await;
    db.update_conversation_state(assessment.command.source_transcript_id(), &ConvState::Idle)
        .await
        .unwrap();
    let projection = db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap();
    captured_admission_refused(
        db.apply_native_close_captured_loss_retry(&assessment).await,
        "Close requires its current executing source",
    );
    assert!(db.get_close_run(&captured_successor(&assessment.context)).await.is_err());
    assert_eq!(db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap(), projection);
    assert_captured_origin_preserved(&db, &assessment.context).await;
}

#[tokio::test]
async fn captured_loss_new_member_creation_work_refuses_without_adopting_source() {
    let (db, assessment) = captured_loss_assessment().await;
    let member = db.list_close_attempt_members(assessment.context.failed_run().attempt_id.as_str())
        .await.unwrap().remove(0);
    // A real accepted creation job on an immutable captured member exercises the
    // durable-work gate without forging a workflow, terminal fact, or retry plan.
    let job = db.insert_conversation_creation_job(&crate::InsertConversationCreationJob {
        id: "captured-member-new-work".into(),
        conversation_id: member.conversation_id.as_str().to_owned(),
        message_id: None,
        intent: phoenix_core::domain::db_schema::ConversationCreationIntent {
            cwd: "/tmp".into(),
            model: None,
            effort: None,
            text: "new captured member work".into(),
            expansion_preflighted: false,
            llm_text: None,
            skill_invocation: None,
            message_id: "captured-member-work-message".into(),
            images: Vec::new(),
            files: Vec::new(),
            mode: None,
            base_branch: None,
            checkout_ref: None,
            seed_parent_id: None,
            seed_label: None,
            approved_task: None,
        },
    }).await.unwrap();
    let projection = db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap();
    captured_admission_refused(
        db.apply_native_close_captured_loss_retry(&assessment).await,
        "active durable captured member obligation",
    );
    assert_eq!(db.get_conversation_creation_job(&job.id).await.unwrap(), job);
    assert_eq!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap(), None);
    assert!(db.get_close_run(&captured_successor(&assessment.context)).await.is_err());
    assert_eq!(db.get_active_close_projection_for_product(assessment.command.intent().target()).await.unwrap(), projection);
    assert_captured_origin_preserved(&db, &assessment.context).await;
}

#[tokio::test]
async fn captured_loss_runtime_context_uses_accepted_clean_scope_snapshot_and_finite_effects() {
    let (db, assessment) = captured_loss_assessment().await;
    let NativeCloseRetryResult::NewlyAdmitted(receipt) =
        established(db.apply_native_close_captured_loss_retry(&assessment).await)
    else {
        panic!("expected captured-loss successor");
    };
    let execution = db.read_native_close_retry_execution_context(&assessment.command, &receipt.admitted)
        .await.unwrap();
    let expected_snapshots: Vec<_> = assessment.inspection.scopes.iter().map(|scope| (
        CloseRetirementTarget { scope: scope.scope.clone() }, scope.snapshot.clone(),
    )).collect();
    assert_eq!(execution.worktree_snapshots, expected_snapshots);
    assert_ne!(execution.snapshot, *assessment.context.snapshot());
    assert_eq!(execution.scopes, assessment.context.scopes());
    let mut expected_effects = Vec::new();
    for captured in assessment.context.scopes() {
        if let Some(worktree) = &captured.captured_worktree {
            let CapturedWorktreeIdentity::Resolved(identity) = worktree else {
                panic!("fixture must capture a resolved worktree");
            };
            expected_effects.push(crate::CloseSafeRetryEffect {
                scope: captured.scope.clone(),
                resource: RetiredResourceIdentity::parse(
                    RetiredResourceKind::Worktree, LossItemIdentity::Worktree(identity.clone()),
                ).unwrap(),
            });
        }
        expected_effects.push(crate::CloseSafeRetryEffect {
            scope: captured.scope.clone(),
            resource: scope_resource(&captured.scope).unwrap(),
        });
    }
    assert_eq!(execution.effects, expected_effects);
    assert_eq!(execution.effects.len(), 2);
    assert_eq!(execution.progress.pending, expected_effects);
    assert!(execution.progress.successful.is_empty());
    assert_eq!(execution.resources.len(), expected_effects.len());
    for (resource, effect) in execution.resources.iter().zip(&expected_effects) {
        assert_eq!(resource.attempt_id, receipt.admitted.attempt_id);
        assert_eq!(resource.scope, effect.scope);
        assert_eq!(resource.resource, effect.resource);
        assert_eq!(resource.snapshot, execution.snapshot);
    }
    assert_eq!(db.list_close_safe_retry_effects(&captured_successor(&assessment.context)).await.unwrap(), expected_effects);
    assert_captured_origin_preserved(&db, &assessment.context).await;
}
