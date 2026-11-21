use std::collections::BTreeSet;

async fn source_fence_fixture() -> (Database, Assessment, CloseRunRef) {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "source-fence").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let run = CloseRunRef {
        attempt_id: failure.attempt_id,
        ordinal: receipt.admitted.run_ordinal,
    };
    (db, assessment, run)
}

async fn record_source_bound_success(
    db: &Database,
    assessment: &Assessment,
    run: &CloseRunRef,
    effect: &CloseSafeRetryEffect,
    detail: &str,
) -> LocalAuthorityResult<DbResult<()>> {
    let admitted = crate::native_close::NativeCloseRunRef {
        target: assessment.command.intent().target().clone(),
        attempt_id: run.attempt_id.clone(),
        run_ordinal: run.ordinal,
    };
    db.record_native_close_retry_success(&assessment.command, &admitted, effect, detail).await
}

async fn complete_source_bound_retry(
    db: &Database,
    assessment: &Assessment,
    run: &CloseRunRef,
) -> LocalAuthorityResult<DbResult<CloseObligation>> {
    let admitted = crate::native_close::NativeCloseRunRef {
        target: assessment.command.intent().target().clone(),
        attempt_id: run.attempt_id.clone(),
        run_ordinal: run.ordinal,
    };
    db.complete_native_close_retry(&assessment.command, &admitted).await
}

async fn write_source_fence_state(
    db: &Database,
    command: &NativeCloseCommand,
    state: &ConvState,
    replay: bool,
) -> DbResult<()> {
    let source = command.source_transcript_id();
    if replay {
        db.update_state_and_replay(
            source,
            state,
            Utc::now(),
            &phoenix_core::domain::provider_replay::ProviderReplayUpdate::Clear,
        )
        .await
    } else {
        db.update_conversation_state(source, state).await
    }
}

async fn source_fence_allowance(db: &Database, run: &CloseRunRef) -> Option<(i64, i64)> {
    sqlx::query_as(
        "SELECT consumed, suppressed FROM close_completion_retry_allowances WHERE attempt_id = ?1",
    )
    .bind(run.attempt_id.as_str())
    .fetch_optional(db.pool())
    .await
    .unwrap()
}

async fn assert_source_fenced(
    db: &Database,
    assessment: &Assessment,
    run: &CloseRunRef,
    pending: &[CloseSafeRetryEffect],
) {
    assert_native_retry_stopped(db, assessment, run, pending,
        CloseCleanupFailureAuthority::ExplicitlySuppressed).await;
}

async fn assert_native_retry_stopped(
    db: &Database,
    assessment: &Assessment,
    run: &CloseRunRef,
    pending: &[CloseSafeRetryEffect],
    authority: CloseCleanupFailureAuthority,
) {
    assert_eq!(db.get_close_run(run).await.unwrap().status, CloseRunStatus::Stopped);
    completion_refused(complete_source_bound_retry(db, assessment, run).await);
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    let matching: Vec<_> = failures.iter().filter(|failure| failure.run_ordinal == run.ordinal).collect();
    assert_eq!(matching.len(), 1);
    let failure = &matching[0].occurrence;
    assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
    assert_eq!(failure.attempt_id, run.attempt_id);
    assert_eq!(&failure.source_product_conversation_id, assessment.command.intent().target());
    assert_eq!(failure.authority, authority);
    assert_eq!(failure.reason, RetirementFailureReason::Interrupted);
    assert_eq!(failure.stop_certainty, CloseStopCertainty::ShutdownUncertain);
    assert_eq!(failure.stop_certainty.confirmed_at_us(), None);
    let expected_resources: Vec<_> = pending.iter().map(|effect| CloseCleanupFailureResource {
        scope: effect.scope.clone(),
        resource: effect.resource.clone(),
        disposition: CloseCleanupResourceDisposition::Unattempted,
    }).collect();
    assert_eq!(failure.remaining_resources, expected_resources);
    assert!(failure.remaining_resources.iter().all(|resource| {
        resource.disposition != CloseCleanupResourceDisposition::Failed
    }));
    assert_eq!(
        db.close_cleanup_failure_timing(&failure.failure_occurrence_id).await.unwrap(),
        Some((failure.occurred_at_us, None)),
    );

    let events = db.pending_coordinator_watch_events(100).await.unwrap();
    let matching: Vec<_> = events.iter().filter(|event| {
        event.event_id == failure.failure_occurrence_id
    }).collect();
    assert_eq!(matching.len(), 1);
    let event = matching[0];
    assert_eq!(event.product_conversation_id, failure.source_product_conversation_id);
    assert_eq!(event.source_occurrence_kind, "close_cleanup_failure");
    assert_eq!(event.source_occurrence_id, failure.failure_occurrence_id);
    assert_eq!(event.occurred_at_us, failure.occurred_at_us);
    assert_eq!(event.outcome, crate::coordinator_watches::WatchOutcome::CleanupFailed {
        reason: "interrupted".into(),
    });
    let crate::coordinator_watches::WatchEventRoute::MandatoryCloseFailure {
        run_ordinal,
        remaining_resources,
        subject,
        detail,
        stop,
    } = &event.route else {
        panic!("expected mandatory Close failure route");
    };
    assert_eq!(*run_ordinal, run.ordinal);
    assert_eq!(*subject, crate::coordinator_watches::MandatoryCloseFailureSubject::Attempt);
    assert_eq!(detail, &failure.detail);
    assert!(matches!(stop, crate::coordinator_watches::CloseFailureStop::ShutdownUncertain));
    assert_eq!(remaining_resources.len(), expected_resources.len());
    for (actual, expected) in remaining_resources.iter().zip(&expected_resources) {
        let identity = expected.resource.identity();
        assert_eq!(actual.scope, expected.scope);
        assert_eq!(actual.resource_kind, expected.resource.kind().as_str());
        assert_eq!(actual.identity_kind, identity.identity_kind());
        assert_eq!(actual.identity_codec, identity.codec());
        assert_eq!(actual.identity_value, identity.value());
        assert_eq!(actual.disposition, CloseCleanupResourceDisposition::Unattempted);
    }
    assert_eq!(
        db.get_close_obligation(run.attempt_id.as_str()).await.unwrap().close_outcome(),
        Some(CloseCompletionOutcome::CloseIncomplete),
    );
}

#[tokio::test]
async fn ordinary_and_replay_stop_fence_native_retry_and_suppress_existing_allowance() {
    for replay in [false, true] {
        for consumed in [0_i64, 1_i64] {
            let (db, assessment, run) = source_fence_fixture().await;
            let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
            assert!(!pending.is_empty());
            assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
            if consumed == 1 {
                sqlx::query(
                    "UPDATE close_completion_retry_allowances SET consumed = 1 WHERE attempt_id = ?1",
                )
                .bind(run.attempt_id.as_str())
                .execute(db.pool())
                .await
                .unwrap();
            }
            let original = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
            write_source_fence_state(&db, &assessment.command, &ConvState::Idle, replay).await.unwrap();
            assert_eq!(
                db.get_conversation(assessment.command.source_transcript_id()).await.unwrap().state,
                ConvState::Idle,
            );
            assert_source_fenced(&db, &assessment, &run, &pending).await;
            assert_eq!(source_fence_allowance(&db, &run).await, Some((consumed, 1)));
            assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), original);

            let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
            write_source_fence_state(&db, &assessment.command, &ConvState::Idle, replay).await.unwrap();
            assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
            assert_source_fenced(&db, &assessment, &run, &pending).await;
            assert_eq!(source_fence_allowance(&db, &run).await, Some((consumed, 1)));
        }
    }
}

