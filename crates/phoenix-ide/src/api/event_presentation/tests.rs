use super::*;
use phoenix_db::{MessageContent, MessageType};

fn message(id: &str) -> Message {
    Message {
        origin: InputOrigin::SubscriptionEvent {
            event_id: id.into(),
        },
        message_id: format!("message-{id}"),
        conversation_id: "recipient".into(),
        sequence_id: 1,
        message_type: MessageType::User,
        content: MessageContent::user("Original event\n  bytes: λ <>&\n"),
        display_data: None,
        usage_data: None,
        created_at: chrono::Utc::now(),
    }
}

pub(crate) async fn fixture(count: usize) -> (Database, Vec<Message>) {
    let db = Database::open_in_memory().await.unwrap();
    let source = db
        .create_conversation("source", "source", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    let watch: i64 = sqlx::query_scalar(
        "SELECT id FROM coordinator_watches WHERE source_product_conversation_id = ?",
    )
    .bind(source.product_conversation_id.as_str())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let mut messages = Vec::new();
    for n in 0..count {
        let id = format!("event-{n}");
        let state = ["pending", "accepted", "suppressed"][n % 3];
        sqlx::query("INSERT INTO coordinator_watch_events (event_id, watch_id, source_occurrence_kind, source_occurrence_id, source_generation, source_transcript_id, terminal_kind, occurred_at_us, delivery_state, accepted_transcript_id) VALUES (?, ?, 'creation', ?, 0, ?, 'completed', 123456789, ?, ?)")
            .bind(&id).bind(watch).bind(&id).bind(&source.id).bind(state)
            .bind((state == "accepted").then_some(&source.id)).execute(db.pool()).await.unwrap();
        messages.push(message(&id));
    }
    db.unwatch_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?")
        .bind(source.product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
    (db, messages)
}

#[tokio::test]
async fn caller_projects_every_event_across_400_boundary_and_historical_delivery_states() {
    let (db, messages) = fixture(401).await;
    for count in [400, 401] {
        let enriched = enrich_messages(&db, &messages[..count]).await;
        assert_eq!(enriched.len(), count);
        for (original, wire) in messages.iter().zip(enriched) {
            assert_eq!(
                wire.content,
                serde_json::to_value(&original.content).unwrap()
            );
            let Some(EventPresentation::Available(event)) = wire.event_presentation else {
                panic!("event omitted")
            };
            assert!(matches!(event.outcome, EventOutcome::TurnFinished));
            assert_eq!(event.occurred_at_us, 123456789);
        }
    }
}

#[tokio::test]
async fn missing_lookup_failed_and_shared_projections_keep_original_message() {
    let (db, mut messages) = fixture(1).await;
    messages.push(message("missing"));
    let missing = enrich_messages(&db, &messages).await;
    assert_eq!(missing.len(), 2);
    assert!(matches!(
        missing[1].event_presentation,
        Some(EventPresentation::Unavailable {
            reason: EventPresentationUnavailableReason::Missing,
            ..
        })
    ));
    let original = serde_json::to_value(&messages[0].content).unwrap();
    let shared = EventPresentations::redacted().enrich(&messages[0]);
    assert_eq!(shared.content, original);
    assert!(matches!(
        shared.event_presentation,
        Some(EventPresentation::Unavailable {
            reason: EventPresentationUnavailableReason::Redacted,
            ..
        })
    ));
    let shared_json = serde_json::to_value(shared).unwrap();
    assert!(shared_json["event_presentation"]
        .get("display_name")
        .is_none());
    db.pool().close().await;
    let failed = enrich_messages(&db, &messages).await;
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].content, original);
    assert!(matches!(
        failed[0].event_presentation,
        Some(EventPresentation::Unavailable {
            reason: EventPresentationUnavailableReason::LookupFailed,
            ..
        })
    ));
}

#[test]
fn ordinary_message_serializes_explicit_null_and_all_outcomes_preserve_reasons() {
    let mut ordinary = message("not-event");
    ordinary.origin = InputOrigin::UserApi;
    let wire = serde_json::to_value(EventPresentations::redacted().enrich(&ordinary)).unwrap();
    assert_eq!(
        wire.get("event_presentation"),
        Some(&serde_json::Value::Null)
    );
    for (outcome, expected) in [
        (WatchOutcome::Completed, "turn_finished"),
        (
            WatchOutcome::Failed {
                reason: "exact\nreason".into(),
            },
            "failed",
        ),
        (
            WatchOutcome::CleanupFailed {
                reason: "exact\nreason".into(),
            },
            "cleanup_failed",
        ),
        (WatchOutcome::Cancelled, "cancelled"),
        (WatchOutcome::AwaitingUserResponse, "awaiting_user_response"),
        (WatchOutcome::AwaitingTaskApproval, "awaiting_task_approval"),
    ] {
        let event = WatchEventPresentation {
            event_id: "event".into(),
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::parse("product")
                    .unwrap(),
            display_name: None,
            source_transcript_id: "source".into(),
            source_occurrence_kind: "creation".into(),
            source_occurrence_id: "occurrence".into(),
            source_generation: 0,
            outcome,
            occurred_at_us: 1,
            provenance: WatchEventProvenance::MandatoryCloseFailure,
        };
        let value = serde_json::to_value(EventPresentationPayload::from(event)).unwrap();
        assert_eq!(value["outcome"]["kind"], expected);
        if matches!(expected, "failed" | "cleanup_failed") {
            assert_eq!(value["outcome"]["reason"], "exact\nreason");
        }
        assert_eq!(value["display_name"], serde_json::Value::Null);
        assert_eq!(value["provenance"], "mandatory_close_failure");
    }
}
