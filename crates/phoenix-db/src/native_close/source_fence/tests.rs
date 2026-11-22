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

// Installed-prefix fixtures deliberately exercise their historical fence, not
// migration168 origin logic or a production compatibility bypass.
async fn historical_write_source_fence_state(
    db: &Database,
    command: &NativeCloseCommand,
    state: &ConvState,
) -> DbResult<()> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    crate::native_close::fence_pre_completion_native_sources_tx(
        &mut tx, command.source_transcript_id(), state,
        crate::native_close::NativeSourceLossCause::UserSourceLoss,
    ).await?;
    sqlx::query("UPDATE conversations SET state = ?2, state_kind = ?3 WHERE id = ?1")
        .bind(command.source_transcript_id()).bind(serde_json::to_string(state)
            .map_err(|error| DbError::Serialization(error.to_string()))?)
        .bind(crate::conv_state_kind(state))
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
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
async fn migration_179_populated_upgrade_preserves_retry_and_adds_initial_source_authority() {
    let (retry_db, assessment, receipt) = migration_176_pending_fixture().await;
    assert_eq!(
        crate::migrations::run_through_for_test(retry_db.pool(), 178)
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
    crate::migrations::setup_real_175_for_test(&initial_pool).await;
    let initial_db = Database::from_pool_for_tests(initial_pool, String::new());
    assert_eq!(
        crate::migrations::run_through_for_test(initial_db.pool(), 178)
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
    // Reproduce the installed165 admission SQL, without asking the current168
    // API to execute against an intentionally historical schema.
    let initial_obligation = initial_db.begin_close_foundation(
        &product_id("migration166-initial"),
        &TranscriptConversationId::parse("migration166-initial").unwrap(),
        "migration166-initial-attempt",
    ).await.unwrap();
    sqlx::query("UPDATE close_obligations SET native_managed = 1 WHERE attempt_id = ?1")
        .bind(initial_obligation.attempt_id().as_str()).execute(initial_db.pool()).await.unwrap();
    sqlx::query("INSERT INTO native_close_initial_receipts
        (request_id, request_fingerprint, source_transcript_id, source_message_id, source_tool_use_id,
         product_conversation_id, intent, attempt_id, run_ordinal)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'initial', ?7, 1)")
        .bind(initial_command.request_id().to_string()).bind(initial_command.fingerprint())
        .bind(initial_command.source_transcript_id()).bind(&initial_command.source().message_id)
        .bind(&initial_command.source().tool_use_id).bind(initial_command.intent().target().as_str())
        .bind(initial_obligation.attempt_id().as_str()).execute(initial_db.pool()).await.unwrap();
    let initial_run = CloseRunRef::initial(initial_obligation.attempt_id().clone());

    assert_eq!(
        crate::migrations::run_through_for_test(retry_db.pool(), 179)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        crate::migrations::run_through_for_test(initial_db.pool(), 179)
            .await
            .unwrap(),
        1
    );
    historical_write_source_fence_state(&retry_db, &assessment.command, &ConvState::Idle)
        .await
        .unwrap();
    assert_source_fenced(&retry_db, &assessment, &retry_run, &retry_pending).await;
    assert_eq!(source_fence_allowance(&retry_db, &retry_run).await, Some((0, 1)));

    historical_write_source_fence_state(&initial_db, &initial_command, &ConvState::Idle)
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

async fn all_complete_initial_source_fence_fixture() -> (
    Database,
    NativeCloseCommand,
    crate::native_close::NativeCloseReceipt,
    ConvState,
    CloseRunRef,
) {
    all_complete_initial_source_fence_fixture_with_omissions(false, None, false).await
}

async fn all_complete_initial_source_fence_fixture_with_omissions(
    omit_process_success: bool,
    empty_generation: Option<&str>,
    omit_bash_dispatch: bool,
) -> (
    Database,
    NativeCloseCommand,
    crate::native_close::NativeCloseReceipt,
    ConvState,
    CloseRunRef,
) {
    initial_source_fence_inventory_fixture(omit_process_success, empty_generation, omit_bash_dispatch, true, false).await
}

async fn initial_source_fence_inventory_fixture(
    omit_process_success: bool,
    empty_generation: Option<&str>,
    omit_bash_dispatch: bool,
    record_completion: bool,
    include_second_bash: bool,
) -> (
    Database,
    NativeCloseCommand,
    crate::native_close::NativeCloseReceipt,
    ConvState,
    CloseRunRef,
) {
    let db = Database::open_in_memory().await.unwrap();
    initial_source_fence_inventory_fixture_with_db(
        db,
        omit_process_success,
        empty_generation,
        omit_bash_dispatch,
        record_completion,
        include_second_bash,
    )
    .await
}

async fn initial_source_fence_inventory_fixture_with_db(
    db: Database,
    omit_process_success: bool,
    empty_generation: Option<&str>,
    omit_bash_dispatch: bool,
    record_completion: bool,
    include_second_bash: bool,
) -> (
    Database,
    NativeCloseCommand,
    crate::native_close::NativeCloseReceipt,
    ConvState,
    CloseRunRef,
) {
    use phoenix_core::domain::close::{CapturedWorktreeIdentity, CloseOwnedResourceInventory,
        CloseRetirementSnapshot, OpaqueIdentity};

    create_root(&db, "all-complete-source-fence").await;
    let scope = if empty_generation.is_some() {
        db.get_conversation("all-complete-source-fence").await.unwrap().attached_work_scope_id.unwrap()
    } else {
        allocate_scope_worktree(&db, "all-complete-source-fence").await
    };
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
            scopes: if empty_generation.is_some() { vec![] } else {
                vec![crate::ReplaceCloseInspectionScopeRequest {
                    scope: scope.clone(), snapshot: scope_snapshot.clone(), losses: vec![],
                }]
            },
        }, empty_generation).await.established().unwrap().is_ok());
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let captured = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap();
    let worktree = match captured[0].captured_worktree.clone() {
        Some(CapturedWorktreeIdentity::Resolved(worktree)) => Some(worktree),
        None if empty_generation.is_some() => None,
        _ => panic!("expected captured worktree or empty inspection"),
    };
    let LocalAuthorityResult::DurableFactEstablished(Ok(resources)) =
        db.capture_native_close_initial_retirement_inventory(&command, &receipt, &source,
            crate::CaptureCloseRetirementInventoryRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scopes: vec![crate::CaptureCloseRetirementInventoryScopeRequest {
                    scope: scope.clone(),
                    inventory: CloseOwnedResourceInventory {
                        worktree,
                        work_scopes: BTreeSet::new(),
                        bash_process_groups: if include_second_bash {
                            [OpaqueIdentity::parse("complete-bash-session").unwrap(),
                             OpaqueIdentity::parse("complete-bash-session-b").unwrap()].into()
                        } else {
                            [OpaqueIdentity::parse("complete-bash-session").unwrap()].into()
                        },
                        tmux_servers: BTreeSet::new(),
                        pty_sessions: BTreeSet::new(), browser_sessions: BTreeSet::new(),
                        equivalent_live_resources: BTreeSet::new(),
                    },
                }],
            }).await else { panic!("expected sealed inventory"); };
    for target in &resources {
        if omit_bash_dispatch && target.resource.kind() == RetiredResourceKind::BashProcessGroup {
            continue;
        }
        assert!(matches!(db.record_native_close_initial_retirement_dispatch(
            &command, &receipt, &source, RecordCloseRetirementDispatchRequest {
                attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                scope: target.scope.clone(), resource: target.resource.clone(),
            },
        ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
        if !record_completion {
            continue;
        }
        if target.resource.kind() == RetiredResourceKind::BashProcessGroup && !omit_process_success {
            let success = crate::CloseProcessStepSuccess {
                run: run.clone(),
                scope: target.scope.clone(),
                resource_kind: crate::CloseProcessResourceKind::BashProcessGroup,
                identity: OpaqueIdentity::parse("complete-bash-session").unwrap(),
                outcome: crate::CloseProcessStepOutcome::Retired,
                observed_at_us: chrono::Utc::now().timestamp_micros(),
            };
            assert!(db.record_close_process_step_success(&success).await.is_err());
            assert!(matches!(db.record_native_close_initial_process_step_success(
                &command, &receipt, &source, snapshot.clone(), success,
            ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
        }
        {
            assert!(matches!(db.record_native_close_initial_retirement_evidence(
                &command, &receipt, &source, RecordCloseRetirementEvidenceRequest {
                    attempt_id: run.attempt_id.clone(), snapshot: snapshot.clone(),
                    scope: target.scope.clone(), resource: target.resource.clone(),
                    outcome: RetirementOutcome::Retired, detail: Some("complete proof".into()),
                },
            ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
        }
    }
    assert!(!resources.is_empty());
    assert!(resources.iter().any(|target| {
        target.resource.kind() == RetiredResourceKind::BashProcessGroup
    }));
    assert!(resources.iter().any(|target| {
        target.resource.kind() == RetiredResourceKind::WorkScope
    }));
    if record_completion && !omit_process_success {
        assert!(db.unresolved_expected_close_cleanup_resources(run.attempt_id.as_str())
            .await.unwrap().is_empty());
    }
    (db, command, receipt, source, run)
}

#[tokio::test]
async fn initial_process_authorization_requires_exact_durable_source_scope_snapshot_and_dispatch() {
    use phoenix_core::domain::close::{CloseRetirementSnapshot, RetiredResourceKind};

    let (db, command, receipt, source, run) = initial_source_fence_inventory_fixture(false, None, false, false, false).await;
    let inventories = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap();
    let process = inventories.iter().find(|item| {
        item.resource.kind() == RetiredResourceKind::BashProcessGroup
    }).unwrap();
    let auth = db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, process.scope.clone(), process.snapshot.clone(),
        RetiredResourceKind::BashProcessGroup,
    ).await.established().unwrap().unwrap();
    let (authorized_run, authorized_scope, authorized_snapshot, kind, process_inventory) = auth.into_parts();
    assert_eq!(authorized_run.attempt_id, run.attempt_id);
    assert_eq!(authorized_scope, process.scope);
    assert_eq!(authorized_snapshot, process.snapshot);
    assert_eq!(kind, RetiredResourceKind::BashProcessGroup);
    assert_eq!(process_inventory.len(), 1);
    assert!(process_inventory.iter().all(|item| {
        item.resource.kind() == RetiredResourceKind::BashProcessGroup
    }));

    // A process-kind absent from this sealed inventory is valid but yields no work.
    let empty = db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, process.scope.clone(), process.snapshot.clone(),
        RetiredResourceKind::BrowserSession,
    ).await.established().unwrap().unwrap();
    assert!(empty.into_parts().4.is_empty());
    // A second transaction-time check does not claim a physical launch.
    assert!(db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, process.scope.clone(), process.snapshot.clone(),
        RetiredResourceKind::BashProcessGroup,
    ).await.established().unwrap().is_ok());

    for (scope, snapshot, kind) in [
        (process.scope.clone(), CloseRetirementSnapshot::parse("wrong-generation", "wrong-fingerprint").unwrap(), RetiredResourceKind::BashProcessGroup),
        (phoenix_core::work_scope::WorkScopeId::new(), process.snapshot.clone(), RetiredResourceKind::BashProcessGroup),
        (process.scope.clone(), process.snapshot.clone(), RetiredResourceKind::WorkScope),
        (process.scope.clone(), process.snapshot.clone(), RetiredResourceKind::EquivalentLiveResource),
    ] {
        assert!(db.authorize_native_close_initial_process_step(
            &command, &receipt, &source, scope, snapshot, kind,
        ).await.established().unwrap().is_err());
    }
}

#[tokio::test]
async fn initial_process_authorization_rejects_missing_dispatch_and_stopped_or_changed_source() {
    use phoenix_core::domain::close::RetiredResourceKind;

    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture(false, None, true, false, false).await;
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    assert!(db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, item.scope.clone(), item.snapshot.clone(), RetiredResourceKind::BashProcessGroup,
    ).await.established().unwrap().is_err());

    let (db, command, receipt, source, run) = initial_source_fence_inventory_fixture(false, None, false, false, false).await;
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    assert!(db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, item.scope, item.snapshot, RetiredResourceKind::BashProcessGroup,
    ).await.established().unwrap().is_err());
}

#[tokio::test]
async fn exact_initial_process_entry_is_per_resource_not_whole_kind() {
    use crate::native_close::{
        NativeCloseInitialEntryRelease, NativeCloseInitialResourceEntryDecision as Decision,
        NativeCloseInitialResourceEntryResult as EntryResult,
    };
    use phoenix_core::domain::close::RetiredResourceKind;

    for process_receipt in [false, true] {
    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture(false, None, false, false, true).await;
    let resources = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap();
    let mut bash: Vec<_> = resources.into_iter()
        .filter(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).collect();
    bash.sort_by_key(|item| item.resource.identity().value().to_owned());
    assert_eq!(bash.len(), 2);
    let a = &bash[0];
    let b = &bash[1];
    if process_receipt {
        let mut success = initial_bash_process_success(&run, &a.scope);
        success.identity = phoenix_core::domain::close::OpaqueIdentity::parse(a.resource.identity().value()).unwrap();
        assert!(db.record_native_close_initial_process_step_success(
            &command, &receipt, &source, a.snapshot.clone(), success,
        ).await.established().unwrap().is_ok());
    } else {
        assert!(db.record_native_close_initial_retirement_evidence(
            &command, &receipt, &source, crate::RecordCloseRetirementEvidenceRequest {
                attempt_id: run.attempt_id.clone(), snapshot: a.snapshot.clone(),
                scope: a.scope.clone(), resource: a.resource.clone(),
                outcome: phoenix_core::domain::close::RetirementOutcome::Retired, detail: None,
            },
        ).await.established().unwrap().is_ok());
    }

    let mut called = false;
    assert!(matches!(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, a.scope.clone(), a.snapshot.clone(), &a.resource,
        || { called = true; Decision::Entered },
    ).await, EntryResult::Rejected { release: NativeCloseInitialEntryRelease::Confirmed, .. }));
    assert!(!called);

    // Whole-kind compatibility remains deliberately conservative: A's success blocks the set.
    assert!(db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, a.scope.clone(), a.snapshot.clone(),
        RetiredResourceKind::BashProcessGroup,
    ).await.established().unwrap().is_err());

    assert!(matches!(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, b.scope.clone(), b.snapshot.clone(), &b.resource,
        || Decision::Entered,
    ).await, EntryResult::Invoked {
        decision: Decision::Entered,
        release: NativeCloseInitialEntryRelease::Confirmed,
    }));
    }
}