#[tokio::test]
async fn same_kind_source_replacement_checks_call_message_and_canonical_input() {
    for replay in [false, true] {
        for replacement in ["call", "message", "input"] {
            let (db, assessment, run) = source_fence_fixture().await;
            let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
            let mut state = db.get_conversation(assessment.command.source_transcript_id())
                .await.unwrap().state;
            write_source_fence_state(&db, &assessment.command, &state, replay).await.unwrap();
            assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
            assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().len(), 1);
            assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));

            let ConvState::ToolExecuting { assistant_message, current_tool, .. } = &mut state else {
                panic!("expected executing native source");
            };
            match replacement {
                "call" => {
                    current_tool.id = "replacement-call".into();
                    for block in &mut assistant_message.content {
                        if let ContentBlock::ToolUse { id, .. } = block {
                            *id = current_tool.id.clone();
                        }
                    }
                }
                "message" => assistant_message.message_id = "replacement-message".into(),
                "input" => {
                    let input = NativeCloseIntent::Observe {
                        target: assessment.command.intent().target().clone(),
                        attempt_id: run.attempt_id.clone(),
                        run_ordinal: run.ordinal,
                    }.source_input();
                    current_tool.input = ToolInput::Unknown {
                        name: "close_conversation".into(),
                        input: input.clone(),
                    };
                    for block in &mut assistant_message.content {
                        if let ContentBlock::ToolUse { input: canonical, .. } = block {
                            *canonical = input.clone();
                        }
                    }
                }
                _ => unreachable!(),
            }
            write_source_fence_state(&db, &assessment.command, &state, replay).await.unwrap();
            assert_eq!(
                db.get_conversation(assessment.command.source_transcript_id()).await.unwrap().state,
                state,
            );
            assert_source_fenced(&db, &assessment, &run, &pending).await;
            assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
        }
    }
}

#[tokio::test]
async fn source_fence_preserves_successes_and_reports_only_exact_pending_effects() {
    let (db, assessment, run) = source_fence_fixture().await;
    let before = db.close_safe_retry_progress(&run).await.unwrap();
    assert!(before.pending.len() >= 2);
    let succeeded = before.pending[0].clone();
    let detail = "exact planned resource retired before source stop";
    success_recorded(record_source_bound_success(&db, &assessment, &run, &succeeded, detail).await);
    let progress = db.close_safe_retry_progress(&run).await.unwrap();
    assert_eq!(progress.successful, vec![succeeded.clone()]);
    assert_eq!(progress.pending, before.pending[1..]);

    write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false).await.unwrap();
    assert_source_fenced(&db, &assessment, &run, &progress.pending).await;
    // Retained success does not authorize replay after the source and run stop.
    success_refused(record_source_bound_success(&db, &assessment, &run, &succeeded, detail).await);
    success_refused(record_source_bound_success(
        &db, &assessment, &run, &progress.pending[0], "must not retire an effect after source stop",
    ).await);
    completion_refused(complete_source_bound_retry(&db, &assessment, &run).await);
}

#[tokio::test]
async fn empty_pending_source_fence_does_not_invent_allowance_or_failed_resource() {
    for replay in [false, true] {
        let db = Database::open_in_memory().await.unwrap();
        // Model an obligation predating allowance seeding without deleting a protected row.
        sqlx::query("DROP TRIGGER close_obligations_create_completion_retry_allowance")
            .execute(db.pool()).await.unwrap();
        let failure = fixture(&db, Vec::new(), true).await;
        let assessment = assess(&db, &failure, "source-fence-no-allowance").await;
        let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
        let run = CloseRunRef {
            attempt_id: failure.attempt_id,
            ordinal: receipt.admitted.run_ordinal,
        };
        assert_eq!(source_fence_allowance(&db, &run).await, None);
        let progress = db.close_safe_retry_progress(&run).await.unwrap();
        assert!(!progress.pending.is_empty());
        for effect in &progress.pending {
            success_recorded(record_source_bound_success(
                &db, &assessment, &run, effect, "retired before source stop",
            ).await);
        }
        assert!(db.close_safe_retry_progress(&run).await.unwrap().pending.is_empty());
        write_source_fence_state(&db, &assessment.command, &ConvState::Idle, replay).await.unwrap();
        assert_source_fenced(&db, &assessment, &run, &[]).await;
        assert_eq!(source_fence_allowance(&db, &run).await, None);
        completion_refused(complete_source_bound_retry(&db, &assessment, &run).await);
    }
}

#[tokio::test]
async fn finalized_native_retry_only_suppresses_allowance_on_source_loss() {
    for replay in [false, true] {
        let (db, assessment, run) = source_fence_fixture().await;
        let progress = db.close_safe_retry_progress(&run).await.unwrap();
        for effect in &progress.pending {
            success_recorded(record_source_bound_success(
                &db, &assessment, &run, effect, "retired before finalization",
            ).await);
        }
        let completed = retry_completed(complete_source_bound_retry(&db, &assessment, &run).await);
        assert_eq!(completed.close_outcome(), Some(CloseCompletionOutcome::Archived));
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Completed);
        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        let events = db.pending_coordinator_watch_events(100).await.unwrap();
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));

        write_source_fence_state(&db, &assessment.command, &ConvState::Idle, replay).await.unwrap();
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Completed);
        assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), completed);
        assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
        let after = db.pending_coordinator_watch_events(100).await.unwrap();
        assert_eq!(after.len(), events.len());
        assert!(!after.iter().any(|event| event.event_id == run.failure_occurrence_id()));
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
    }
}

async fn assert_initial_interrupted(db: &Database, run: &CloseRunRef) {
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    let failure = &failures[0].occurrence;
    assert_eq!(failure.authority, CloseCleanupFailureAuthority::AttemptInterrupted);
    assert_eq!(failure.reason, RetirementFailureReason::Interrupted);
    assert_eq!(failure.stop_certainty, CloseStopCertainty::ShutdownUncertain);
    assert!(failure.remaining_resources.is_empty());
}

