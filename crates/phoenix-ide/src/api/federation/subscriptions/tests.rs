use super::*;
use phoenix_core::domain::close::TranscriptConversationId;
use phoenix_core::domain::federation_subscription::{
    FederatedTerminalFact, SubscriptionCurrentState, SubscriptionGap, TerminalFactOutcome,
};

fn request() -> SubscriptionRequest {
    SubscriptionRequest {
        caller_instance_id: InstanceId::new(),
        reference: SubscriptionReference {
            instance_id: InstanceId::new(),
            product_conversation_id: ProductConversationId::parse("source").unwrap(),
        },
        subscription_id: Uuid::new_v4(),
        operation: SubscriptionOperation::Replay {
            after: SubscriptionCursor::START,
        },
    }
}
fn response(
    request: &SubscriptionRequest,
    events: Vec<FederatedTerminalFact>,
) -> SubscriptionResponse {
    SubscriptionResponse {
        caller_instance_id: request.caller_instance_id,
        reference: request.reference.clone(),
        subscription_id: request.subscription_id,
        result: SubscriptionResult::Replay {
            replay: SubscriptionReplay::Facts { events },
        },
    }
}
fn fact(sequence: i64) -> FederatedTerminalFact {
    FederatedTerminalFact {
        event_id: Uuid::new_v4(),
        sequence: sequence.try_into().unwrap(),
        source_transcript_id: TranscriptConversationId::parse("exact-transcript").unwrap(),
        outcome: TerminalFactOutcome::Completed,
        occurred_at_us: 1,
    }
}
#[test]
fn response_identity_order_payload_and_budget_are_closed() {
    let req = request();
    let good = response(&req, vec![fact(1), fact(2)]);
    validate_response(&req, &good).unwrap();
    let mut bad = good.clone();
    bad.caller_instance_id = InstanceId::new();
    assert!(validate_response(&req, &bad).is_err());
    let mut bad = good.clone();
    bad.reference.instance_id = InstanceId::new();
    assert!(validate_response(&req, &bad).is_err());
    let mut bad = good.clone();
    bad.reference.product_conversation_id = ProductConversationId::parse("different").unwrap();
    assert!(validate_response(&req, &bad).is_err());
    let mut bad = good.clone();
    bad.subscription_id = Uuid::new_v4();
    assert!(validate_response(&req, &bad).is_err());
    assert!(validate_response(&req, &response(&req, vec![fact(2), fact(1)])).is_err());
    assert!(validate_response(&req, &response(&req, vec![fact(1), fact(3)])).is_err());
    let first = fact(1);
    let mut duplicate = first.clone();
    duplicate.sequence = 2.try_into().unwrap();
    assert!(validate_response(&req, &response(&req, vec![first, duplicate])).is_err());
    assert!(validate_response(&req, &response(&req, (1..=17).map(fact).collect())).is_err());
    assert!(append_bounded_response_chunk(
        &mut Vec::new(),
        &vec![0; RESPONSE_BYTES + 1],
        RESPONSE_BYTES
    )
    .is_err());
}
#[test]
fn request_and_replay_deny_unknown_fields_and_disallowed_terminal_categories() {
    let req = request();
    let mut value = serde_json::to_value(&req).unwrap();
    value["callback"] = serde_json::json!("https://not-supported");
    assert!(serde_json::from_value::<SubscriptionRequest>(value).is_err());
    let mut value = serde_json::to_value(&req).unwrap();
    value["operation"]["after"] = serde_json::json!(-1);
    assert!(serde_json::from_value::<SubscriptionRequest>(value).is_err());
    for kind in [
        "awaiting_user_response",
        "awaiting_task_approval",
        "cleanup_failed",
        "token",
    ] {
        let mut value = serde_json::to_value(fact(1)).unwrap();
        value["outcome"] = serde_json::json!(kind);
        assert!(serde_json::from_value::<FederatedTerminalFact>(value).is_err());
    }
}
#[test]
fn gap_requires_forward_resume_and_exact_requested_cursor() {
    let req = request();
    let mut response = response(&req, vec![]);
    response.result = SubscriptionResult::Replay {
        replay: SubscriptionReplay::Gap {
            gap: SubscriptionGap {
                requested_after: SubscriptionCursor::START,
                resume_after: SubscriptionCursor::START,
                snapshot: SubscriptionSnapshot {
                    transcript_id: TranscriptConversationId::parse("current-head").unwrap(),
                    state: SubscriptionCurrentState::Nonterminal,
                },
            },
        },
    };
    assert!(validate_response(&req, &response).is_err());
}

async fn held_sync_task() -> (SubscriptionSync, tokio::sync::oneshot::Receiver<()>) {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let sync = SubscriptionSync {
        pending: Some(tokio::spawn(async move {
            let _held = held_tx;
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        })),
    };
    started_rx.await.unwrap();
    (sync, held_rx)
}

#[tokio::test]
async fn dropping_sync_owner_aborts_in_flight_work_instead_of_detaching_it() {
    let (sync, held) = held_sync_task().await;
    drop(sync);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), held)
            .await
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn closed_runtime_authority_aborts_retained_work_and_does_not_launch_a_new_task() {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let (mut sync, held) = held_sync_task().await;
    state.runtime.begin_process_shutdown();
    sync.sync_pass(&state.runtime).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), held)
            .await
            .unwrap()
            .is_err()
    );
    assert!(sync.pending.is_none());
    sync.sync_pass(&state.runtime).await;
    assert!(sync.pending.is_none());
}