#[tokio::test]
async fn exact_initial_process_entry_missing_dispatch_never_invokes_handoff() {
    use crate::native_close::{
        NativeCloseInitialResourceEntryDecision as Decision,
        NativeCloseInitialResourceEntryResult as EntryResult,
    };
    use phoenix_core::domain::close::RetiredResourceKind;

    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture(false, None, true, false, false).await;
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    let mut called = false;
    assert!(matches!(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, item.scope, item.snapshot, &item.resource,
        || { called = true; Decision::Entered },
    ).await, EntryResult::Rejected { .. }));
    assert!(!called);
}

#[tokio::test]
async fn exact_initial_process_entry_rejects_stopped_and_restored_source_without_handoff() {
    use crate::native_close::{
        NativeCloseInitialEntryRelease, NativeCloseInitialResourceEntryDecision as Decision,
        NativeCloseInitialResourceEntryResult as EntryResult,
    };
    use phoenix_core::domain::close::RetiredResourceKind;

    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture(false, None, false, false, false).await;
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    for restore in [false, true] {
        if restore {
            write_source_fence_state(&db, &command, &source, false).await.unwrap();
        }
        let mut called = false;
        assert!(matches!(db.enter_native_close_initial_process_resource(
            &command, &receipt, &source, item.scope.clone(), item.snapshot.clone(), &item.resource,
            || { called = true; Decision::Entered },
        ).await, EntryResult::Rejected { release: NativeCloseInitialEntryRelease::Confirmed, .. }));
        assert!(!called);
    }
}