#[tokio::test]
async fn initial_native_source_loss_suppresses_allowance_without_a_successor() {
    for replay in [false, true] {
        let db = Database::open_in_memory().await.unwrap();
        create_root(&db, "root").await;
        let command = retry_command(&db, "initial-source-fence", NativeCloseIntent::Initial {
            target: product_id("root"),
        }).await;
        let LocalAuthorityResult::DurableFactEstablished(Ok(
            crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
        )) = db.apply_native_close_initial(&command).await else {
            panic!("expected initial native admission");
        };
        let run = CloseRunRef::initial(receipt.admitted.attempt_id);
        write_source_fence_state(&db, &command, &ConvState::Idle, replay).await.unwrap();
        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        assert_eq!(failures.len(), 1);
        let failure = &failures[0].occurrence;
        assert_eq!(failure.authority, CloseCleanupFailureAuthority::ExplicitlySuppressed);
        assert_eq!(failure.reason, RetirementFailureReason::Interrupted);
        assert_eq!(failure.stop_certainty, CloseStopCertainty::ShutdownUncertain);
        assert!(failure.remaining_resources.is_empty());
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
        assert_stopped_source_loss_only_suppresses(&db, &command, &run, replay, 1).await;
    }
}

#[tokio::test]
async fn initial_source_fence_rolls_back_and_then_terminalizes_atomically() {
    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "root").await;
    let command = retry_command(&db, "initial-source-fence-rollback", NativeCloseIntent::Initial {
        target: product_id("root"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else {
        panic!("expected initial native admission");
    };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id);
    let retained_source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    sqlx::raw_sql("CREATE TRIGGER reject_initial_suppression BEFORE UPDATE OF suppressed ON close_completion_retry_allowances WHEN NEW.suppressed = 1 BEGIN SELECT RAISE(ABORT, 'injected rollback'); END;")
        .execute(db.pool()).await.unwrap();

    assert!(write_source_fence_state(&db, &command, &ConvState::Idle, false).await.is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, retained_source);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));

    sqlx::query("DROP TRIGGER reject_initial_suppression").execute(db.pool()).await.unwrap();
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].occurrence.authority, CloseCleanupFailureAuthority::ExplicitlySuppressed);
    assert!(failures[0].occurrence.remaining_resources.is_empty());
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
}

#[tokio::test]
async fn stopped_explicit_native_source_loss_suppresses_without_a_running_successor() {
    for replay in [false, true] {
        let (db, assessment, run) = source_fence_fixture().await;
        let progress = db.close_safe_retry_progress(&run).await.unwrap();
        for effect in &progress.pending {
            success_recorded(record_source_bound_success(
                &db, &assessment, &run, effect, "retired before interruption",
            ).await);
        }
        db.classify_interrupted_close_run(&run).await.unwrap();
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
        assert_stopped_source_loss_only_suppresses(&db, &assessment.command, &run, replay, 2).await;
    }
}

async fn assert_stopped_source_loss_only_suppresses(
    db: &Database,
    command: &NativeCloseCommand,
    run: &CloseRunRef,
    replay: bool,
    expected_runs: i64,
) {
    let stopped = db.get_close_run(run).await.unwrap();
    assert_eq!(stopped.status, CloseRunStatus::Stopped);
    let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    let events = db.pending_coordinator_watch_events(100).await.unwrap();
    let event_ids: Vec<_> = events.iter().map(|event| event.event_id.clone()).collect();
    for _ in 0..2 {
        write_source_fence_state(db, command, &ConvState::Idle, replay).await.unwrap();
        assert_eq!(source_fence_allowance(db, run).await, Some((0, 1)));
        assert_eq!(db.get_close_run(run).await.unwrap(), stopped);
        assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation);
        assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
        let after = db.pending_coordinator_watch_events(100).await.unwrap();
        assert_eq!(after.iter().map(|event| event.event_id.clone()).collect::<Vec<_>>(), event_ids);
        let runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1",
        ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
        assert_eq!(runs, expected_runs);
    }
}

#[tokio::test]
async fn observe_only_receipt_does_not_authorize_allowance_suppression() {
    for replay in [false, true] {
        let db = Database::open_in_memory().await.unwrap();
        let failure = fixture(&db, Vec::new(), true).await;
        let run = CloseRunRef::initial(failure.attempt_id.clone());
        let command = retry_command(&db, "observe-source-fence", NativeCloseIntent::Observe {
            target: failure.source_product_conversation_id.clone(),
            attempt_id: run.attempt_id.clone(),
            run_ordinal: run.ordinal,
        }).await;
        assert!(matches!(db.apply_native_close_observe(&command).await,
            LocalAuthorityResult::DurableFactEstablished(Ok(_))));
        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        let events = db.pending_coordinator_watch_events(100).await.unwrap();
        write_source_fence_state(&db, &command, &ConvState::Idle, replay).await.unwrap();
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
        assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
        assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), events.len());
    }
}

#[tokio::test]
async fn rejected_source_state_write_rolls_back_native_fence() {
    for replay in [false, true] {
        let (db, assessment, run) = source_fence_fixture().await;
        let before = db.get_conversation(assessment.command.source_transcript_id()).await.unwrap().state;
        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        sqlx::raw_sql(
            "CREATE TRIGGER test_reject_native_source_write BEFORE UPDATE OF state ON conversations
             WHEN OLD.runtime_role = 'coordinator'
             BEGIN SELECT RAISE(ABORT, 'state rejected'); END;",
        )
        .execute(db.pool())
        .await
        .unwrap();
        assert!(write_source_fence_state(&db, &assessment.command, &ConvState::Idle, replay).await.is_err());
        assert_eq!(
            db.get_conversation(assessment.command.source_transcript_id()).await.unwrap().state,
            before,
        );
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
        assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
        assert!(!db.pending_coordinator_watch_events(100).await.unwrap().iter()
            .any(|event| event.event_id == run.failure_occurrence_id()));
    }
}

async fn set_startup_allowance_bits(db: &Database, run: &CloseRunRef, consumed: i64, suppressed: i64) {
    sqlx::query(
        "UPDATE close_completion_retry_allowances SET consumed = ?2, suppressed = ?3 WHERE attempt_id = ?1",
    )
    .bind(run.attempt_id.as_str()).bind(consumed).bind(suppressed)
    .execute(db.pool()).await.unwrap();
}

#[tokio::test]
async fn startup_and_observer_interrupt_native_retry_from_original_pending_plan_preserving_allowance() {
    for observer_only in [false, true] {
        for consumed in [0_i64, 1_i64] {
            for suppressed in [0_i64, 1_i64] {
                let (db, assessment, run) = source_fence_fixture().await;
                let before = db.close_safe_retry_progress(&run).await.unwrap();
                assert!(before.pending.len() >= 2);
                success_recorded(record_source_bound_success(
                    &db, &assessment, &run, &before.pending[0], "retired before interruption",
                ).await);
                let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
                set_startup_allowance_bits(&db, &run, consumed, suppressed).await;
                let original = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
                if observer_only {
                    db.classify_interrupted_close_run(&run).await.unwrap();
                } else {
                    db.reset_all_to_idle().await.unwrap();
                    assert_eq!(db.get_conversation(assessment.command.source_transcript_id())
                        .await.unwrap().state, ConvState::Idle);
                }
                assert_native_retry_stopped(&db, &assessment, &run, &pending,
                    CloseCleanupFailureAuthority::AttemptInterrupted).await;
                assert_eq!(source_fence_allowance(&db, &run).await, Some((consumed, suppressed)));
                assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), original);
                success_refused(record_source_bound_success(
                    &db, &assessment, &run, &before.pending[0], "retired before interruption",
                ).await);
                let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
                db.classify_interrupted_close_run(&run).await.unwrap();
                db.reset_all_to_idle().await.unwrap();
                assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
                assert_native_retry_stopped(&db, &assessment, &run, &pending,
                    CloseCleanupFailureAuthority::AttemptInterrupted).await;
                assert_eq!(source_fence_allowance(&db, &run).await, Some((consumed, suppressed)));
            }
        }
    }
}

