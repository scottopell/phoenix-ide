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

async fn assert_initial_captured_scope_interruption(db: &Database, run: &CloseRunRef) {
    let failures = db.list_close_cleanup_failures(run.attempt_id.as_str()).await.unwrap();
    assert_eq!(failures.len(), 1);
    let failure = &failures[0].occurrence;
    let CloseCleanupFailureAuthority::CapturedScope { scope, resource } = &failure.authority else {
        panic!("initial fixture retains a captured scope: {:?}", failure.authority);
    };
    assert_eq!(resource, &opaque_resource(RetiredResourceKind::WorkScope, scope.as_str()));
    assert!(failure.remaining_resources.contains(&CloseCleanupFailureResource {
        scope: scope.clone(),
        resource: resource.clone(),
        disposition: CloseCleanupResourceDisposition::Failed,
    }));
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
        db.classify_interrupted_close_run(&run).await.unwrap();
        assert_initial_captured_scope_interruption(&db, &run).await;
        let state = db.get_conversation(command.source_transcript_id()).await.unwrap().state;
        write_source_fence_state(&db, &command, &state, replay).await.unwrap();
        assert_eq!(source_fence_allowance(&db, &run).await, Some((0, 0)));
        assert_stopped_source_loss_only_suppresses(&db, &command, &run, replay, 1).await;
    }
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
                    db.classify_interrupted_close_run(&run).await.unwrap();
                    assert_initial_captured_scope_interruption(&db, &run).await;
                }
                set_startup_allowance_bits(&db, &run, consumed, suppressed).await;
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