#[tokio::test]
async fn exact_initial_process_entry_revalidates_after_independent_writer_revokes_source() {
    use std::{future::Future, task::Poll};
    use crate::native_close::{
        NativeCloseInitialEntryRelease, NativeCloseInitialResourceEntryDecision as Decision,
        NativeCloseInitialResourceEntryResult as EntryResult,
    };
    use phoenix_core::domain::close::RetiredResourceKind;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("entry.sqlite");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    // File-backed open runs only legacy schema setup; mirror numbered startup migrations.
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture_with_db(db, false, None, false, false, false).await;
    let writer = Database::open(path.to_str().unwrap()).await.unwrap();
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    let mut tx = writer.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let mut called = false;
    let mut entry = Box::pin(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, item.scope, item.snapshot, &item.resource,
        || { called = true; Decision::Entered },
    ));
    // Poll once while the independent writer owns the reservation. This does not
    // assume whether the pending entry is at pool acquisition or SQLite's busy wait.
    std::future::poll_fn(|cx| {
        assert!(entry.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    }).await;
    crate::write_conversation_state_tx(
        &mut tx, command.source_transcript_id(), &ConvState::Idle, Utc::now(),
    ).await.unwrap();
    tx.commit().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), entry).await.unwrap();
    assert!(matches!(result, EntryResult::Rejected {
        release: NativeCloseInitialEntryRelease::Confirmed, ..
    }));
    assert!(!called);
}