#[tokio::test]
async fn startup_preserves_initial_and_stopped_native_receipts_and_both_allowance_bits() {
    for stopped in [false, true] {
        for consumed in [0_i64, 1_i64] {
            for suppressed in [0_i64, 1_i64] {
                let db = Database::open_in_memory().await.unwrap();
                create_root(&db, "root").await;
                let command = retry_command(&db, "startup-initial", NativeCloseIntent::Initial {
                    target: product_id("root"),
                }).await;
                let LocalAuthorityResult::DurableFactEstablished(Ok(
                    crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
                )) = db.apply_native_close_initial(&command).await else {
                    panic!("expected initial native admission");
                };
                let run = CloseRunRef::initial(receipt.admitted.attempt_id);
                if stopped {
                    db.reset_all_to_idle().await.unwrap();
                    assert_initial_interrupted(&db, &run).await;
                }
                set_startup_allowance_bits(&db, &run, consumed, suppressed).await;
                if !stopped {
                    db.reset_all_to_idle().await.unwrap();
                    assert_initial_interrupted(&db, &run).await;
                }
                let retained_run = db.get_close_run(&run).await.unwrap();
                let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
                let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
                let events = db.pending_coordinator_watch_events(100).await.unwrap();
                for _ in 0..2 {
                    db.reset_all_to_idle().await.unwrap();
                    assert_eq!(source_fence_allowance(&db, &run).await, Some((consumed, suppressed)));
                    assert_eq!(db.get_close_run(&run).await.unwrap(), retained_run);
                    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation);
                    assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
                    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), events.len());
                }
            }
        }
    }
}

#[tokio::test]
async fn startup_leaves_completed_native_retry_and_allowance_untouched() {
    for consumed in [0_i64, 1_i64] {
        for suppressed in [0_i64, 1_i64] {
            let (db, assessment, run) = source_fence_fixture().await;
            for effect in db.close_safe_retry_progress(&run).await.unwrap().pending {
                success_recorded(record_source_bound_success(
                    &db, &assessment, &run, &effect, "retired before completion",
                ).await);
            }
            let completed = retry_completed(complete_source_bound_retry(&db, &assessment, &run).await);
            set_startup_allowance_bits(&db, &run, consumed, suppressed).await;
            let retained_run = db.get_close_run(&run).await.unwrap();
            let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
            let events = db.pending_coordinator_watch_events(100).await.unwrap();
            db.reset_all_to_idle().await.unwrap();
            assert_eq!(db.get_conversation(assessment.command.source_transcript_id())
                .await.unwrap().state, ConvState::Idle);
            assert_eq!(db.get_close_run(&run).await.unwrap(), retained_run);
            assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), completed);
            assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
            assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), events.len());
            assert_eq!(source_fence_allowance(&db, &run).await, Some((consumed, suppressed)));
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep each corrupted-plan case's full rollback proof together.
async fn source_fence_refuses_resource_plan_inspection_and_retry_artifact_near_misses_atomically() {
    for near_miss in ["resource_count", "inspection_binding", "retry_evidence"] {
        let (db, assessment, run) = source_fence_fixture().await;
        let source = db
            .get_conversation(assessment.command.source_transcript_id())
            .await
            .unwrap()
            .state;
        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        let events = db
            .pending_coordinator_watch_events(100)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.event_id)
            .collect::<Vec<_>>();
        let effects: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id = ?1 AND run_ordinal = ?2",
        )
        .bind(run.attempt_id.as_str())
        .bind(run.ordinal.get())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(effects > 0);

        match near_miss {
            "resource_count" => {
                sqlx::query("DROP TRIGGER native_close_retry_plans_immutable")
                    .execute(db.pool())
                    .await
                    .unwrap();
                sqlx::query(
                    "UPDATE native_close_retry_plans SET effect_count = effect_count + 1
                     WHERE attempt_id = ?1 AND run_ordinal = ?2",
                )
                .bind(run.attempt_id.as_str())
                .bind(run.ordinal.get())
                .execute(db.pool())
                .await
                .unwrap();
            }
            "inspection_binding" => {
                sqlx::query("DROP TRIGGER native_close_retry_plans_immutable")
                    .execute(db.pool())
                    .await
                    .unwrap();
                sqlx::query(
                    "UPDATE native_close_retry_plans
                     SET inspection_fingerprint = 'unbound-inspection'
                     WHERE attempt_id = ?1 AND run_ordinal = ?2",
                )
                .bind(run.attempt_id.as_str())
                .bind(run.ordinal.get())
                .execute(db.pool())
                .await
                .unwrap();
            }
            "retry_evidence" => {
                sqlx::query("DROP TRIGGER close_runs_terminal_and_identity_immutable")
                    .execute(db.pool())
                    .await
                    .unwrap();
                sqlx::query(
                    "UPDATE close_runs SET retry_evidence_kind = 'verified_completion'
                     WHERE attempt_id = ?1 AND run_ordinal = ?2",
                )
                .bind(run.attempt_id.as_str())
                .bind(run.ordinal.get())
                .execute(db.pool())
                .await
                .unwrap();
            }
            _ => unreachable!(),
        }

        assert!(write_source_fence_state(&db, &assessment.command, &ConvState::Idle, false)
            .await
            .is_err());
        assert_eq!(
            db.get_conversation(assessment.command.source_transcript_id()).await.unwrap().state,
            source,
        );
        assert_eq!(
            db.get_close_run(&run).await.unwrap().status,
            CloseRunStatus::Running
        );
        assert_eq!(
            db.list_close_cleanup_failures(run.attempt_id.as_str())
                .await
                .unwrap(),
            failures
        );
        assert_eq!(
            db.pending_coordinator_watch_events(100)
                .await
                .unwrap()
                .into_iter()
                .map(|event| event.event_id)
                .collect::<Vec<_>>(),
            events
        );
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
        let run_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1",
        )
        .bind(run.attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(run_count, 2);
    }
}

