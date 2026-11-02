use super::*;
use phoenix_core::domain::{
    close::TranscriptConversationId,
    federation_subscription::{
        FederatedTerminalFact, GapReason, SubscriptionCurrentState, SubscriptionGap,
        TerminalFactOccurrence, TerminalFactOccurrenceKind, TerminalFactOutcome,
    },
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
        occurrence: TerminalFactOccurrence::new(
            TerminalFactOccurrenceKind::DirectTurn,
            format!("turn-{sequence}"),
            0,
        )
        .unwrap(),
        source_transcript_id: TranscriptConversationId::parse("exact-transcript").unwrap(),
        outcome: TerminalFactOutcome::Completed,
        occurred_at_us: 1,
    }
}
fn pending_fact(sequence: i64) -> RemoteNotification {
    let req = request();
    RemoteNotification {
        admission_id: format!("federation:{}:fact:{sequence}", req.reference.instance_id),
        reference: req.reference,
        subscription_id: req.subscription_id,
        content: SubscriptionDelivery::Fact {
            fact: fact(sequence),
        },
        disposition: RemoteNotificationDisposition::Pending,
    }
}

fn pending_gap() -> RemoteNotification {
    let mut notification = pending_fact(1);
    notification.admission_id = format!(
        "federation:{}:gap:{}",
        notification.reference.instance_id, notification.subscription_id
    );
    notification.content = SubscriptionDelivery::Gap {
        gap: SubscriptionGap {
            last_acknowledged: SubscriptionCursor::START,
            last_produced: 2.try_into().unwrap(),
            reason: GapReason::DisconnectedAge,
            failed_at_us: 1,
            terminal_evidence_expires_at_us: 1 + TERMINAL_EVIDENCE_LIFETIME_US,
            snapshot: SubscriptionSnapshot {
                transcript_id: TranscriptConversationId::parse("head").unwrap(),
                state: SubscriptionCurrentState::Nonterminal,
            },
        },
    };
    notification
}

#[test]
fn one_pass_selects_only_the_first_unaccepted_staged_fact() {
    let first = pending_fact(1);
    let second = pending_fact(2);
    let selected = next_pending_notification(vec![first.clone(), second.clone()]).unwrap();
    assert_eq!(selected.admission_id, first.admission_id);
    assert_eq!(
        next_pending_notification(vec![second.clone()])
            .unwrap()
            .admission_id,
        second.admission_id
    );

    let mut accepted = first;
    accepted.disposition = RemoteNotificationDisposition::Accepted {
        transcript_id: TranscriptConversationId::parse("coordinator").unwrap(),
    };
    assert_eq!(
        next_pending_notification(vec![accepted, second.clone()])
            .unwrap()
            .admission_id,
        second.admission_id
    );
}

#[test]
fn failed_gap_drains_the_frozen_fact_prefix_before_presenting_the_gap() {
    let first = pending_fact(1);
    let second = pending_fact(2);
    let gap = pending_gap();

    let selected =
        next_pending_notification(vec![first.clone(), second.clone(), gap.clone()]).unwrap();
    assert_eq!(selected.admission_id, first.admission_id);

    let selected = next_pending_notification(vec![second.clone(), gap.clone()]).unwrap();
    assert_eq!(selected.admission_id, second.admission_id);

    let selected = next_pending_notification(vec![gap.clone()]).unwrap();
    assert_eq!(selected.admission_id, gap.admission_id);
}