#[tokio::test]
async fn exact_initial_process_entry_releases_writer_before_retained_cleanup_finishes() {
    use crate::native_close::{
        NativeCloseInitialEntryRelease, NativeCloseInitialResourceEntryDecision as Decision,
        NativeCloseInitialResourceEntryResult as EntryResult,
    };
    use phoenix_core::domain::close::RetiredResourceKind;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("entry.sqlite");
    let db = Database::open(path.to_str().unwrap()).await.unwrap();
    // File-backed open runs only legacy schema setup; mirror numbered startup migrations.
    crate::migrations::run_pending_migrations(db.pool()).await.unwrap();
    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture_with_db(db, false, None, false, false, true).await;
    let writer = Database::open(path.to_str().unwrap()).await.unwrap();
    let bash: Vec<_> = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().filter(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).collect();
    assert_eq!(bash.len(), 2);
    let (release, pending_cleanup) = tokio::sync::oneshot::channel::<()>();
    let mut retained_cleanup = None;
    assert!(matches!(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, bash[0].scope.clone(), bash[0].snapshot.clone(), &bash[0].resource,
        || { retained_cleanup = Some(pending_cleanup); Decision::Entered },
    ).await, EntryResult::Invoked {
        decision: Decision::Entered, release: NativeCloseInitialEntryRelease::Confirmed,
    }));
    let mut retained_cleanup = retained_cleanup.unwrap();
    assert!(matches!(retained_cleanup.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
    tokio::time::timeout(std::time::Duration::from_secs(5),
        writer.update_conversation_state(command.source_transcript_id(), &ConvState::Idle),
    ).await.unwrap().unwrap();
    let mut called = false;
    assert!(matches!(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, bash[1].scope.clone(), bash[1].snapshot.clone(), &bash[1].resource,
        || { called = true; Decision::Entered },
    ).await, EntryResult::Rejected { release: NativeCloseInitialEntryRelease::Confirmed, .. }));
    assert!(!called);
    release.send(()).unwrap();
    retained_cleanup.await.unwrap();
}

#[tokio::test]
async fn exact_initial_process_entry_requires_original_binding_and_preserves_local_refusal() {
    use crate::native_close::{
        NativeCloseInitialEntryRelease, NativeCloseInitialResourceEntryDecision as Decision,
        NativeCloseInitialResourceEntryResult as EntryResult,
    };
    use phoenix_core::domain::close::{CloseRetirementSnapshot, LossItemIdentity, RetiredResourceIdentity, RetiredResourceKind};

    let (db, command, receipt, source, run) =
        initial_source_fence_inventory_fixture(false, None, false, false, false).await;
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    let foreign = RetiredResourceIdentity::parse(
        RetiredResourceKind::BashProcessGroup,
        LossItemIdentity::Opaque(phoenix_core::domain::close::OpaqueIdentity::parse("foreign-process").unwrap()),
    ).unwrap();
    let wrong_snapshot = CloseRetirementSnapshot::parse("wrong-generation", "wrong-fingerprint").unwrap();
    for (scope, snapshot, resource) in [
        (item.scope.clone(), item.snapshot.clone(), foreign),
        (item.scope.clone(), wrong_snapshot, item.resource.clone()),
        (phoenix_core::work_scope::WorkScopeId::new(), item.snapshot.clone(), item.resource.clone()),
    ] {
        let mut called = false;
        assert!(matches!(db.enter_native_close_initial_process_resource(
            &command, &receipt, &source, scope, snapshot, &resource,
            || { called = true; Decision::Entered },
        ).await, EntryResult::Rejected { release: NativeCloseInitialEntryRelease::Confirmed, .. }));
        assert!(!called);
    }
    assert!(matches!(db.enter_native_close_initial_process_resource(
        &command, &receipt, &source, item.scope, item.snapshot, &item.resource,
        || Decision::Refused,
    ).await, EntryResult::Invoked {
        decision: Decision::Refused, release: NativeCloseInitialEntryRelease::Confirmed,
    }));
}

#[tokio::test]
async fn initial_process_authorization_rejects_source_changed_then_restored() {
    use phoenix_core::domain::close::RetiredResourceKind;

    let (db, command, receipt, source, run) = initial_source_fence_inventory_fixture(false, None, false, false, false).await;
    let item = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
        .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    write_source_fence_state(&db, &command, &source, false).await.unwrap();
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
    assert!(db.authorize_native_close_initial_process_step(
        &command, &receipt, &source, item.scope, item.snapshot, RetiredResourceKind::BashProcessGroup,
    ).await.established().unwrap().is_err());
}

#[tokio::test]
async fn initial_process_authorization_rejects_prior_process_success_or_positive_retirement() {
    use phoenix_core::domain::close::RetiredResourceKind;

    for record_process_receipt in [false, true] {
        let (db, command, receipt, source, run) = initial_source_fence_inventory_fixture(false, None, false, false, false).await;
        let process = db.list_close_expected_retirement_resources(run.attempt_id.as_str()).await.unwrap()
            .into_iter().find(|item| item.resource.kind() == RetiredResourceKind::BashProcessGroup).unwrap();
        if record_process_receipt {
            assert!(db.record_native_close_initial_process_step_success(
                &command, &receipt, &source, process.snapshot.clone(),
                initial_bash_process_success(&run, &process.scope),
            ).await.established().unwrap().is_ok());
        } else {
            assert!(db.record_native_close_initial_retirement_evidence(
                &command, &receipt, &source, RecordCloseRetirementEvidenceRequest {
                    attempt_id: run.attempt_id.clone(), scope: process.scope.clone(),
                    snapshot: process.snapshot.clone(), resource: process.resource.clone(),
                    outcome: RetirementOutcome::Retired, detail: None,
                },
            ).await.established().unwrap().is_ok());
        }
        assert!(db.authorize_native_close_initial_process_step(
            &command, &receipt, &source, process.scope, process.snapshot,
            RetiredResourceKind::BashProcessGroup,
        ).await.established().unwrap().is_err());
    }
}

fn initial_bash_process_success(
    run: &CloseRunRef,
    scope: &phoenix_core::work_scope::WorkScopeId,
) -> crate::CloseProcessStepSuccess {
    crate::CloseProcessStepSuccess {
        run: run.clone(),
        scope: scope.clone(),
        resource_kind: crate::CloseProcessResourceKind::BashProcessGroup,
        identity: phoenix_core::domain::close::OpaqueIdentity::parse("complete-bash-session").unwrap(),
        outcome: crate::CloseProcessStepOutcome::Retired,
        observed_at_us: chrono::Utc::now().timestamp_micros(),
    }
}