#[tokio::test]
async fn migration_166_populated_upgrade_preserves_retry_and_adds_initial_source_authority() {
    let (retry_db, assessment, receipt) = migration_163_pending_fixture().await;
    assert_eq!(
        crate::migrations::run_through_for_test(retry_db.pool(), 165)
            .await
            .unwrap(),
        3
    );
    let retry_run = CloseRunRef {
        attempt_id: receipt.admitted.attempt_id,
        ordinal: receipt.admitted.run_ordinal,
    };
    let retry_pending = retry_db
        .close_safe_retry_progress(&retry_run)
        .await
        .unwrap()
        .pending;
    assert!(!retry_pending.is_empty());

    let initial_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::migrations::setup_real_162_for_test(&initial_pool).await;
    let initial_db = Database::from_pool_for_tests(initial_pool, String::new());
    assert_eq!(
        crate::migrations::run_through_for_test(initial_db.pool(), 165)
            .await
            .unwrap(),
        3
    );
    create_root(&initial_db, "migration166-initial").await;
    let initial_command = retry_command(
        &initial_db,
        "migration166-initial",
        NativeCloseIntent::Initial {
            target: product_id("migration166-initial"),
        },
    )
    .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(initial_receipt),
    )) = initial_db
        .apply_native_close_initial(&initial_command)
        .await
    else {
        panic!("expected initial native admission before migration 166");
    };
    let initial_run = CloseRunRef::initial(initial_receipt.admitted.attempt_id);

    assert_eq!(
        crate::migrations::run_through_for_test(retry_db.pool(), 166)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        crate::migrations::run_through_for_test(initial_db.pool(), 166)
            .await
            .unwrap(),
        1
    );
    write_source_fence_state(&retry_db, &assessment.command, &ConvState::Idle, false)
        .await
        .unwrap();
    assert_source_fenced(&retry_db, &assessment, &retry_run, &retry_pending).await;
    assert_eq!(source_fence_allowance(&retry_db, &retry_run).await, Some((0, 1)));

    write_source_fence_state(&initial_db, &initial_command, &ConvState::Idle, false)
        .await
        .unwrap();
    let initial_failures = initial_db
        .list_close_cleanup_failures(initial_run.attempt_id.as_str())
        .await
        .unwrap();
    assert_eq!(initial_failures.len(), 1);
    assert_eq!(
        initial_failures[0].occurrence.authority,
        CloseCleanupFailureAuthority::ExplicitlySuppressed
    );
    assert!(initial_failures[0].occurrence.remaining_resources.is_empty());
    assert_eq!(source_fence_allowance(&initial_db, &initial_run).await, Some((0, 1)));
}

#[tokio::test]
async fn initial_source_fence_retains_exact_sealed_inventory_without_physical_effects() {
    use phoenix_core::domain::close::{CapturedWorktreeIdentity, CloseOwnedResourceInventory,
        CloseRetirementSnapshot, OpaqueIdentity};

    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "sealed-initial-source").await;
    let scope = allocate_scope_worktree(&db, "sealed-initial-source").await;
    let command = retry_command(&db, "sealed-initial-source", NativeCloseIntent::Initial {
        target: product_id("sealed-initial-source"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    let snapshot = CloseRetirementSnapshot::parse("sealed-generation", "sealed-fingerprint").unwrap();
    let inspection = crate::ReplaceCloseInspectionRequest {
        attempt_id: run.attempt_id.clone(),
        scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
            scope: scope.clone(), snapshot: snapshot.clone(), losses: vec![],
        }],
    };
    assert!(db.replace_native_close_initial_inspection(
        &command, &receipt, &source, inspection, None).await.established().unwrap().is_ok());
    let snapshot = CloseRetirementSnapshot::parse(
        encode_aggregate_snapshot_component([(&scope, snapshot.generation())]),
        encode_aggregate_snapshot_component([(&scope, snapshot.fingerprint())]),
    ).unwrap();
    let captured = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap();
    let Some(CapturedWorktreeIdentity::Resolved(worktree)) = captured[0].captured_worktree.clone()
        else { panic!("expected captured worktree"); };
    let tmux = OpaqueIdentity::parse("sealed-tmux").unwrap();
    let inventory = crate::CaptureCloseRetirementInventoryRequest {
        attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
        scopes: vec![crate::CaptureCloseRetirementInventoryScopeRequest {
            scope: scope.clone(),
            inventory: CloseOwnedResourceInventory {
                worktree: Some(worktree), work_scopes: BTreeSet::new(),
                bash_process_groups: BTreeSet::new(), tmux_servers: [tmux.clone()].into(),
                pty_sessions: BTreeSet::new(), browser_sessions: BTreeSet::new(),
                equivalent_live_resources: BTreeSet::new(),
            },
        }],
    };
    assert!(db.capture_native_close_initial_retirement_inventory(
        &command, &receipt, &source, inventory).await.established().unwrap().is_ok());
    let before = source_fence_allowance(&db, &run).await;
    let source_before = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    let obligation_before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    sqlx::raw_sql(
        "CREATE TRIGGER fail_initial_source_fence BEFORE INSERT ON close_cleanup_failures
         BEGIN SELECT RAISE(ABORT, 'injected source-fence failure'); END;",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(write_source_fence_state(&db, &command, &ConvState::Idle, false)
        .await
        .is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source_before);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation_before);
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert_eq!(source_fence_allowance(&db, &run).await, before);
    sqlx::query("DROP TRIGGER fail_initial_source_fence")
        .execute(db.pool())
        .await
        .unwrap();

    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(obligation.snapshot(), Some(&snapshot));
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    let failure = &failures[0].occurrence;
    assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
    assert!(matches!(&failure.authority, CloseCleanupFailureAuthority::ExpectedResource {
        scope: failed_scope, snapshot: retained_snapshot, resource,
    } if failed_scope == &scope && retained_snapshot == &snapshot
        && resource.kind() == RetiredResourceKind::TmuxServer));
    assert!(failure.remaining_resources.iter().any(|item| item.scope == scope
        && item.resource.kind() == RetiredResourceKind::WorkScope
        && item.disposition == CloseCleanupResourceDisposition::Unattempted));
    assert!(failure.remaining_resources.iter().any(|item| item.scope == scope
        && item.resource.kind() == RetiredResourceKind::Worktree
        && item.disposition == CloseCleanupResourceDisposition::Unattempted));
    assert!(failure.remaining_resources.iter().any(|item| item.scope == scope
        && item.resource.kind() == RetiredResourceKind::TmuxServer
        && item.resource.identity().value() == tmux.as_str()
        && item.disposition == CloseCleanupResourceDisposition::Failed));
    assert_eq!(source_fence_allowance(&db, &run).await, before.map(|(consumed, _)| (consumed, 1)));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), 1);
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn initial_source_fence_retains_no_worktree_scope_inventory_and_dispatch() {
    use phoenix_core::domain::close::{CloseOwnedResourceInventory, RetiredResourceKind};

    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "fenced-no-worktree").await;
    create_child(&db, "fenced-no-worktree-child", "fenced-no-worktree").await;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO work_scopes (
            id, authority_kind, lifecycle, environment_kind, cwd,
            worktree_path, branch_name, base_branch, created_at, updated_at
         ) VALUES
            ('fenced-unowned-scope', 'work', 'active', 'unowned_cwd', '/tmp', NULL, NULL, NULL, ?1, ?1),
            ('fenced-none-scope', 'work', 'active', 'none', NULL, NULL, NULL, NULL, ?1, ?1)",
    )
    .bind(&now)
    .execute(db.pool())
    .await
    .unwrap();
    for (conversation, scope) in [
        ("fenced-no-worktree", "fenced-unowned-scope"),
        ("fenced-no-worktree-child", "fenced-none-scope"),
    ] {
        sqlx::query("UPDATE conversations SET work_scope_id = ?1 WHERE id = ?2")
            .bind(scope)
            .bind(conversation)
            .execute(db.pool())
            .await
            .unwrap();
    }
    let command = retry_command(&db, "fenced-no-worktree-child", NativeCloseIntent::Initial {
        target: product_id("fenced-no-worktree"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    let scopes = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap();
    assert!(!scopes.is_empty());
    assert!(scopes.iter().all(|scope| scope.captured_worktree.is_none()));
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    db.replace_native_close_initial_inspection(
        &command,
        &receipt,
        &source,
        crate::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: vec![],
        },
        // Use the writer's schema-supported namespace, not an arbitrary generation.
        Some("server_git_status_v2_retry_source_fence_test"),
    )
    .await
    .established()
    .unwrap()
    .unwrap();
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    assert_eq!(snapshot.generation(), "server_git_status_v2_retry_source_fence_test");
    assert_eq!(snapshot.fingerprint(), "no-worktree");
    let LocalAuthorityResult::DurableFactEstablished(Ok(resources)) =
        db.capture_native_close_initial_retirement_inventory(&command, &receipt, &source,
            crate::CaptureCloseRetirementInventoryRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scopes: scopes.iter().map(|scope| crate::CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: None, work_scopes: BTreeSet::new(),
                        bash_process_groups: BTreeSet::new(), tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(), browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                }).collect(),
            }).await else { panic!("expected sealed no-worktree inventory"); };
    assert_eq!(resources.len(), scopes.len());
    assert!(resources.iter().all(|item| item.resource.kind() == RetiredResourceKind::WorkScope));
    let target = &resources[0];
    assert!(matches!(
        db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &source,
            RecordCloseRetirementDispatchRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scope: target.scope.clone(), resource: target.resource.clone(),
            },
        ).await,
        LocalAuthorityResult::DurableFactEstablished(Ok(()))
    ));
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    let after = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(after.snapshot(), Some(&snapshot));
    assert_eq!(after.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    assert!(matches!(&failures[0].occurrence.authority,
        CloseCleanupFailureAuthority::ExpectedResource { scope, snapshot: retained, resource }
            if scope == &target.scope && retained == &snapshot && resource == &target.resource));
    assert_eq!(failures[0].occurrence.remaining_resources.len(), resources.len());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), 1);
}

