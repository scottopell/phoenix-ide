#[path = "slots/reconstruction_owner.rs"]
mod reconstruction_owner;

use super::*;

async fn commit_same_state_new_slot(db: &Database, rt: &TestRuntime) -> chrono::DateTime<Utc> {
    let stamp = rt.state_updated_at + chrono::Duration::microseconds(1);
    db.update_conversation_state_at(CONVERSATION, &rt.state, stamp)
        .await
        .unwrap();
    let stored = db.get_conversation(CONVERSATION).await.unwrap();
    assert_eq!(stored.state, rt.state);
    assert_eq!(stored.state_updated_at, stamp);
    assert_ne!(stored.state_updated_at, rt.state_updated_at);
    stamp
}

async fn assert_receipt_matches_slot(
    db: &Database,
    context: &phoenix_db::workflow::GoalStartRequestContext,
    active: &crate::runtime::traits::ActiveDirectTurn,
    stamp: chrono::DateTime<Utc>,
) {
    let receipt: &phoenix_db::workflow::AdmittedGoalRequest = context.admitted();
    let (id, intent_generation): (String, i64) = sqlx::query_as(
        "SELECT request_id,generation FROM conversation_goal_requests WHERE request_id=?",
    )
    .bind(context.request_id().as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let claim_generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM durable_turns WHERE turn_id=? AND workflow_id=?",
    )
    .bind(receipt.intent().execution.turn_id())
    .bind(receipt.intent().execution.workflow_id())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(receipt.intent().id.as_str(), id);
    assert_eq!(receipt.intent().generation(), intent_generation);
    assert_eq!(receipt.claim_generation(), claim_generation);
    assert_eq!(claim_generation, i64::try_from(active.generation).unwrap());
    assert_eq!(receipt.state_updated_at(), stamp);
    assert_eq!(context.purpose(), receipt.intent().cause.purpose());
}

#[tokio::test]
async fn conversation_goal_same_normal_attempt_stale_slot_cannot_claim_successor_or_retry() {
    for attempt in [1, 2] {
        let (directory, db, receipt, mut stale, stale_client) = adopted().await;
        if attempt == 1 {
            checkpoint_latest(&db, &receipt, GoalRequestCause::Initial).await;
        }
        normal_state(&db, &mut stale, attempt).await;
        let before = ledger(&db).await;
        let stamp = commit_same_state_new_slot(&db, &stale).await;
        assert_normal_refused(&db, &mut stale, &stale_client, "slot timestamp differs").await;
        assert_eq!(ledger(&db).await, before);
        let active = stale.active_direct_turn.as_deref().unwrap().clone();
        let repository = db.workflow_repository();
        let context = repository
            .admit_goal_start_request_context(
                CONVERSATION,
                Some((active.turn_id, active.generation)),
                stamp,
                GoalRequestOperation::Normal { attempt },
            )
            .await
            .unwrap()
            .unwrap();
        assert_receipt_matches_slot(&db, &context, &active, stamp).await;
        assert_eq!(
            context.purpose(),
            phoenix_core::domain::conversation_goal::GoalRequestPurpose::Normal
        );
        let client = Arc::new(RenderingAnthropicClient::new());
        let mut fresh = runtime(
            db.clone(),
            directory.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(false)),
        )
        .with_active_direct_turn(Some(active))
        .with_state_updated_at(stamp);
        fresh.state = stale.state.clone();
        dispatch(&mut fresh).await;
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert_eq!(
            client.requests.lock().unwrap()[0]
                .telemetry
                .as_ref()
                .unwrap()
                .request_id,
            context.request_id().as_str()
        );
        assert_eq!(
            ledger(&db).await,
            (
                if attempt == 1 { 2 } else { 1 },
                i64::from(attempt == 1),
                1,
                0
            )
        );
        dispatch(&mut fresh).await;
        assert_eq!(ledger(&db).await.0, if attempt == 1 { 2 } else { 1 });
    }
}