async fn initial_process_success_count(db: &Database, run: &CloseRunRef) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_process_step_successes
         WHERE attempt_id = ?1 AND run_ordinal = 1 AND resource_kind = 'bash_process_group'",
    )
    .bind(run.attempt_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn initial_process_success_is_source_bound_idempotent_and_generic_writer_cannot_replay() {
    let (db, command, receipt, source, run) =
        all_complete_initial_source_fence_fixture_with_omissions(true, None, false).await;
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let scope = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap()[0]
        .scope.clone();
    let success = initial_bash_process_success(&run, &scope);

    assert!(db.record_close_process_step_success(&success).await.is_err());
    assert_eq!(initial_process_success_count(&db, &run).await, 0);
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot.clone(), success.clone(),
    ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    assert_eq!(initial_process_success_count(&db, &run).await, 1);
    let retained = db.list_close_process_step_successes(&run).await.unwrap();
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot.clone(), success.clone(),
    ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    assert_eq!(db.list_close_process_step_successes(&run).await.unwrap(), retained);
    let mut conflicting_time = success.clone();
    conflicting_time.observed_at_us -= 1;
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot.clone(), conflicting_time,
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    assert_eq!(db.list_close_process_step_successes(&run).await.unwrap(), retained);

    let mut conflicting = success.clone();
    conflicting.outcome = crate::CloseProcessStepOutcome::AbsenceVerified;
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot, conflicting,
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    assert!(db.record_close_process_step_success(&success).await.is_err());
    assert_eq!(initial_process_success_count(&db, &run).await, 1);
}

#[tokio::test]
async fn initial_process_success_requires_exact_dispatch() {
    let (db, command, receipt, source, run) =
        all_complete_initial_source_fence_fixture_with_omissions(true, None, true).await;
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let scope = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap()[0]
        .scope.clone();
    let success = initial_bash_process_success(&run, &scope);
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot, success,
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    assert_eq!(initial_process_success_count(&db, &run).await, 0);
}

#[tokio::test]
async fn initial_process_success_rejects_wrong_live_source_and_revoked_restored_source() {
    let (db, command, receipt, source, run) =
        all_complete_initial_source_fence_fixture_with_omissions(true, None, false).await;
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let scope = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap()[0]
        .scope.clone();
    let success = initial_bash_process_success(&run, &scope);
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &ConvState::Idle, snapshot.clone(), success.clone(),
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    assert_eq!(initial_process_success_count(&db, &run).await, 0);

    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot.clone(), success.clone(),
    ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot.clone(), success.clone(),
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    write_source_fence_state(&db, &command, &source, false).await.unwrap();
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot, success,
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    assert_eq!(initial_process_success_count(&db, &run).await, 1);
}

#[tokio::test]
async fn initial_process_success_requires_exact_run_snapshot_scope_and_identity() {
    let (db, command, receipt, source, run) =
        all_complete_initial_source_fence_fixture_with_omissions(true, None, false).await;
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let scope = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap()[0]
        .scope.clone();
    let success = initial_bash_process_success(&run, &scope);
    let mut wrong_attempt = success.clone();
    wrong_attempt.run.attempt_id = phoenix_core::domain::close::CloseAttemptId::parse(
        format!("{}-other", run.attempt_id),
    ).unwrap();
    let mut wrong_ordinal = success.clone();
    wrong_ordinal.run.ordinal = phoenix_core::domain::close::CloseRunOrdinal::parse(2).unwrap();
    let mut wrong_scope = success.clone();
    wrong_scope.scope = phoenix_core::work_scope::WorkScopeId::parse("not-captured-scope").unwrap();
    let mut wrong_identity = success.clone();
    wrong_identity.identity = phoenix_core::domain::close::OpaqueIdentity::parse("other-session").unwrap();
    let wrong_snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        "other-generation", "other-fingerprint",
    ).unwrap();

    for (candidate_snapshot, candidate) in [
        (snapshot.clone(), wrong_attempt),
        (snapshot.clone(), wrong_ordinal),
        (snapshot.clone(), wrong_scope),
        (snapshot.clone(), wrong_identity),
        (wrong_snapshot, success),
    ] {
        assert!(matches!(db.record_native_close_initial_process_step_success(
            &command, &receipt, &source, candidate_snapshot, candidate,
        ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
        assert_eq!(initial_process_success_count(&db, &run).await, 0);
    }
}

#[tokio::test]
async fn initial_process_success_rejects_out_of_interval_times_and_stopped_run_replay() {
    let (db, command, receipt, source, run) =
        all_complete_initial_source_fence_fixture_with_omissions(true, None, false).await;
    let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
        .snapshot().cloned().unwrap();
    let scope = db.list_close_attempt_scopes(run.attempt_id.as_str()).await.unwrap()[0]
        .scope.clone();
    let created_at: i64 = sqlx::query_scalar(
        "SELECT created_at_us FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
    ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    for observed_at_us in [created_at - 1, chrono::Utc::now().timestamp_micros() + 60_000_000] {
        let mut invalid_time = initial_bash_process_success(&run, &scope);
        invalid_time.observed_at_us = observed_at_us;
        assert!(matches!(db.record_native_close_initial_process_step_success(
            &command, &receipt, &source, snapshot.clone(), invalid_time,
        ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
        assert_eq!(initial_process_success_count(&db, &run).await, 0);
    }

    let success = initial_bash_process_success(&run, &scope);
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot.clone(), success.clone(),
    ).await, LocalAuthorityResult::DurableFactEstablished(Ok(()))));
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    assert!(matches!(db.record_native_close_initial_process_step_success(
        &command, &receipt, &source, snapshot, success,
    ).await, LocalAuthorityResult::DurableFactEstablished(Err(_))));
    assert_eq!(initial_process_success_count(&db, &run).await, 1);
}

#[tokio::test]
async fn initial_all_complete_source_fence_records_truthful_unsuccessful_outcome() {
    for interruption in [false, true] {
        let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
        let original_obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_close_initial_complete_retirement_proofs WHERE attempt_id = ?1 AND run_ordinal = 1 AND product_conversation_id = ?2")
                .bind(run.attempt_id.as_str()).bind(command.intent().target().as_str())
                .fetch_one(db.pool()).await.unwrap(),
            1,
        );
        if interruption {
            db.reset_all_to_idle().await.unwrap();
        } else {
            write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
        }
        let failure = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        assert_eq!(failure.len(), 1);
        let failure = &failure[0].occurrence;
        assert_eq!(failure.failure_occurrence_id, run.failure_occurrence_id());
        assert_eq!(failure.authority, if interruption {
            CloseCleanupFailureAuthority::AttemptInterrupted
        } else {
            CloseCleanupFailureAuthority::ExplicitlySuppressed
        });
        assert_eq!(failure.reason, RetirementFailureReason::Interrupted);
        assert!(failure.remaining_resources.is_empty());
        assert!(matches!(failure.stop_certainty,
            CloseStopCertainty::ConversationAndProcessesStopped { .. }));
        assert_eq!(source_fence_allowance(&db, &run).await,
            Some((0, if interruption { 0 } else { 1 })));
        let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
        assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::ArchivedCleanupAttention));
        assert_eq!(obligation.snapshot(), original_obligation.snapshot());
        let terminal_source = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
        assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);

        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        let events: Vec<_> = db.pending_coordinator_watch_events(100).await.unwrap()
            .into_iter().map(|event| (event.event_id, event.source_occurrence_id, event.occurred_at_us)).collect();
        let stopped = db.get_close_run(&run).await.unwrap();
        if !interruption {
            write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
        } else {
            db.reset_all_to_idle().await.unwrap();
        }
        assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
        assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap()
            .into_iter().map(|event| (event.event_id, event.source_occurrence_id, event.occurred_at_us)).collect::<Vec<_>>(), events);
        assert_eq!(db.get_close_run(&run).await.unwrap(), stopped);
        assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation);
        assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, terminal_source);

        assert!(db.complete_native_close_initial(&command, &receipt, &source)
            .await.established().unwrap().is_err(), "the interrupted initial finalizer must remain refused");
    }
}

