// Current member work is independent of the retained original retirement proof.
// These tests strengthen existing explicit retry safety, not captured-loss admission.
async fn install_retry_member_active_turn(db: &Database) {
    let captured: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM close_attempt_participants
         WHERE attempt_id = 'attempt-failure' AND conversation_id = 'participant')",
    ).fetch_one(db.pool()).await.unwrap();
    assert!(captured);
    // Deliberately install durable state below ordinary admission: an already
    // admitted close fences that API. This models independently appearing work,
    // with a real workflow FK, Runtime ownership, nonterminal state and valid
    // user_api provenance (no forged terminal/evidence rows or disabled triggers).
    let mut tx = db.pool().begin().await.unwrap();
    let workflow_id: i64 = sqlx::query_scalar(
        "INSERT INTO workflows (profile_kind, profile_version,
             runtime_acceptance_enabled, external_acceptance_enabled, version, generation,
             status, snapshot_codec_family, snapshot_codec_version, snapshot_payload,
             created_at, updated_at)
         VALUES ('direct_turn', 1, 0, 1, 1, 0, 'Active', 'direct_turn.snapshot', 1, X'01', 1, 1)
         RETURNING workflow_id",
    ).fetch_one(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO durable_turns (workflow_id, conversation_id, client_turn_key,
             prepared_fingerprint, prepared_payload, disposition, generation,
             owns_conversation, origin_kind, canonical_message_id, materialized)
         VALUES (?1, 'participant', 'retry-new-member-work', 'retry-member-fingerprint',
             X'01', 'Runtime', 0, 1, 'user_api', NULL, 0)",
    ).bind(workflow_id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

fn retry_member_work_refused<T: std::fmt::Debug>(result: LocalAuthorityResult<DbResult<T>>) {
    let LocalAuthorityResult::DurableFactEstablished(
        Err(DbError::CloseFoundationPrecondition(message)),
    ) = result else {
        panic!("expected captured member quiescence precondition: {result:?}");
    };
    assert!(message.contains("active durable captured member obligation"), "{message}");
}

#[tokio::test]
async fn native_retry_new_admission_requires_current_captured_member_quiescence() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "member-work-admission").await;
    let original = db.read_stopped_close_original_retirement_proof(
        &assessment.request.failed_run,
    ).await.unwrap();
    install_retry_member_active_turn(&db).await;
    // Historical capture/restoration is still exactly the same proof; it is not
    // positive evidence that newly appearing work has shut down.
    assert_eq!(db.read_stopped_close_original_retirement_proof(
        &assessment.request.failed_run,
    ).await.unwrap(), original);
    let before = retry_dispatch_state(&db, &failure.attempt_id).await;

    retry_member_work_refused(db.apply_native_close_explicit_retry(&assessment).await);

    assert_no_admission(&db, &failure.attempt_id).await;
    assert_eq!(retry_dispatch_state(&db, &failure.attempt_id).await, before);
    assert_eq!(db.read_stopped_close_original_retirement_proof(
        &assessment.request.failed_run,
    ).await.unwrap(), original);
}

#[tokio::test]
async fn native_retry_exact_receipt_replay_ignores_new_member_work_without_effects() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "member-work-replay").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    install_retry_member_active_turn(&db).await;
    let before = retry_dispatch_state(&db, &failure.attempt_id).await;

    assert_eq!(db.read_native_close_explicit_retry_receipt(&assessment.command).await.unwrap(),
        Some(receipt.admitted.clone()));
    let result = db.apply_native_close_explicit_retry(&assessment).await;
    assert!(matches!(&result,
        LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::Replay(replay)))
            if replay.admitted == receipt.admitted
    ), "{result:?}");

    assert_eq!(retry_dispatch_state(&db, &failure.attempt_id).await, before);
    let run = CloseRunRef {
        attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal,
    };
    assert!(db.close_safe_retry_progress(&run).await.unwrap().successful.is_empty());
}