#[tokio::test]
async fn conversation_goal_same_summary_operation_and_attempt_stale_slot_cannot_charge_or_send() {
    for attempt in [1, 2] {
        let (directory, db, _receipt, mut stale, stale_client) = adopted().await;
        let request = summary_state(&db, &mut stale, "same:summary:operation", attempt).await;
        let before = ledger(&db).await;
        let stamp = commit_same_state_new_slot(&db, &stale).await;
        let error = stale
            .execute_effect(Effect::RequestContinuation {
                request: request.clone(),
            })
            .await
            .unwrap_err();
        assert!(error.contains("slot timestamp differs"), "{error}");
        assert!(stale.llm_task_handle.is_none());
        assert!(stale_client.requests.lock().unwrap().is_empty());
        assert_eq!(ledger(&db).await, before);
        let active = stale.active_direct_turn.as_deref().unwrap().clone();
        let context = db
            .workflow_repository()
            .admit_goal_start_request_context(
                CONVERSATION,
                Some((active.turn_id, active.generation)),
                stamp,
                GoalRequestOperation::Summary {
                    operation: text(&request.operation_id),
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_receipt_matches_slot(&db, &context, &active, stamp).await;
        assert!(matches!(&context.admitted().intent().cause,
            GoalRequestCause::Summary { operation } if operation.as_str() == request.operation_id));
        let client = Arc::new(RenderingAnthropicClient::new());
        let mut fresh = runtime(
            db.clone(),
            directory.path(),
            client.clone(),
            Arc::new(DeferredMcpCatalog::new(false)),
        )
        .with_active_direct_turn(Some(active))
        .with_state_updated_at(stamp);
        fresh.state = stale.state.clone();
        send_summary(&mut fresh, request.clone()).await;
        send_summary(&mut fresh, request).await;
        {
            let captures = client.requests.lock().unwrap();
            assert_eq!(captures.len(), 2);
            assert!(captures.iter().all(|captured| captured
                .telemetry
                .as_ref()
                .unwrap()
                .request_id
                == context.request_id().as_str()));
        }

        assert_eq!(ledger(&db).await, (2, 0, 1, 0));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_admission_receipt_distinguishes_retained_intent_from_reconstructed_generation(
) {
    let (directory, db, receipt, mut rt, client) = adopted().await;
    let active = rt.active_direct_turn.as_deref().unwrap().clone();
    let before = ledger(&db).await;
    let claim_rows: Vec<(i64, String, i64, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT e.effect_id,e.status,a.attempt_id,a.status,a.generation,l.lease_until
         FROM workflow_effects e JOIN workflow_attempts a
           ON a.workflow_id=e.workflow_id AND a.effect_id=e.effect_id
         LEFT JOIN workflow_reclaimable_leases l
           ON l.workflow_id=a.workflow_id AND l.attempt_id=a.attempt_id
         WHERE e.workflow_id=? AND e.kind='deliver_runtime_turn'
         ORDER BY e.effect_id,a.ordinal",
    )
    .bind(receipt.execution().workflow_id())
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert!(!claim_rows.is_empty());
    assert!(
        claim_rows
            .iter()
            .all(|row| row.1 == "Receipted" && row.3 == "ReceiptAccepted" && row.5.is_none()),
        "{claim_rows:?}"
    );
    let now = u64::try_from(Utc::now().timestamp()).unwrap();
    let result = db
        .workflow_repository()
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id: active.turn_id,
            workflow_id: WorkflowId(u64::try_from(receipt.execution().workflow_id()).unwrap()),
            process_incarnation: ProcessIncarnation(2),
            now: Timestamp(now),
            lease_until: LeaseExpiry(now + 3600),
        })
        .await
        .unwrap();
    assert_eq!(
        result.outcome,
        phoenix_workflow::ClaimOutcome::Ineligible,
        "adopted delivery is not a reclaimable lease: {result:?}; {claim_rows:?}"
    );
    let recovery_input = phoenix_db::workflow::GoalStartRecoveryInput::new(
        CONVERSATION.to_owned(),
        (active.turn_id, active.generation),
        (rt.state.clone(), rt.state_updated_at),
        Timestamp(now),
    );
    let recovery = db
        .workflow_repository()
        .recover_goal_start_runtime(&recovery_input)
        .await
        .unwrap();
    let phoenix_db::workflow::GoalStartRecoveryOutcome::Recovered(recovered) = recovery else {
        panic!("actual reconstruction must establish ownership: {recovery:?}");
    };
    assert_eq!(recovered.turn_id(), active.turn_id);
    assert!(recovered.generation() > active.generation);
    let context = db
        .workflow_repository()
        .admit_goal_start_request_context(
            CONVERSATION,
            Some((active.turn_id, recovered.generation())),
            rt.state_updated_at,
            GoalRequestOperation::Normal { attempt: 1 },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        context.admitted().intent().generation(),
        i64::try_from(active.generation).unwrap()
    );
    assert_eq!(
        context.admitted().claim_generation(),
        i64::try_from(recovered.generation()).unwrap()
    );
    assert_ne!(
        context.admitted().intent().generation(),
        context.admitted().claim_generation()
    );
    assert_eq!(context.admitted().state_updated_at(), rt.state_updated_at);
    let stale = db
        .workflow_repository()
        .admit_goal_start_request_context(
            CONVERSATION,
            Some((active.turn_id, active.generation)),
            rt.state_updated_at,
            GoalRequestOperation::Normal { attempt: 1 },
        )
        .await
        .unwrap_err();
    assert!(
        stale.to_string().contains("no live materialized execution"),
        "{stale}"
    );
    assert_eq!(ledger(&db).await, before);
    assert_normal_refused(&db, &mut rt, &client, "no live materialized execution").await;
    assert!(client.requests.lock().unwrap().is_empty());
    let rendered = Arc::new(RenderingAnthropicClient::new());
    let mut reconstructed = runtime(
        db.clone(),
        directory.path(),
        rendered.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    )
    .with_active_direct_turn(Some(crate::runtime::traits::ActiveDirectTurn {
        turn_id: recovered.turn_id(),
        generation: recovered.generation(),
    }))
    .with_state_updated_at(rt.state_updated_at);
    reconstructed.state = rt.state.clone();
    dispatch(&mut reconstructed).await;
    assert_eq!(ledger(&db).await, before);
    assert_eq!(rendered.requests.lock().unwrap().len(), 1);
    assert_eq!(
        rendered.requests.lock().unwrap()[0]
            .telemetry
            .as_ref()
            .unwrap()
            .request_id,
        context.request_id().as_str()
    );
}