#[tokio::test]
async fn all_complete_initial_source_fence_preserves_custom_empty_inspection_generation() {
    for generation in ["no-worktree", "server_git_status_v2_retry_source_fence_test"] {
        let (db, command, _receipt, _source, run) =
            all_complete_initial_source_fence_fixture_with_omissions(false, Some(generation), false).await;
        let snapshot = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap()
            .snapshot().cloned().unwrap();
        assert_eq!(snapshot.generation(), generation);
        assert_eq!(snapshot.fingerprint(), "no-worktree");
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_close_initial_complete_retirement_proofs WHERE attempt_id = ?1")
            .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), 1);
        write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
        let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
        assert_eq!(obligation.snapshot(), Some(&snapshot));
        assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::ArchivedCleanupAttention));
        let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].occurrence.remaining_resources.is_empty());
        assert!(matches!(failures[0].occurrence.authority, CloseCleanupFailureAuthority::ExplicitlySuppressed { .. }));
    }
}

#[tokio::test]
async fn all_complete_initial_source_fence_retains_successful_bash_process_retirement() {
    let (db, command, _receipt, _source, run) =
        all_complete_initial_source_fence_fixture().await;
    let process_successes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM close_retirement_resources
         WHERE attempt_id = ?1 AND resource_kind = 'bash_process_group'",
    )
    .bind(run.attempt_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(process_successes, 1);

    write_source_fence_state(&db, &command, &ConvState::Idle, false)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM close_retirement_resources
             WHERE attempt_id = ?1 AND resource_kind = 'bash_process_group'",
        )
        .bind(run.attempt_id.as_str())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1,
        "successful process retirement remains a retained fact after fencing",
    );
    assert_eq!(
        db.get_close_obligation(run.attempt_id.as_str())
            .await
            .unwrap()
            .close_outcome(),
        Some(CloseCompletionOutcome::ArchivedCleanupAttention),
    );
}

#[tokio::test]
async fn all_complete_initial_source_fence_missing_process_step_proof_rolls_back() {
    {
        let (db, command, _receipt, source, run) =
            all_complete_initial_source_fence_fixture_with_omissions(true, None, false)
                .await;
        assert!(db.unresolved_expected_close_cleanup_resources(run.attempt_id.as_str()).await.unwrap().is_empty());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_close_initial_complete_retirement_proofs WHERE attempt_id = ?1")
            .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), 0);
        let run_before = db.get_close_run(&run).await.unwrap();
        let obligation_before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
        let failures_before = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
        assert!(failures_before.is_empty());
        assert!(write_source_fence_state(&db, &command, &ConvState::Idle, false)
            .await
            .is_err());
        assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
        assert_eq!(db.get_close_run(&run).await.unwrap(), run_before);
        assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation_before);
        assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
        assert!(db.pending_coordinator_watch_events(100).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn all_complete_initial_source_fence_without_exact_proof_rolls_back() {
    let (db, command, _receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    let successes_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    let insert_failure = "INSERT INTO close_cleanup_failures (
        failure_occurrence_id, attempt_id, cleanup_run_ordinal,
        source_product_conversation_id, authority_kind, reason, detail,
        stop_certainty, confirmed_at_us, occurred_at_us
    ) VALUES (?1, ?2, 1, ?3, 'attempt_interrupted', 'interrupted',
              'process_interrupted: schema proof control', 'conversation_and_processes_stopped', ?4, ?4)";
    let confirmed_at_us = chrono::Utc::now().timestamp_micros();
    // Same insert and timestamps must be admissible with complete proof.
    // Roll it back so only the dispatch omission changes the negative control.
    let mut positive = db.pool().begin().await.unwrap();
    sqlx::query(insert_failure)
        .bind(run.failure_occurrence_id()).bind(run.attempt_id.as_str())
        .bind(command.intent().target().as_str()).bind(confirmed_at_us)
        .execute(&mut *positive).await.unwrap();
    positive.rollback().await.unwrap();
    // Dispatch rows are retained facts, not inferred from successful outcome rows.
    // Removing one must defeat the new positive proof without changing successes.
    let removed = sqlx::query("DELETE FROM close_retirement_resource_dispatches WHERE attempt_id = ?1 AND resource_kind = 'work_scope'")
        .bind(run.attempt_id.as_str()).execute(db.pool()).await.unwrap();
    assert_eq!(removed.rows_affected(), 1);
    assert!(db.unresolved_expected_close_cleanup_resources(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM native_close_initial_complete_retirement_proofs WHERE attempt_id = ?1")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), 0);
    let run_before = db.get_close_run(&run).await.unwrap();
    // The schema exception must also reject a direct parent insert, independently
    // of the source-fenced Rust entry point's proof checks.
    assert!(sqlx::query(insert_failure)
        .bind(run.failure_occurrence_id()).bind(run.attempt_id.as_str())
        .bind(command.intent().target().as_str()).bind(confirmed_at_us)
        .execute(db.pool()).await.is_err());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1")
        .bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), successes_before);

    let obligation_before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    let allowance_before = source_fence_allowance(&db, &run).await;
    assert!(write_source_fence_state(&db, &command, &ConvState::Idle, false).await.is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
    assert_eq!(db.get_close_run(&run).await.unwrap(), run_before);
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation_before);
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert_eq!(source_fence_allowance(&db, &run).await, allowance_before);
    assert!(db.pending_coordinator_watch_events(100).await.unwrap().is_empty());
}

#[tokio::test]
async fn all_complete_initial_source_fence_notification_failure_rolls_back_every_fact() {
    let (db, command, _receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    let messages_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool()).await.unwrap();
    let archived_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM conversations WHERE archived = 1")
        .fetch_one(db.pool()).await.unwrap();
    let lifecycle_before: String = sqlx::query_scalar("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
        .bind(command.intent().target().as_str()).fetch_one(db.pool()).await.unwrap();
    sqlx::raw_sql("CREATE TRIGGER reject_mandatory_close_event BEFORE INSERT ON coordinator_watch_events WHEN NEW.route_kind = 'mandatory_close_failure' BEGIN SELECT RAISE(ABORT, 'injected mandatory notification failure'); END;")
        .execute(db.pool()).await.unwrap();
    let run_before = db.get_close_run(&run).await.unwrap();
    let obligation_before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert!(write_source_fence_state(&db, &command, &ConvState::Idle, false).await.is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
    assert_eq!(db.get_close_run(&run).await.unwrap(), run_before);
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation_before);
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
        .fetch_one(db.pool()).await.unwrap(), messages_before);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM conversations WHERE archived = 1")
        .fetch_one(db.pool()).await.unwrap(), archived_before);
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
        .bind(command.intent().target().as_str()).fetch_one(db.pool()).await.unwrap(), lifecycle_before);
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
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