#[test]
fn a_durable_gap_at_the_current_cursor_remains_eligible_for_delivery() {
    let mut notification = pending_fact(1);
    notification.content = SubscriptionDelivery::Gap {
        gap: SubscriptionGap {
            last_acknowledged: SubscriptionCursor::START,
            last_produced: 1.try_into().unwrap(),
            reason: GapReason::DisconnectedAge,
            failed_at_us: 1,
            terminal_evidence_expires_at_us: 1 + TERMINAL_EVIDENCE_LIFETIME_US,
            snapshot: SubscriptionSnapshot {
                transcript_id: TranscriptConversationId::parse("head").unwrap(),
                state: SubscriptionCurrentState::Nonterminal,
            },
        },
    };
    assert!(next_pending_notification(vec![notification]).is_some());
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
    let mut invalid = fact(1);
    invalid.occurred_at_us = -1;
    assert!(validate_response(&req, &response(&req, vec![invalid])).is_err());
    let mut invalid = fact(1);
    invalid.source_transcript_id = TranscriptConversationId::parse("x".repeat(1025)).unwrap();
    assert!(validate_response(&req, &response(&req, vec![invalid])).is_err());
    let mut invalid = fact(1);
    invalid.occurrence.id.clear();
    assert!(validate_response(&req, &response(&req, vec![invalid])).is_err());
    let mut invalid = fact(1);
    invalid.occurrence.id = "x".repeat(1025);
    assert!(validate_response(&req, &response(&req, vec![invalid])).is_err());
    let mut invalid = fact(1);
    invalid.occurrence.generation = i64::MAX as u64 + 1;
    assert!(validate_response(&req, &response(&req, vec![invalid])).is_err());
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
fn gap_requires_loss_after_the_exact_acknowledged_cursor() {
    let req = request();
    let mut good = response(&req, vec![]);
    good.result = SubscriptionResult::Replay {
        replay: SubscriptionReplay::Gap {
            gap: SubscriptionGap {
                last_acknowledged: SubscriptionCursor::START,
                last_produced: 1.try_into().unwrap(),
                reason: GapReason::EventCount,
                failed_at_us: 1,
                terminal_evidence_expires_at_us: 1 + TERMINAL_EVIDENCE_LIFETIME_US,
                snapshot: SubscriptionSnapshot {
                    transcript_id: TranscriptConversationId::parse("current-head").unwrap(),
                    state: SubscriptionCurrentState::Nonterminal,
                },
            },
        },
    };
    validate_response(&req, &good).unwrap();

    let mut progressed = req.clone();
    progressed.operation = SubscriptionOperation::Replay {
        after: 1.try_into().unwrap(),
    };
    validate_response(&progressed, &good).unwrap();
    progressed.operation = SubscriptionOperation::Replay {
        after: 2.try_into().unwrap(),
    };
    assert!(validate_response(&progressed, &good).is_err());

    let mut bad = good.clone();
    let SubscriptionResult::Replay {
        replay: SubscriptionReplay::Gap { gap },
    } = &mut bad.result
    else {
        unreachable!();
    };
    gap.last_produced = SubscriptionCursor::START;
    assert!(validate_response(&req, &bad).is_err());

    let mut bad = good.clone();
    let SubscriptionResult::Replay {
        replay: SubscriptionReplay::Gap { gap },
    } = &mut bad.result
    else {
        unreachable!();
    };
    gap.last_acknowledged = 1.try_into().unwrap();
    assert!(validate_response(&req, &bad).is_err());

    let mut bad = good.clone();
    let SubscriptionResult::Replay {
        replay: SubscriptionReplay::Gap { gap },
    } = &mut bad.result
    else {
        unreachable!();
    };
    gap.terminal_evidence_expires_at_us += 1;
    assert!(validate_response(&req, &bad).is_err());

    let mut bad = good;
    let SubscriptionResult::Replay {
        replay: SubscriptionReplay::Gap { gap },
    } = &mut bad.result
    else {
        unreachable!();
    };
    gap.failed_at_us = -1;
    assert!(validate_response(&req, &bad).is_err());
}

#[test]
fn authorization_response_validates_delivery_bounds_and_navigation() {
    let mut req = request();
    req.operation = SubscriptionOperation::Authorize {
        delivery: SubscriptionDelivery::Fact { fact: fact(1) },
    };
    let mut good = response(&req, vec![]);
    good.result = SubscriptionResult::Authorization {
        authorization: SubscriptionAuthorization::Authorized {
            authorization_id: Uuid::new_v4(),
        },
        navigation: Some(SubscriptionNavigation {
            stable_path: "/c/source".into(),
            source_path: "/c/exact-transcript?source_transcript=exact-transcript".into(),
        }),
    };
    validate_response(&req, &good).unwrap();

    let mut bad = good.clone();
    let SubscriptionResult::Authorization {
        navigation: Some(navigation),
        ..
    } = &mut bad.result
    else {
        unreachable!();
    };
    navigation.stable_path = "https://source.invalid/c/source".into();
    assert!(validate_response(&req, &bad).is_err());

    let mut bad = good.clone();
    let SubscriptionResult::Authorization {
        navigation: Some(navigation),
        ..
    } = &mut bad.result
    else {
        unreachable!();
    };
    navigation.source_path = "/c/other?source_transcript=other".into();
    assert!(validate_response(&req, &bad).is_err());

    let mut bad = good.clone();
    bad.result = SubscriptionResult::Authorization {
        authorization: SubscriptionAuthorization::Refresh,
        navigation: Some(SubscriptionNavigation {
            stable_path: "/c/source".into(),
            source_path: "/c/exact-transcript?source_transcript=exact-transcript".into(),
        }),
    };
    assert!(validate_response(&req, &bad).is_err());

    if let SubscriptionOperation::Authorize {
        delivery: SubscriptionDelivery::Fact { fact },
    } = &mut req.operation
    {
        fact.occurred_at_us = -1;
    }
    assert!(validate_response(&req, &good).is_err());
    if let SubscriptionOperation::Authorize {
        delivery: SubscriptionDelivery::Fact { fact },
    } = &mut req.operation
    {
        fact.occurred_at_us = 1;
        fact.source_transcript_id = TranscriptConversationId::parse("x".repeat(1025)).unwrap();
    }
    assert!(validate_response(&req, &good).is_err());

    {
        let SubscriptionOperation::Authorize { delivery } = &mut req.operation else {
            unreachable!();
        };
        *delivery = SubscriptionDelivery::Gap {
            gap: SubscriptionGap {
                last_acknowledged: SubscriptionCursor::START,
                last_produced: 1.try_into().unwrap(),
                reason: GapReason::DisconnectedAge,
                failed_at_us: 1,
                terminal_evidence_expires_at_us: 1 + TERMINAL_EVIDENCE_LIFETIME_US,
                snapshot: SubscriptionSnapshot {
                    transcript_id: TranscriptConversationId::parse("head").unwrap(),
                    state: SubscriptionCurrentState::Nonterminal,
                },
            },
        };
    }
    let mut authorized_gap = good.clone();
    let SubscriptionResult::Authorization {
        navigation: Some(navigation),
        ..
    } = &mut authorized_gap.result
    else {
        unreachable!();
    };
    navigation.source_path = "/c/head?source_transcript=head".into();
    validate_response(&req, &authorized_gap).unwrap();

    let SubscriptionOperation::Authorize { delivery } = &mut req.operation else {
        unreachable!();
    };
    *delivery = SubscriptionDelivery::Gap {
        gap: SubscriptionGap {
            last_acknowledged: SubscriptionCursor::START,
            last_produced: SubscriptionCursor::START,
            reason: GapReason::DisconnectedAge,
            failed_at_us: 1,
            terminal_evidence_expires_at_us: 1 + TERMINAL_EVIDENCE_LIFETIME_US,
            snapshot: SubscriptionSnapshot {
                transcript_id: TranscriptConversationId::parse("head").unwrap(),
                state: SubscriptionCurrentState::Nonterminal,
            },
        },
    };
    assert!(validate_response(&req, &good).is_err());
}

#[test]
fn evidence_expired_requires_a_fact_and_cannot_supply_navigation() {
    let mut req = request();
    req.operation = SubscriptionOperation::Authorize {
        delivery: SubscriptionDelivery::Fact { fact: fact(1) },
    };
    let mut reply = response(&req, vec![]);
    reply.result = SubscriptionResult::Authorization {
        authorization: SubscriptionAuthorization::EvidenceExpired,
        navigation: None,
    };
    validate_response(&req, &reply).unwrap();

    if let SubscriptionResult::Authorization { navigation, .. } = &mut reply.result {
        *navigation = Some(SubscriptionNavigation {
            stable_path: "/c/source".into(),
            source_path: "/c/exact-transcript?source_transcript=exact-transcript".into(),
        });
    }
    assert!(validate_response(&req, &reply).is_err());
    if let SubscriptionResult::Authorization { navigation, .. } = &mut reply.result {
        *navigation = None;
    }

    let SubscriptionOperation::Authorize { delivery } = &mut req.operation else {
        unreachable!();
    };
    *delivery = pending_gap().content;
    assert!(validate_response(&req, &reply).is_err());
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