#[tokio::test]
async fn native_retry_live_execution_rechecks_current_captured_member_quiescence() {
    for boundary in ["dispatch", "success", "finalization"] {
        let db = Database::open_in_memory().await.unwrap();
        let failure = fixture(&db, Vec::new(), true).await;
        let assessment = assess(&db, &failure, boundary).await;
        let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
        let run = CloseRunRef {
            attempt_id: failure.attempt_id.clone(), ordinal: receipt.admitted.run_ordinal,
        };
        if boundary == "finalization" {
            for effect in &assessment.request.remaining_effects {
                success_recorded(db.record_native_close_retry_success(
                    &assessment.command, &receipt.admitted, effect, "retired before new member work",
                ).await);
            }
            assert!(db.close_safe_retry_progress(&run).await.unwrap().pending.is_empty());
        }
        install_retry_member_active_turn(&db).await;
        let before = retry_dispatch_state(&db, &failure.attempt_id).await;
        let obligation = db.get_close_obligation(failure.attempt_id.as_str()).await.unwrap();
        let effect = assessment.request.remaining_effects.iter()
            .find(|effect| effect.resource.kind() == RetiredResourceKind::Worktree).unwrap();

        match boundary {
            "dispatch" => retry_member_work_refused(db.record_native_close_retry_retirement_dispatch(
                &assessment.command, &receipt.admitted, effect,
            ).await),
            "success" => retry_member_work_refused(db.record_native_close_retry_success(
                &assessment.command, &receipt.admitted, effect, "must not record while member runs",
            ).await),
            "finalization" => retry_member_work_refused(db.complete_native_close_retry(
                &assessment.command, &receipt.admitted,
            ).await),
            _ => unreachable!(),
        }

        assert_eq!(retry_dispatch_state(&db, &failure.attempt_id).await, before, "{boundary}");
        assert_eq!(db.get_close_obligation(failure.attempt_id.as_str()).await.unwrap(), obligation);
        assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);
    }
}

#[tokio::test]
async fn native_retry_member_work_allows_uncertain_failure_but_not_stopped_placement() {
    let db = Database::open_in_memory().await.unwrap();
    let failure = fixture(&db, Vec::new(), true).await;
    let assessment = assess(&db, &failure, "member-work-failure").await;
    let receipt = admitted(db.apply_native_close_explicit_retry(&assessment).await);
    let run = CloseRunRef {
        attempt_id: failure.attempt_id.clone(),
        ordinal: receipt.admitted.run_ordinal,
    };
    let pending = db.close_safe_retry_progress(&run).await.unwrap().pending;
    let index = pending.iter().position(|effect| {
        effect.resource.kind() == RetiredResourceKind::Worktree
    }).unwrap();
    let request = native_resource_failure_request(&assessment, &receipt, &pending, index);
    install_retry_member_active_turn(&db).await;
    let before = retry_dispatch_state(&db, &failure.attempt_id).await;
    let mut stopped = request.clone();
    stopped.stop_certainty = CloseStopCertainty::ConversationAndProcessesStopped {
        confirmed_at_us: stopped.occurred_at_us,
    };

    retry_member_work_refused(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, stopped,
    ).await);
    assert_eq!(retry_dispatch_state(&db, &failure.attempt_id).await, before);
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Running);

    let obligation = retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request.clone(),
    ).await);
    assert_eq!(obligation.close_outcome(), Some(CloseCompletionOutcome::CloseIncomplete));
    assert_eq!(db.get_close_run(&run).await.unwrap().status, CloseRunStatus::Stopped);
    let retained = retry_dispatch_state(&db, &failure.attempt_id).await;
    assert_eq!(retry_completed(db.terminalize_native_close_retry_resource_failure(
        &assessment.command, &receipt.admitted, request,
    ).await), obligation);
    assert_eq!(retry_dispatch_state(&db, &failure.attempt_id).await, retained);
}