async fn completion_retry_rows(db: &Database, run: &CloseRunRef) -> (i64, i64, i64, i64) {
    let attempt = run.attempt_id.as_str();
    let admissions = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM native_close_completion_retry_admissions WHERE attempt_id = ?1",
    ).bind(attempt).fetch_one(db.pool()).await.unwrap();
    let runs = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM close_runs WHERE attempt_id = ?1",
    ).bind(attempt).fetch_one(db.pool()).await.unwrap();
    let plans = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM native_close_retry_plans WHERE attempt_id = ?1",
    ).bind(attempt).fetch_one(db.pool()).await.unwrap();
    let effects = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM close_run_retry_effects WHERE attempt_id = ?1",
    ).bind(attempt).fetch_one(db.pool()).await.unwrap();
    (admissions, runs, plans, effects)
}

#[tokio::test]
async fn live_all_success_interruption_admits_exactly_one_effect_free_completion_successor() {
    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    let successes = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1",
    ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert!(successes > 0);
    let interrupted = db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().unwrap();
    assert_eq!(interrupted.close_outcome(), Some(CloseCompletionOutcome::ArchivedCleanupAttention));
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].occurrence.failure_occurrence_id, run.failure_occurrence_id());
    assert_eq!(failures[0].occurrence.authority, CloseCleanupFailureAuthority::AttemptInterrupted);
    assert!(failures[0].occurrence.remaining_resources.is_empty());
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 1);
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_ok());
    assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);

    let crate::native_close::NativeCloseCompletionResult::NewlyAdmitted(completion) =
        db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
            .await.established().unwrap().unwrap() else { panic!("expected new completion admission"); };
    assert_eq!(completion.origin, receipt);
    assert_eq!(completion.successor.attempt_id, run.attempt_id);
    assert_eq!(completion.successor.target, *command.intent().target());
    assert_eq!(completion.successor.run_ordinal.get(), 2);
    assert_eq!(source_fence_allowance(&db, &run).await, Some((1, 0)));
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
    assert_eq!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().unwrap(),
        crate::native_close::NativeCloseCompletionResult::Replay(completion.clone()));
    let successor = CloseRunRef { attempt_id: run.attempt_id.clone(), ordinal: completion.successor.run_ordinal };
    assert_eq!(db.get_close_run(&successor).await.unwrap().status, CloseRunStatus::Running);
    let archived = db.complete_native_close_initial_completion_retry(&command, &completion, &source)
        .await.established().unwrap().unwrap();
    assert_eq!(archived.close_outcome(), Some(CloseCompletionOutcome::Archived));
    assert_eq!(db.get_close_run(&successor).await.unwrap().status, CloseRunStatus::Completed);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let terminal_runs = sqlx::query_as::<_, (i64, String, i64, Option<i64>)>(
        "SELECT run_ordinal, status, created_at_us, ended_at_us FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal",
    ).bind(run.attempt_id.as_str()).fetch_all(db.pool()).await.unwrap();
    assert_eq!(terminal_runs.len(), 2);
    assert!(terminal_runs[1].3.is_some_and(|ended| ended >= terminal_runs[1].2));
    assert_eq!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap(), failures);
    assert_eq!(db.complete_native_close_initial_completion_retry(&command, &completion, &source)
        .await.established().unwrap().unwrap(), archived);
    assert_eq!(sqlx::query_as::<_, (i64, String, i64, Option<i64>)>(
        "SELECT run_ordinal, status, created_at_us, ended_at_us FROM close_runs WHERE attempt_id = ?1 ORDER BY run_ordinal",
    ).bind(run.attempt_id.as_str()).fetch_all(db.pool()).await.unwrap(), terminal_runs);
    assert_eq!(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM messages WHERE message_id = ?1",
    ).bind(format!("close-run-outcome:2:{}", run.attempt_id)).fetch_one(db.pool()).await.unwrap(), 1);
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
    assert_eq!(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1",
    ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), successes);
}

