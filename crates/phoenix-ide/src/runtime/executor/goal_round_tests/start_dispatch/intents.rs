use super::*;

// The checkpoint primitive is exercised directly here; these are admission tests,
// not proof that the production response owner commits this checkpoint atomically.
async fn checkpoint_latest(
    db: &Database,
    receipt: &GoalStartReceipt,
    cause: GoalRequestCause,
) -> GoalRequestId {
    let (id, generation): (String, i64) = sqlx::query_as(
        "SELECT request_id,generation FROM conversation_goal_requests ORDER BY ordinal DESC LIMIT 1",
    )
    .fetch_one(db.pool()).await.unwrap();
    let intent = GoalRequestIntent::new(
        GoalRequestId::parse(id).unwrap(),
        receipt.input().goal().clone(),
        receipt.execution(),
        generation,
        cause,
    )
    .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    let claim_generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM durable_turns WHERE turn_id=? AND workflow_id=?
         AND disposition='Runtime' AND terminal_kind IS NULL AND owns_conversation=1",
    )
    .bind(receipt.execution().turn_id())
    .bind(receipt.execution().workflow_id())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    phoenix_db::conversation_goals::execution::checkpoint_request_tx(
        &mut tx,
        db.goal_execution_installation(),
        &intent,
        claim_generation,
        &text("test-owned-response-checkpoint"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    intent.id
}

async fn normal_state(db: &Database, rt: &mut TestRuntime, attempt: u32) {
    rt.state = ConvState::LlmRequesting { attempt };
    db.update_conversation_state(CONVERSATION, &rt.state)
        .await
        .unwrap();
}

async fn assert_normal_refused(
    db: &Database,
    rt: &mut TestRuntime,
    client: &RenderingAnthropicClient,
    reason: &str,
) {
    let before = ledger(db).await;
    let captures = client.requests.lock().unwrap().len();
    let error = rt.execute_effect(Effect::RequestLlm).await.unwrap_err();
    assert!(error.contains(reason), "{error}");
    assert!(rt.llm_task_handle.is_none());
    assert_eq!(client.requests.lock().unwrap().len(), captures);
    assert_eq!(ledger(db).await, before);
}

#[tokio::test]
async fn conversation_goal_real_normal_successor_requires_durable_checkpoint_and_retries_once_charged(
) {
    let (_dir, db, receipt, mut rt, client) = adopted().await;
    let initial = request_id(&db).await;
    dispatch(&mut rt).await;
    normal_state(&db, &mut rt, 2).await;
    dispatch(&mut rt).await;
    assert_eq!(ledger(&db).await, (1, 0, 1, 0));
    assert!(client.requests.lock().unwrap().iter().all(|request| request
        .telemetry
        .as_ref()
        .unwrap()
        .request_id
        == initial));

    let predecessor = checkpoint_latest(&db, &receipt, GoalRequestCause::Initial).await;
    assert_eq!(predecessor.as_str(), initial);
    assert_normal_refused(&db, &mut rt, &client, "state advanced").await;
    db.update_conversation_state(CONVERSATION, &ConvState::LlmRequesting { attempt: 1 })
        .await
        .unwrap();
    // The stale runtime still owns attempt 2, not the newly persisted attempt 1.
    assert_normal_refused(&db, &mut rt, &client, "state advanced").await;
    let now = Utc::now();
    rt.state = ConvState::ServerOverloadRetrying {
        retry: phoenix_core::domain::sm_state::ServerOverloadRetry {
            target: phoenix_core::domain::sm_state::ServerOverloadTarget::Ordinary,
            phase: phoenix_core::domain::sm_state::ServerOverloadPhase::InFlight,
            attempt: 2,
            started_at: now,
            deadline_at: now + chrono::Duration::seconds(60),
            logical_request_id: initial.clone(),
            model_id: rt.context.model_id.clone(),
        },
    };
    db.update_conversation_state(CONVERSATION, &rt.state)
        .await
        .unwrap();
    assert_normal_refused(&db, &mut rt, &client, "state advanced").await;
    normal_state(&db, &mut rt, 1).await;
    dispatch(&mut rt).await;
    let successor: String = sqlx::query_scalar(
        "SELECT request_id FROM conversation_goal_requests WHERE cause='after_checkpoint'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_ne!(successor, initial);
    assert_eq!(
        client
            .requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .telemetry
            .as_ref()
            .unwrap()
            .request_id,
        successor
    );
    assert_eq!(ledger(&db).await, (2, 1, 1, 0));
    normal_state(&db, &mut rt, 2).await;
    dispatch(&mut rt).await;
    assert_eq!(
        client
            .requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .telemetry
            .as_ref()
            .unwrap()
            .request_id,
        successor
    );
    assert_eq!(ledger(&db).await, (2, 1, 1, 0));
    checkpoint_latest(
        &db,
        &receipt,
        GoalRequestCause::AfterCheckpoint { predecessor },
    )
    .await;
    normal_state(&db, &mut rt, 1).await;
    let before = ledger(&db).await;
    let captures = client.requests.lock().unwrap().len();
    let error = rt.execute_effect(Effect::RequestLlm).await.unwrap_err();
    assert!(error.contains("goal request limit is exhausted"), "{error}");
    assert!(rt.llm_task_handle.is_none());
    assert_eq!(client.requests.lock().unwrap().len(), captures);
    assert_eq!(ledger(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_concurrent_same_summary_operation_commits_one_charge_and_one_identity() {
    let (_dir, db, _receipt, mut rt, client) = adopted().await;
    let operation = "same:\"operation:[with]/delimiters";
    summary_state(&db, &mut rt, operation, 1).await;
    let active = rt.active_direct_turn.as_deref().unwrap();
    let repository = db.workflow_repository();
    let left = repository.admit_goal_start_request_context(
        CONVERSATION,
        Some((active.turn_id, active.generation)),
        GoalRequestOperation::Summary {
            operation: text(operation),
        },
    );
    let right = repository.admit_goal_start_request_context(
        CONVERSATION,
        Some((active.turn_id, active.generation)),
        GoalRequestOperation::Summary {
            operation: text(operation),
        },
    );
    let (left, right) = tokio::join!(left, right);
    assert_eq!(
        left.unwrap().unwrap().request_id,
        right.unwrap().unwrap().request_id
    );
    assert_eq!(ledger(&db).await, (2, 0, 1, 0));
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn conversation_goal_pending_summary_never_authorizes_normal_or_another_summary() {
    let (_dir, db, _receipt, mut rt, client) = adopted().await;
    let request = summary_state(&db, &mut rt, "first:operation", 1).await;
    send_summary(&mut rt, request).await;
    let before = ledger(&db).await;
    let captures = client.requests.lock().unwrap().len();
    normal_state(&db, &mut rt, 1).await;
    let error = rt.execute_effect(Effect::RequestLlm).await.unwrap_err();
    assert!(
        error.contains("pending Summary cannot authorize Normal"),
        "{error}"
    );
    let request = summary_state(&db, &mut rt, "first:operation:distinct", 1).await;
    let error = rt
        .execute_effect(Effect::RequestContinuation { request })
        .await
        .unwrap_err();
    assert!(error.contains("different pending Summary"), "{error}");
    assert!(rt.llm_task_handle.is_none());
    assert_eq!(client.requests.lock().unwrap().len(), captures);
    assert_eq!(ledger(&db).await, before);
}

#[tokio::test]
async fn conversation_goal_summary_requires_exact_persisted_operation_and_capacity() {
    let (_dir, db, receipt, mut rt, client) = adopted().await;
    summary_state(&db, &mut rt, "persisted:operation", 1).await;
    let stale = phoenix_core::domain::sm_state::ContinuationSummaryRequest {
        operation_id: "different:operation".into(),
        rejected_tool_calls: vec![],
        attempt: 1,
    };
    rt.state = ConvState::AwaitingContinuation {
        request: stale.clone(),
    };
    let before = ledger(&db).await;
    let error = rt
        .execute_effect(Effect::RequestContinuation { request: stale })
        .await
        .unwrap_err();
    assert!(error.contains("state advanced"), "{error}");
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(ledger(&db).await, before);
    let request = summary_state(&db, &mut rt, "persisted:operation", 1).await;
    send_summary(&mut rt, request).await;
    checkpoint_latest(
        &db,
        &receipt,
        GoalRequestCause::Summary {
            operation: text("persisted:operation"),
        },
    )
    .await;
    let request = summary_state(&db, &mut rt, "different:operation", 1).await;
    let before = ledger(&db).await;
    let captures = client.requests.lock().unwrap().len();
    let error = rt
        .execute_effect(Effect::RequestContinuation { request })
        .await
        .unwrap_err();
    assert!(error.contains("goal request limit is exhausted"), "{error}");
    assert!(rt.llm_task_handle.is_none());
    assert_eq!(client.requests.lock().unwrap().len(), captures);
    assert_eq!(ledger(&db).await, before);
}

#[derive(Debug, Clone, Copy)]
enum SummaryFence {
    Stop,
    ControlAba,
    Queue,
}

#[tokio::test]
async fn conversation_goal_new_summary_charge_rechecks_live_fences_before_provider() {
    for fence in [
        SummaryFence::Stop,
        SummaryFence::ControlAba,
        SummaryFence::Queue,
    ] {
        let (_dir, db, receipt, mut rt, client) = adopted().await;
        let request = summary_state(&db, &mut rt, "fenced:summary", 1).await;
        match fence {
            SummaryFence::Stop | SummaryFence::ControlAba => {
                change_control(&db, &receipt.input().goal().owner, ControlCommand::Stop, 1).await;
                if matches!(fence, SummaryFence::ControlAba) {
                    change_control(
                        &db,
                        &receipt.input().goal().owner,
                        ControlCommand::Release,
                        2,
                    )
                    .await;
                }
            }
            SummaryFence::Queue => {
                db.append_steering_entry(
                    CONVERSATION,
                    &phoenix_core::domain::sm_event::SteerEntry {
                        text: "accepted before Summary".into(),
                        origin: InputOrigin::UnknownHistorical,
                        llm_text: None,
                        images: vec![],
                        files: vec![],
                        message_id: "summary-queue".into(),
                        user_agent: None,
                        skill_invocation: None,
                    },
                    "summary-queue-fingerprint",
                )
                .await
                .unwrap();
            }
        }
        let before = ledger(&db).await;
        let error = rt
            .execute_effect(Effect::RequestContinuation { request })
            .await
            .unwrap_err();
        assert!(!error.is_empty(), "{fence:?}");
        assert!(client.requests.lock().unwrap().is_empty(), "{fence:?}");
        assert!(rt.llm_task_handle.is_none(), "{fence:?}");
        assert_eq!(ledger(&db).await, before, "{fence:?}");
    }
}

#[tokio::test]
async fn conversation_goal_older_summary_cannot_borrow_later_normal_or_summary_charge() {
    for later_summary in [false, true] {
        let (_dir, db, receipt, mut rt, client) = adopted_with_limit(4).await;
        let request = summary_state(&db, &mut rt, "old:summary", 1).await;
        send_summary(&mut rt, request).await;
        let predecessor = checkpoint_latest(
            &db,
            &receipt,
            GoalRequestCause::Summary {
                operation: text("old:summary"),
            },
        )
        .await;
        normal_state(&db, &mut rt, 1).await;
        dispatch(&mut rt).await;
        if later_summary {
            checkpoint_latest(
                &db,
                &receipt,
                GoalRequestCause::AfterCheckpoint { predecessor },
            )
            .await;
            let request = summary_state(&db, &mut rt, "new:summary", 1).await;
            send_summary(&mut rt, request).await;
        }
        let before = ledger(&db).await;
        let captures = client.requests.lock().unwrap().len();
        let request = summary_state(&db, &mut rt, "old:summary", 2).await;
        let error = rt
            .execute_effect(Effect::RequestContinuation { request })
            .await
            .unwrap_err();
        assert!(
            error.contains(if later_summary {
                "different pending Summary"
            } else {
                "already checkpointed"
            }),
            "{error}"
        );
        assert!(rt.llm_task_handle.is_none());
        assert_eq!(client.requests.lock().unwrap().len(), captures);
        assert_eq!(ledger(&db).await, before);
    }
}