#[tokio::test]
async fn initial_source_or_startup_fence_after_nonempty_loss_inspection_retains_captured_scope() {
    use phoenix_core::domain::close::{CloseLossItem, CloseRetirementSnapshot, GitPathIdentity};

    for startup in [false, true] {
    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "fenced-loss").await;
    let scope = allocate_scope_worktree(&db, "fenced-loss").await;
    let command = retry_command(&db, "fenced-loss", NativeCloseIntent::Initial {
        target: product_id("fenced-loss"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.replace_native_close_initial_inspection(&command, &receipt, &source,
        crate::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(),
                snapshot: CloseRetirementSnapshot::parse("loss-gen", "loss-fp").unwrap(),
                losses: vec![CloseLossItem::UntrackedNonIgnoredPath(
                    GitPathIdentity::from_bytes(b"untracked.txt".to_vec()))],
            }],
        }, None).await.established().unwrap().is_ok());
    let before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(before.phase(), ClosePhase::AwaitingLossConfirmation);
    let snapshot = before.snapshot().cloned().unwrap();
    if startup {
        db.reset_all_to_idle().await.unwrap();
    } else {
        write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    }
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let after = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(after.snapshot(), Some(&snapshot));
    assert_eq!(after.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    assert!(matches!(&failures[0].occurrence.authority,
        CloseCleanupFailureAuthority::CapturedScope { scope: failed, .. } if failed == &scope));
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, i64::from(!startup))));
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn initial_source_or_startup_fence_retains_committed_repair_inventory() {
    use phoenix_core::domain::close::{CapturedWorktreeIdentity, CloseOwnedResourceInventory,
        CloseRetirementSnapshot, LossItemIdentity, RetiredResourceIdentity,
        RetirementFailureReason};

    for startup in [false, true] {
    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "fenced-repair").await;
    let scope = allocate_scope_worktree(&db, "fenced-repair").await;
    let command = retry_command(&db, "fenced-repair", NativeCloseIntent::Initial {
        target: product_id("fenced-repair"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    let scope_snapshot = CloseRetirementSnapshot::parse("repair-gen", "repair-fp").unwrap();
    assert!(db.replace_native_close_initial_inspection(&command, &receipt, &source,
        crate::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(), snapshot: scope_snapshot.clone(), losses: vec![],
            }],
        }, None).await.established().unwrap().is_ok());
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let captured = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap();
    let Some(CapturedWorktreeIdentity::Resolved(worktree)) = captured[0].captured_worktree.clone()
        else { panic!("expected captured worktree"); };
    let LocalAuthorityResult::DurableFactEstablished(Ok(resources)) =
        db.capture_native_close_initial_retirement_inventory(&command, &receipt, &source,
            crate::CaptureCloseRetirementInventoryRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scopes: vec![crate::CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: Some(worktree.clone()), work_scopes: BTreeSet::new(),
                        bash_process_groups: BTreeSet::new(), tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(), browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                }],
            }).await else { panic!("expected inventory"); };
    db.route_close_attempt_to_repair(crate::RouteCloseAttemptToRepairRequest {
        attempt_id: run.attempt_id.clone(), scope: scope.clone(),
        residual: RetiredResourceIdentity::parse(
            RetiredResourceKind::Worktree, LossItemIdentity::Worktree(worktree)).unwrap(),
        reason: RetirementFailureReason::IdentityNotProven,
        detail: "worktree identity requires repair".into(),
    }).await.unwrap();
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap().phase(),
        ClosePhase::NeedsRepair);
    let before = db.unresolved_expected_close_cleanup_resources(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(before.len(), resources.len());
    if startup {
        db.reset_all_to_idle().await.unwrap();
    } else {
        write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    }
    let after = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(after.snapshot(), Some(&snapshot));
    assert_eq!(after.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    assert!(matches!(&failures[0].occurrence.authority,
        CloseCleanupFailureAuthority::ExpectedResource { snapshot: retained, .. }
            if retained == &snapshot));
    assert_eq!(failures[0].occurrence.remaining_resources.len(), before.len());
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, i64::from(!startup))));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn initial_source_fence_preserves_proofs_across_mixed_worktree_scopes() {
    use phoenix_core::domain::close::{CapturedWorktreeIdentity, CloseOwnedResourceInventory,
        CloseRetirementSnapshot, OpaqueIdentity};

    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "sealed-two-scopes").await;
    let root_scope = allocate_scope_worktree(&db, "sealed-two-scopes").await;
    create_child(&db, "sealed-two-scopes-child", "sealed-two-scopes").await;
    let child_scope = db.get_conversation("sealed-two-scopes-child").await.unwrap()
        .attached_work_scope_id.unwrap();
    assert_ne!(root_scope, child_scope);
    let command = retry_command(&db, "sealed-two-scopes", NativeCloseIntent::Initial {
        target: product_id("sealed-two-scopes"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    let snapshot = CloseRetirementSnapshot::parse("two-scope-generation", "two-scope-fingerprint").unwrap();
    assert!(db.replace_native_close_initial_inspection(&command, &receipt, &source,
        crate::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
                scope: root_scope.clone(), snapshot: snapshot.clone(), losses: vec![],
            }],
        }, None).await.established().unwrap().is_ok());
    let snapshot = CloseRetirementSnapshot::parse(
        encode_aggregate_snapshot_component([(&root_scope, snapshot.generation())]),
        encode_aggregate_snapshot_component([(&root_scope, snapshot.fingerprint())]),
    ).unwrap();
    let captured = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap();
    let tmux = OpaqueIdentity::parse("two-scope-tmux").unwrap();
    let inventories = captured.into_iter().map(|captured| {
        let worktree = match captured.captured_worktree {
            Some(CapturedWorktreeIdentity::Resolved(worktree)) => Some(worktree),
            None => None,
            other => panic!("unexpected captured worktree identity: {other:?}"),
        };
        crate::CaptureCloseRetirementInventoryScopeRequest {
            scope: captured.scope.clone(),
            inventory: CloseOwnedResourceInventory {
                worktree, work_scopes: BTreeSet::new(),
                bash_process_groups: BTreeSet::new(),
                tmux_servers: if captured.scope == child_scope { [tmux.clone()].into() }
                    else { BTreeSet::new() },
                pty_sessions: BTreeSet::new(), browser_sessions: BTreeSet::new(),
                equivalent_live_resources: BTreeSet::new(),
            },
        }
    }).collect();
    let LocalAuthorityResult::DurableFactEstablished(Ok(resources)) =
        db.capture_native_close_initial_retirement_inventory(&command, &receipt, &source,
            crate::CaptureCloseRetirementInventoryRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(), scopes: inventories,
            }).await else { panic!("expected sealed inventory"); };
    let retired = resources.iter().find(|target| target.scope == root_scope
        && target.resource.kind() == RetiredResourceKind::WorkScope).unwrap();
    assert!(matches!(db.record_native_close_initial_retirement_dispatch(
        &command, &receipt, &source, RecordCloseRetirementDispatchRequest {
            attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
            scope: retired.scope.clone(), resource: retired.resource.clone(),
        },
    ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    assert!(matches!(db.record_native_close_initial_retirement_evidence(
        &command, &receipt, &source, RecordCloseRetirementEvidenceRequest {
            attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
            scope: retired.scope.clone(), resource: retired.resource.clone(),
            outcome: RetirementOutcome::Retired, detail: Some("already retired".into()),
        },
    ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    let dispatched = resources.iter().find(|target| target.scope == child_scope
        && target.resource.kind() == RetiredResourceKind::TmuxServer).unwrap();
    assert!(matches!(
        db.record_native_close_initial_retirement_dispatch(
            &command,
            &receipt,
            &source,
            RecordCloseRetirementDispatchRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scope: dispatched.scope.clone(), resource: dispatched.resource.clone(),
            },
        ).await,
        LocalAuthorityResult::DurableFactEstablished(Ok(()))
    ));
    let expected = db.unresolved_expected_close_cleanup_resources(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(expected.len(), resources.len() - 1);
    let dispatch_sql = "SELECT scope, inspection_generation, inspection_fingerprint,
        resource_kind, identity_kind, identity_codec, identity_value, dispatched_at_us
        FROM close_retirement_resource_dispatches WHERE attempt_id = ?1
        ORDER BY scope, resource_kind, identity_kind, identity_value";
    type DispatchFact = (String, String, String, String, String, String, String, i64);
    let dispatches_before = sqlx::query_as::<_, DispatchFact>(dispatch_sql)
        .bind(run.attempt_id.as_str())
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert_eq!(dispatches_before.len(), 2);
    assert!(dispatches_before.iter().any(|fact| fact.0 == root_scope.as_str()
        && fact.3 == "work_scope"));
    assert!(dispatches_before.iter().any(|fact| fact.0 == child_scope.as_str()
        && fact.3 == "tmux_server"));
    assert!(!expected.iter().any(|item| item.scope == retired.scope
        && item.resource == retired.resource));
    let allowance_before = source_fence_allowance(&db, &run).await;
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    let failure = &failures[0].occurrence;
    assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
    assert_eq!(failure.stop_certainty, CloseStopCertainty::ShutdownUncertain);
    assert!(matches!(&failure.authority, CloseCleanupFailureAuthority::ExpectedResource {
        scope, snapshot: retained, resource,
    } if scope == &expected[0].scope && retained == &snapshot
        && resource == &expected[0].resource));
    assert_eq!(failure.remaining_resources.len(), expected.len());
    for item in &expected {
        assert!(failure.remaining_resources.iter().any(|remaining|
            remaining.scope == item.scope && remaining.resource == item.resource
                && remaining.disposition == if item == &expected[0] {
                    CloseCleanupResourceDisposition::Failed
                } else { item.disposition }));
    }
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT proof_kind FROM close_retirement_resources
        WHERE attempt_id = ?1 AND scope = ?2 AND resource_kind = 'work_scope'")
        .bind(run.attempt_id.as_str()).bind(root_scope.as_str())
        .fetch_one(db.pool()).await.unwrap(), "retired");
    assert_eq!(
        sqlx::query_as::<_, DispatchFact>(dispatch_sql)
            .bind(run.attempt_id.as_str())
            .fetch_all(db.pool())
            .await
            .unwrap(),
        dispatches_before,
        "source fencing must preserve both exact prior dispatch facts without adding effects"
    );
    assert_eq!(source_fence_allowance(&db, &run).await,
        allowance_before.map(|(consumed, _)| (consumed, 1)));
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);
    db.reset_all_to_idle().await.unwrap();
    assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().len(), 1);
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn initial_source_fence_refuses_all_complete_sealed_inventory_without_mutation() {
    use phoenix_core::domain::close::{CapturedWorktreeIdentity, CloseOwnedResourceInventory,
        CloseRetirementSnapshot};

    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "all-complete-source-fence").await;
    let scope = allocate_scope_worktree(&db, "all-complete-source-fence").await;
    let command = retry_command(&db, "all-complete-source-fence", NativeCloseIntent::Initial {
        target: product_id("all-complete-source-fence"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    let scope_snapshot = CloseRetirementSnapshot::parse("complete-generation", "complete-fingerprint").unwrap();
    assert!(db.replace_native_close_initial_inspection(&command, &receipt, &source,
        crate::ReplaceCloseInspectionRequest {
            attempt_id: run.attempt_id.clone(),
            scopes: vec![crate::ReplaceCloseInspectionScopeRequest {
                scope: scope.clone(), snapshot: scope_snapshot.clone(), losses: vec![],
            }],
        }, None).await.established().unwrap().is_ok());
    let snapshot = CloseRetirementSnapshot::parse(
        encode_aggregate_snapshot_component([(&scope, scope_snapshot.generation())]),
        encode_aggregate_snapshot_component([(&scope, scope_snapshot.fingerprint())]),
    ).unwrap();
    let captured = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap();
    let Some(CapturedWorktreeIdentity::Resolved(worktree)) = captured[0].captured_worktree.clone()
        else { panic!("expected captured worktree"); };
    let LocalAuthorityResult::DurableFactEstablished(Ok(resources)) =
        db.capture_native_close_initial_retirement_inventory(&command, &receipt, &source,
            crate::CaptureCloseRetirementInventoryRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scopes: vec![crate::CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree: Some(worktree), work_scopes: BTreeSet::new(),
                        bash_process_groups: BTreeSet::new(), tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(), browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                }],
            }).await else { panic!("expected sealed inventory"); };
    for target in &resources {
        assert!(matches!(db.record_native_close_initial_retirement_dispatch(
            &command, &receipt, &source, RecordCloseRetirementDispatchRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scope: target.scope.clone(), resource: target.resource.clone(),
            },
        ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
        assert!(matches!(db.record_native_close_initial_retirement_evidence(
            &command, &receipt, &source, RecordCloseRetirementEvidenceRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scope: target.scope.clone(), resource: target.resource.clone(),
                outcome: RetirementOutcome::Retired, detail: Some("complete proof".into()),
            },
        ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    }
    assert!(db.unresolved_expected_close_cleanup_resources(run.attempt_id.as_str()).await.unwrap().is_empty());

    let source_before = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    let run_before = db.get_close_run(&run).await.unwrap();
    let obligation_before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    let allowance_before = source_fence_allowance(&db, &run).await;
    assert!(write_source_fence_state(&db, &command, &ConvState::Idle, false).await.is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source_before);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, run_before.status);
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation_before);
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert_eq!(source_fence_allowance(&db, &run).await, allowance_before);
    assert!(db.pending_coordinator_watch_events(100).await.unwrap().is_empty());
}

#[tokio::test]
async fn startup_classifies_partial_initial_inspection_without_suppressing_allowance() {
    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "startup-partial-initial").await;
    let scope = allocate_scope_worktree(&db, "startup-partial-initial").await;
    let command = retry_command(&db, "startup-partial-initial", NativeCloseIntent::Initial {
        target: product_id("startup-partial-initial"),
    }).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await else { panic!("expected admission"); };
    let run = CloseRunRef::initial(receipt.admitted.attempt_id.clone());
    let source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &source).await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &source).await.established().unwrap().is_ok());
    sqlx::query("INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
                 VALUES (?1, ?2, 'partial-generation', 'partial-fingerprint', '2026-11-16T00:00:00Z')")
        .bind(run.attempt_id.as_str()).bind(scope.as_str()).execute(db.pool()).await.unwrap();
    db.reset_all_to_idle().await.unwrap();
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    let failure = &failures[0].occurrence;
    assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
    assert!(failure.detail.contains("process_interrupted"));
    assert!(matches!(failure.authority, CloseCleanupFailureAuthority::CapturedScope { .. }));
    assert!(failure.remaining_resources.iter().any(|item| item.scope == scope
        && item.resource.kind() == RetiredResourceKind::WorkScope
        && item.disposition == CloseCleanupResourceDisposition::Unknown));
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
    db.reset_all_to_idle().await.unwrap();
    assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().len(), 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep the partial-inspection fixture and atomic classification proof together.
async fn initial_source_fence_classifies_partial_inspection_without_dispatch() {
    let db = Database::open_in_memory().await.unwrap();
    create_root(&db, "initial-source-fence-inspection").await;
    let scope = allocate_scope_worktree(&db, "initial-source-fence-inspection").await;
    let command = retry_command(
        &db,
        "initial-source-fence-inspection",
        NativeCloseIntent::Initial {
            target: product_id("initial-source-fence-inspection"),
        },
    )
    .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok(
        crate::native_close::NativeCloseInitialResult::NewlyAdmitted(receipt),
    )) = db.apply_native_close_initial(&command).await
    else {
        panic!("expected initial native admission");
    };
    let attempt_id = receipt.admitted.attempt_id.clone();
    let run = CloseRunRef::initial(attempt_id.clone());
    let live_source = db
        .get_conversation(command.source_transcript_id())
        .await
        .unwrap()
        .state;
    assert!(db.prepare_native_close_initial_execution(&command, &receipt, &live_source)
            .await.established().unwrap().is_ok());
    assert!(db.settle_native_close_initial(&command, &receipt, &live_source)
            .await.established().unwrap().is_ok());

    // Synthetic partial-inspection crash fixture: the aggregate snapshot remains null.
    sqlx::query(
        "INSERT INTO close_retirement_inspections (attempt_id, scope, generation, fingerprint, inspected_at)
         VALUES (?1, ?2, 'inspection-generation', 'inspection-fingerprint', '2026-11-16T00:00:00Z')",
    )
    .bind(attempt_id.as_str())
    .bind(scope.as_str())
    .execute(db.pool())
    .await
    .unwrap();

    let source_before = db
        .get_conversation(command.source_transcript_id())
        .await
        .unwrap()
        .state;
    let run_before = sqlx::query_as::<_, (String, i64)>(
        "SELECT status, created_at_us FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = ?2",
    )
    .bind(run.attempt_id.as_str())
    .bind(run.ordinal.get())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let obligation_before = db
        .get_close_obligation(attempt_id.as_str())
        .await
        .unwrap();
    let failures_before = db
        .list_close_cleanup_failures(attempt_id.as_str())
        .await
        .unwrap();
    let allowance_before = source_fence_allowance(&db, &run).await;

    assert!(!matches!(source_before, ConvState::Idle));
    assert_eq!(run_before.0, "running");
    assert_eq!(obligation_before.snapshot(), None);
    assert!(failures_before.is_empty());
    assert_eq!(allowance_before, Some((0, 0)));
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let obligation = db.get_close_obligation(attempt_id.as_str()).await.unwrap();
    assert_eq!(obligation.phase(), ClosePhase::Completed);
    assert_eq!(obligation.snapshot(), None);
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    let failures = db.list_close_cleanup_failures(attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    let failure = &failures[0].occurrence;
    assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
    assert_eq!(failure.stop_certainty, CloseStopCertainty::ShutdownUncertain);
    assert!(failure.detail.contains("source_fenced"));
    assert_eq!(failure.authority.scope(), Some(&scope));
    assert!(matches!(failure.authority, CloseCleanupFailureAuthority::CapturedScope { .. }));
    assert!(failure.remaining_resources.iter().any(|item| item.scope == scope
        && item.resource.kind() == RetiredResourceKind::WorkScope
        && item.disposition == CloseCleanupResourceDisposition::Unknown));
    assert!(failure.remaining_resources.iter().any(|item| item.scope == scope
        && item.resource.kind() == RetiredResourceKind::Worktree
        && item.disposition == CloseCleanupResourceDisposition::Unknown));
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1")
        .bind(attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), 0);
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert_eq!(db.list_close_cleanup_failures(attempt_id.as_str()).await.unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM close_retirement_inspections WHERE attempt_id = ?1",
        )
        .bind(attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}