#[tokio::test]
async fn concurrent_completion_admissions_share_one_successor() {
    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_ok());
    let (left, right) = tokio::join!(
        db.admit_native_close_initial_completion_retry(&command, &receipt, &source),
        db.admit_native_close_initial_completion_retry(&command, &receipt, &source),
    );
    use crate::native_close::NativeCloseCompletionResult::{NewlyAdmitted, Replay};
    let left = left.established().unwrap().unwrap();
    let right = right.established().unwrap().unwrap();
    assert!(matches!((&left, &right), (NewlyAdmitted(a), Replay(b)) | (Replay(a), NewlyAdmitted(b)) if a == b));
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
    assert_eq!(source_fence_allowance(&db, &run).await, Some((1, 0)));
}

#[tokio::test]
async fn completion_admission_without_proof_or_with_suppressed_allowance_changes_nothing() {
    let (db, command, receipt, source, run) =
        all_complete_initial_source_fence_fixture_with_omissions(true, None, false).await;
    assert_eq!(completion_retry_rows(&db, &run).await, (0, 1, 0, 0));
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_err());
    assert!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().is_err());
    assert_eq!(completion_retry_rows(&db, &run).await, (0, 1, 0, 0));
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());

    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    let before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
    assert!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().is_err());
    assert_eq!(completion_retry_rows(&db, &run).await, (0, 1, 0, 0));
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 1)));
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), before);
}

#[tokio::test]
async fn stale_source_refuses_completion_replay_and_startup_revokes_handoff() {
    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_ok());
    let crate::native_close::NativeCloseCompletionResult::NewlyAdmitted(completion) =
        db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
            .await.established().unwrap().unwrap() else { panic!("expected new admission"); };
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    assert!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().is_err());
    assert!(db.complete_native_close_initial_completion_retry(&command, &completion, &source)
        .await.established().unwrap().is_err());
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
    write_source_fence_state(&db, &command, &source, false).await.unwrap();
    assert!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().is_err(), "restoring the identical source must not replay a revoked admission");

    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_ok());
    assert!(matches!(
        db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
            .await.established().unwrap().unwrap(),
        crate::native_close::NativeCloseCompletionResult::NewlyAdmitted(_)
    ));
    db.reset_all_to_idle().await.unwrap();
    write_source_fence_state(&db, &command, &source, false).await.unwrap();
    assert!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().is_err(), "startup revocation must survive restoration of the identical source");
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
    assert_eq!(source_fence_allowance(&db, &run).await, Some((1, 0)));
}

#[tokio::test]
async fn idle_source_after_admission_stops_effect_free_successor_with_mandatory_failure() {
    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_ok());
    let crate::native_close::NativeCloseCompletionResult::NewlyAdmitted(completion) =
        db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
            .await.established().unwrap().unwrap() else { panic!("expected new admission"); };
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    let successor = CloseRunRef { attempt_id: run.attempt_id.clone(), ordinal: completion.successor.run_ordinal };
    assert_eq!(db.get_close_run(&successor).await.unwrap().status, CloseRunStatus::Stopped);
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 2);
    let failure = failures.iter().find(|item| item.occurrence.failure_occurrence_id == successor.failure_occurrence_id())
        .expect("successor failure");
    assert!(failure.occurrence.remaining_resources.is_empty());
    assert_eq!(failure.occurrence.authority, CloseCleanupFailureAuthority::ExplicitlySuppressed);
    assert_eq!(db.pending_coordinator_watch_events(100).await.unwrap().len(), 2);
    assert_eq!(source_fence_allowance(&db, &run).await, Some((1, 1)));
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
    assert!(db.complete_native_close_initial_completion_retry(&command, &completion, &source)
        .await.established().unwrap().is_err());
}

#[tokio::test]
async fn uncertain_completion_failure_cannot_rewrite_history_disposition() {
    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_ok());
    assert!(matches!(db.admit_native_close_initial_completion_retry(&command, &receipt, &source)
        .await.established().unwrap().unwrap(),
        crate::native_close::NativeCloseCompletionResult::NewlyAdmitted(_)));
    write_source_fence_state(&db, &command, &ConvState::Idle, false).await.unwrap();
    let before = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(before.close_outcome(), Some(CloseCompletionOutcome::ArchivedCleanupAttention));
    let failure: (String, Option<i64>) = sqlx::query_as(
        "SELECT stop_certainty, confirmed_at_us FROM close_cleanup_failures WHERE attempt_id = ?1 AND cleanup_run_ordinal = 2",
    ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    assert_eq!(failure, ("shutdown_uncertain".to_owned(), None));
    // Prior History is retained without a rewrite. An uncertain successor must not
    // independently authorize even a no-op History/attention disposition write.
    let rejected = sqlx::query(
        "UPDATE close_obligations SET close_outcome = 'archived_cleanup_attention' WHERE attempt_id = ?1",
    ).bind(run.attempt_id.as_str()).execute(db.pool()).await.unwrap_err();
    assert!(rejected.to_string().contains("automatic completion failure requires exact stopped disposition and mandatory outbox"));
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), before);
    assert_eq!(completion_retry_rows(&db, &run).await, (1, 2, 0, 0));
}

#[tokio::test]
async fn live_interruption_notification_failure_rolls_back_without_mutation() {
    let (db, command, receipt, source, run) = all_complete_initial_source_fence_fixture().await;
    let obligation = db.get_close_obligation(run.attempt_id.as_str()).await.unwrap();
    let successes = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1",
    ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap();
    sqlx::raw_sql("CREATE TRIGGER reject_live_completion_notification BEFORE INSERT ON coordinator_watch_events WHEN NEW.route_kind = 'mandatory_close_failure' BEGIN SELECT RAISE(ABORT, 'injected mandatory notification failure'); END;")
        .execute(db.pool()).await.unwrap();
    assert!(db.interrupt_native_close_initial_after_retirement(&command, &receipt, &source)
        .await.established().unwrap().is_err());
    assert_eq!(db.get_conversation(command.source_transcript_id()).await.unwrap().state, source);
    assert_eq!(db.get_close_obligation(run.attempt_id.as_str()).await.unwrap(), obligation);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    assert!(db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap().is_empty());
    assert!(db.pending_coordinator_watch_events(100).await.unwrap().is_empty());
    assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
    assert_eq!(completion_retry_rows(&db, &run).await, (0, 1, 0, 0));
    assert_eq!(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM close_retirement_resources WHERE attempt_id = ?1",
    ).bind(run.attempt_id.as_str()).fetch_one(db.pool()).await.unwrap(), successes);
}
