use super::*;
use crate::workflow::AcceptAuthoritativeTurn;
use phoenix_core::domain::{
    db_schema::InputOrigin,
    instance_identity::{FederationCredentialVerifier, PeerBearerCredential},
    sm_event::{
        PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SubmittedDirectTurnExpansionPolicy,
        SubmittedDirectTurnIdentity,
    },
};
use phoenix_workflow::{
    AcceptedDisposition, ClientTurnKey, ConversationAuthority, PreparedTurn, Timestamp, TurnOutcome,
};

const NOW: i64 = MAX_DISCONNECTED_AGE_US * 2;

fn send_authority(
    caller: InstanceId,
    verifier: FederationCredentialVerifier,
) -> FederationSendAuthority {
    FederationSendAuthority::new(caller, verifier)
}

async fn active_enrollment_id(db: &Database, caller: InstanceId) -> Uuid {
    let id: String = sqlx::query_scalar(
        "SELECT id FROM federation_enrollments
         WHERE caller_instance_id = ?1 AND revoked_at_us IS NULL",
    )
    .bind(caller.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    Uuid::parse_str(&id).unwrap()
}

async fn open_migrated(path: &str) -> Database {
    let db = Database::open(path).await.unwrap();
    crate::migrations::run_pending_migrations(db.pool())
        .await
        .unwrap();
    db
}
struct Fixture {
    db: Database,
    source: crate::Conversation,
    caller: InstanceId,
    enrollment: Uuid,
    authority: FederationSendAuthority,
    subscription: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let source = db
            .create_conversation("federated-source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
        let caller = InstanceId::new();
        let verifier = FederationCredentialVerifier::from_bearer(
            PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43)))
                .unwrap()
                .expose()
                .as_bytes(),
        );
        db.replace_federation_enrollment(caller, "caller", &verifier)
            .await
            .unwrap();
        let enrollment = active_enrollment_id(&db, caller).await;
        let authority = send_authority(caller, verifier);
        let subscription = Uuid::new_v4();
        db.create_federation_subscription(
            &authority,
            subscription,
            &source.product_conversation_id,
            NOW,
        )
        .await
        .unwrap();
        Self {
            db,
            source,
            caller,
            enrollment,
            authority,
            subscription,
        }
    }
    async fn emit(&self, occurrence: &str, outcome: TerminalFactOutcome, now: i64) {
        self.emit_generation(occurrence, 0, outcome, now).await;
    }

    async fn emit_generation(
        &self,
        occurrence: &str,
        generation: i64,
        outcome: TerminalFactOutcome,
        now: i64,
    ) {
        let mut tx = self.db.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        append_terminal_tx(
            &mut tx,
            ClassifiedTerminal {
                occurrence_kind: "creation",
                occurrence_id: occurrence,
                generation,
                transcript_id: &self.source.id,
                outcome,
                context_exhausted: false,
                occurred_at_us: now,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    async fn replay(&self, after: i64, now: i64) -> SubscriptionReplay {
        self.db
            .replay_federation_subscription(
                &self.authority,
                self.subscription,
                &self.source.product_conversation_id,
                cursor(after).unwrap(),
                now,
            )
            .await
            .unwrap()
    }
}
fn facts(replay: SubscriptionReplay) -> Vec<FederatedTerminalFact> {
    match replay {
        SubscriptionReplay::Facts { events } => events,
        other @ (SubscriptionReplay::Gap { .. } | SubscriptionReplay::Ended { .. }) => {
            panic!("facts required: {other:?}")
        }
    }
}

fn occurrence(id: &str) -> TerminalFactOccurrence {
    TerminalFactOccurrence::new(TerminalFactOccurrenceKind::Creation, id, 0).unwrap()
}

#[tokio::test]
async fn reused_occurrence_id_is_joined_by_kind_id_and_generation() {
    let f = Fixture::new().await;
    f.emit_generation("reused-occurrence", 0, TerminalFactOutcome::Completed, NOW)
        .await;
    f.emit_generation("reused-occurrence", 1, TerminalFactOutcome::Failed, NOW + 1)
        .await;

    let replayed = facts(f.replay(0, NOW + 1).await);
    assert_eq!(replayed.len(), 2);
    assert_ne!(replayed[0].event_id, replayed[1].event_id);
    assert_eq!(
        replayed[0].occurrence,
        TerminalFactOccurrence::new(TerminalFactOccurrenceKind::Creation, "reused-occurrence", 0,)
            .unwrap()
    );
    assert_eq!(replayed[0].outcome, TerminalFactOutcome::Completed);
    assert_eq!(replayed[0].occurred_at_us, NOW);
    assert_eq!(
        replayed[1].occurrence,
        TerminalFactOccurrence::new(TerminalFactOccurrenceKind::Creation, "reused-occurrence", 1,)
            .unwrap()
    );
    assert_eq!(replayed[1].outcome, TerminalFactOutcome::Failed);
    assert_eq!(replayed[1].occurred_at_us, NOW + 1);

    assert!(matches!(
        authorize_source(
            &f,
            &SubscriptionDelivery::Fact {
                fact: replayed[0].clone(),
            },
        )
        .await,
        SubscriptionAuthorization::Authorized { .. }
    ));

    let mut forged = replayed[0].clone();
    forged.occurrence.generation = replayed[1].occurrence.generation;
    let forged_result =
        f.db.authorize_federation_delivery(
            &f.authority,
            f.subscription,
            &f.source.product_conversation_id,
            &SubscriptionDelivery::Fact { fact: forged },
            NOW + 1,
        )
        .await;
    assert!(
        forged_result.is_err(),
        "authorized fact accepted another occurrence generation: {forged_result:?}"
    );

    assert!(matches!(
        authorize_source(
            &f,
            &SubscriptionDelivery::Fact {
                fact: replayed[1].clone(),
            },
        )
        .await,
        SubscriptionAuthorization::Authorized { .. }
    ));
}

#[tokio::test]
async fn future_only_exact_identity_replay_ack_and_compact_dedupe() {
    let mut f = Fixture::new().await;
    f.emit("before", TerminalFactOutcome::Completed, NOW).await;
    let first = facts(f.replay(0, NOW).await);
    assert_eq!(first.len(), 1);
    assert_eq!(first, facts(f.replay(0, NOW).await));
    assert_eq!(first[0].source_transcript_id.as_str(), f.source.id);
    assert!(matches!(
        authorize_source(
            &f,
            &SubscriptionDelivery::Fact {
                fact: first[0].clone(),
            },
        )
        .await,
        SubscriptionAuthorization::Authorized { .. }
    ));
    assert!(facts(f.replay(1, NOW).await).is_empty());
    f.emit("before", TerminalFactOutcome::Completed, NOW + 1)
        .await;
    assert!(facts(f.replay(1, NOW + 1).await).is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_identities")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1
    );
    let release_reason: Option<String> = sqlx::query_scalar(
        "SELECT evidence_release_reason FROM federation_terminal_identities LIMIT 1",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(release_reason.as_deref(), Some("settled"));
    f.db.unsubscribe_federation_subscription(
        &f.authority,
        f.subscription,
        &f.source.product_conversation_id,
    )
    .await
    .unwrap();
    f.subscription = Uuid::new_v4();
    f.db.create_federation_subscription(
        &f.authority,
        f.subscription,
        &f.source.product_conversation_id,
        NOW + 2,
    )
    .await
    .unwrap();
    f.emit("before", TerminalFactOutcome::Completed, NOW + 2)
        .await;
    assert!(facts(f.replay(0, NOW + 2).await).is_empty());
    f.emit("after", TerminalFactOutcome::Failed, NOW + 3).await;
    assert_eq!(
        facts(f.replay(0, NOW + 3).await)[0].outcome,
        TerminalFactOutcome::Failed
    );
}

#[tokio::test]
async fn payload_1025_terminally_fails_with_a_durable_count_gap() {
    let f = Fixture::new().await;
    for n in 0..1025 {
        f.emit(
            &format!("occurrence-{n}"),
            TerminalFactOutcome::Completed,
            NOW + n,
        )
        .await;
    }
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + 1025).await else {
        panic!("retention gap required")
    };
    assert_eq!(gap.last_acknowledged, SubscriptionCursor::START);
    assert_eq!(gap.last_produced.get(), 1025);
    assert_eq!(gap.reason, GapReason::EventCount);
    assert_eq!(gap.failed_at_us, NOW + 1024);
    assert_eq!(gap.snapshot.transcript_id.as_str(), f.source.id);
    let SubscriptionReplay::Gap { gap: retried } = f.replay(0, NOW + 1026).await else {
        panic!("terminal gap must be retried durably")
    };
    assert_eq!(retried, gap);
    let replayed_after_prefix_progress =
        f.db.replay_federation_subscription(
            &f.authority,
            f.subscription,
            &f.source.product_conversation_id,
            cursor(1).unwrap(),
            NOW + 1026,
        )
        .await
        .unwrap();
    assert_eq!(
        replayed_after_prefix_progress,
        SubscriptionReplay::Gap { gap: gap.clone() }
    );
    assert!(f
        .db
        .replay_federation_subscription(
            &f.authority,
            f.subscription,
            &f.source.product_conversation_id,
            cursor(1026).unwrap(),
            NOW + 1026,
        )
        .await
        .is_err());
    let replacement = Uuid::new_v4();
    f.db.create_federation_subscription(
        &f.authority,
        replacement,
        &f.source.product_conversation_id,
        NOW + 1027,
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_terminal_identities WHERE subscription_id = ?1 AND evidence_release_reason = 'subscription_replaced'",
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        1025
    );
    assert!(matches!(
        f.db.replay_federation_subscription(
            &f.authority,
            replacement,
            &f.source.product_conversation_id,
            SubscriptionCursor::START,
            NOW + 1027,
        )
        .await
        .unwrap(),
        SubscriptionReplay::Facts { events } if events.is_empty()
    ));
}

#[tokio::test]
async fn disconnected_age_terminally_fails_at_exact_destination_limit() {
    let f = Fixture::new().await;
    f.emit("expires", TerminalFactOutcome::Cancelled, NOW).await;
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + MAX_DISCONNECTED_AGE_US).await else {
        panic!("expired history must not replay")
    };
    assert_eq!(gap.last_acknowledged, SubscriptionCursor::START);
    assert_eq!(gap.last_produced.get(), 1);
    assert_eq!(gap.reason, GapReason::DisconnectedAge);
    assert_eq!(
        gap.snapshot.state,
        SubscriptionCurrentState::Terminal {
            outcome: TerminalFactOutcome::Cancelled
        }
    );
    assert!(
        matches!(f.replay(0, NOW + MAX_DISCONNECTED_AGE_US + 1).await, SubscriptionReplay::Gap { gap: retry } if retry == gap)
    );
}

#[tokio::test]
async fn disconnected_age_without_unacknowledged_data_remains_active() {
    let f = Fixture::new().await;

    assert!(matches!(
        f.replay(0, NOW + MAX_DISCONNECTED_AGE_US).await,
        SubscriptionReplay::Facts { events } if events.is_empty()
    ));
    assert!(matches!(
        f.replay(0, NOW + MAX_DISCONNECTED_AGE_US * 2).await,
        SubscriptionReplay::Facts { events } if events.is_empty()
    ));
    let (state, acknowledged, produced): (String, i64, i64) = sqlx::query_as(
        "SELECT state, acknowledged_through, last_sequence FROM federation_subscriptions WHERE id = ?1",
    )
    .bind(f.subscription.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(state, "active");
    assert_eq!(acknowledged, 0);
    assert_eq!(produced, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_subscription_gaps WHERE subscription_id = ?1",
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn disconnected_age_is_enforced_before_reconnect_acknowledgement() {
    let f = Fixture::new().await;
    f.emit("expires-before-ack", TerminalFactOutcome::Completed, NOW)
        .await;
    let event = facts(f.replay(0, NOW).await).remove(0);
    assert!(matches!(
        authorize_source(&f, &SubscriptionDelivery::Fact { fact: event }).await,
        SubscriptionAuthorization::Authorized { .. }
    ));

    let SubscriptionReplay::Gap { gap } =
        f.db.replay_federation_subscription(
            &f.authority,
            f.subscription,
            &f.source.product_conversation_id,
            1.try_into().unwrap(),
            NOW + MAX_DISCONNECTED_AGE_US,
        )
        .await
        .unwrap()
    else {
        panic!("the destination prefix cursor must retrieve the durable gap")
    };

    let SubscriptionReplay::Gap { gap: retry } =
        f.replay(0, NOW + MAX_DISCONNECTED_AGE_US + 1).await
    else {
        panic!("the unchanged source cursor must replay the durable gap")
    };
    assert_eq!(gap.last_acknowledged, SubscriptionCursor::START);
    assert_eq!(gap.last_produced.get(), 1);
    assert_eq!(gap.reason, GapReason::DisconnectedAge);
    let acknowledged: i64 = sqlx::query_scalar(
        "SELECT acknowledged_through FROM federation_subscriptions WHERE id = ?1",
    )
    .bind(f.subscription.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(acknowledged, 0);
    assert_eq!(retry, gap);
}

#[tokio::test]
async fn payload_byte_limit_terminally_fails_without_advancing_acknowledgement() {
    let f = Fixture::new().await;
    f.emit("oversize", TerminalFactOutcome::Completed, NOW)
        .await;
    sqlx::query("UPDATE federation_terminal_payloads SET payload_bytes = ?1")
        .bind(i64::try_from(MAX_UNACKNOWLEDGED_PAYLOAD_BYTES).unwrap() + 1)
        .execute(f.db.pool())
        .await
        .unwrap();
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW).await else {
        panic!("payload byte gap required")
    };
    assert_eq!(gap.reason, GapReason::PayloadBytes);
    assert_eq!(gap.last_acknowledged, SubscriptionCursor::START);
    assert_eq!(gap.last_produced.get(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT acknowledged_through FROM federation_subscriptions WHERE id = ?1",
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn source_rejects_cursor_jump_without_exact_next_fact_authorization() {
    let f = Fixture::new().await;
    f.emit("one", TerminalFactOutcome::Completed, NOW).await;
    f.emit("two", TerminalFactOutcome::Completed, NOW + 1).await;
    assert!(f
        .db
        .replay_federation_subscription(
            &f.authority,
            f.subscription,
            &f.source.product_conversation_id,
            cursor(2).unwrap(),
            NOW + 1,
        )
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT acknowledged_through FROM federation_subscriptions WHERE id = ?1",
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn absent_create_unsubscribe_is_idempotent_and_fences_only_the_exact_identity() {
    let f = Fixture::new().await;
    let source =
        f.db.create_conversation("absent-create-source", "source", "/tmp", true, None, None)
            .await
            .unwrap();
    let absent = Uuid::new_v4();
    assert!(f
        .db
        .unsubscribe_federation_subscription(
            &send_authority(
                InstanceId::new(),
                FederationCredentialVerifier::from_bearer(b"wrong-caller"),
            ),
            absent,
            &source.product_conversation_id,
        )
        .await
        .is_err());
    for _ in 0..2 {
        f.db.unsubscribe_federation_subscription(
            &f.authority,
            absent,
            &source.product_conversation_id,
        )
        .await
        .unwrap();
    }
    assert!(f
        .db
        .unsubscribe_federation_subscription(
            &f.authority,
            absent,
            &ProductConversationId::parse("different-source").unwrap(),
        )
        .await
        .is_err());
    assert!(f
        .db
        .create_federation_subscription(&f.authority, absent, &source.product_conversation_id, NOW,)
        .await
        .is_err());
    let binding: (String, String) = sqlx::query_as(
        "SELECT enrollment_id, product_id FROM federation_subscription_create_fences WHERE id = ?1",
    )
    .bind(absent.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(
        binding,
        (
            f.enrollment.to_string(),
            source.product_conversation_id.to_string()
        )
    );
    let created: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM federation_subscriptions WHERE id = ?1)")
            .bind(absent.to_string())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert!(!created);
    f.emit(
        "other-identity-still-active",
        TerminalFactOutcome::Completed,
        NOW,
    )
    .await;
    assert_eq!(facts(f.replay(0, NOW).await).len(), 1);
}

#[tokio::test]
async fn exact_enrollment_owns_subscription_and_replacement_ends_old_binding_atomically() {
    let f = Fixture::new().await;
    f.emit("unadmitted", TerminalFactOutcome::Failed, NOW).await;
    assert!(f
        .db
        .replay_federation_subscription(
            &send_authority(
                InstanceId::new(),
                FederationCredentialVerifier::from_bearer(b"wrong-caller"),
            ),
            f.subscription,
            &f.source.product_conversation_id,
            SubscriptionCursor::START,
            NOW
        )
        .await
        .is_err());
    assert!(f
        .db
        .unsubscribe_federation_subscription(
            &send_authority(
                f.caller,
                FederationCredentialVerifier::from_bearer(b"wrong-verifier"),
            ),
            f.subscription,
            &f.source.product_conversation_id
        )
        .await
        .is_err());
    let verifier = FederationCredentialVerifier::from_bearer(
        PeerBearerCredential::parse(format!("phx_peer_{}", "b".repeat(43)))
            .unwrap()
            .expose()
            .as_bytes(),
    );
    f.db.replace_federation_enrollment(f.caller, "replacement", &verifier)
        .await
        .unwrap();
    assert_ne!(active_enrollment_id(&f.db, f.caller).await, f.enrollment);
    let state: String = sqlx::query_scalar("SELECT state FROM federation_subscriptions")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "revoked");
    let release_reason: Option<String> = sqlx::query_scalar(
        "SELECT evidence_release_reason FROM federation_terminal_identities LIMIT 1",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(release_reason.as_deref(), Some("trust_revoked"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
    assert!(f
        .db
        .replay_federation_subscription(
            &send_authority(f.caller, verifier),
            f.subscription,
            &f.source.product_conversation_id,
            SubscriptionCursor::START,
            NOW
        )
        .await
        .is_err());
}

#[tokio::test]
async fn source_close_ends_subscription_and_suppresses_unadmitted_payloads() {
    let f = Fixture::new().await;
    f.emit("pending", TerminalFactOutcome::Completed, NOW).await;
    sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1")
        .bind(f.source.product_conversation_id.as_str())
        .execute(f.db.pool())
        .await
        .unwrap();
    assert_eq!(
        f.replay(0, NOW).await,
        SubscriptionReplay::Ended {
            reason: SubscriptionEnd::SourceClosed
        }
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
    let release_reason: Option<String> = sqlx::query_scalar(
        "SELECT evidence_release_reason FROM federation_terminal_identities LIMIT 1",
    )
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(release_reason.as_deref(), Some("source_closed"));
    assert!(f
        .db
        .create_federation_subscription(
            &f.authority,
            Uuid::new_v4(),
            &f.source.product_conversation_id,
            NOW
        )
        .await
        .is_err());
}

#[tokio::test]
async fn classification_fanout_consumes_cancellation_once_and_drops_nonterminal_categories() {
    let f = Fixture::new().await;
    f.db.watch_product_conversation(&f.source.product_conversation_id)
        .await
        .unwrap();
    sqlx::query("INSERT INTO execution_cancel_observations(conversation_id) VALUES (?1)")
        .bind(&f.source.id)
        .execute(f.db.pool())
        .await
        .unwrap();
    let mut tx = f.db.pool.begin().await.unwrap();
    crate::coordinator_watches::record_creation_event_tx(
        &mut tx,
        "cancelled-source",
        0,
        &f.source.id,
        "Completed",
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let remote = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
    assert_eq!(remote[0].outcome, TerminalFactOutcome::Cancelled);
    assert_eq!(
        f.db.pending_coordinator_watch_events(16).await.unwrap()[0].outcome,
        crate::WatchOutcome::Cancelled
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution_cancel_observations")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
    let mut tx = f.db.pool.begin().await.unwrap();
    for category in [
        "AwaitingUserResponse",
        "AwaitingTaskApproval",
        "CleanupFailed",
        "Token",
    ] {
        crate::coordinator_watches::record_creation_event_tx(
            &mut tx,
            category,
            0,
            &f.source.id,
            category,
            None,
        )
        .await
        .unwrap();
    }
    crate::coordinator_watches::record_terminal_event_tx(
        &mut tx,
        32,
        0,
        &f.source.id,
        "Failed",
        None,
        false,
    )
    .await
    .unwrap();
    crate::coordinator_watches::record_steering_event_tx(
        &mut tx,
        crate::coordinator_watches::MessageExecutionSource::Steering,
        "steering-failure",
        &f.source.id,
        "Failed",
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let all = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter()
            .filter(|fact| fact.outcome == TerminalFactOutcome::Failed)
            .count(),
        2
    );
}

async fn coordinator(db: &Database) -> String {
    db.get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap()
        .id
}
async fn remote_fixture(db: &Database) -> RemoteSubscription {
    let reference = SubscriptionReference {
        instance_id: InstanceId::new(),
        product_conversation_id: ProductConversationId::parse("same-as-local-product").unwrap(),
    };
    db.save_remote_subscription(&reference, Uuid::new_v4())
        .await
        .unwrap();
    db.remote_subscription_for_product(&reference)
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn exact_management_lookup_reaches_rows_beyond_bounded_listing() {
    let db = Database::open_in_memory().await.unwrap();
    let destination = InstanceId::new();
    let target_reference = SubscriptionReference {
        instance_id: destination,
        product_conversation_id: ProductConversationId::parse("management-target").unwrap(),
    };
    let target_subscription = Uuid::from_u128(u128::MAX);
    db.save_remote_subscription(&target_reference, target_subscription)
        .await
        .unwrap();

    for ordinal in 1..=1024_u128 {
        let reference = SubscriptionReference {
            instance_id: destination,
            product_conversation_id: ProductConversationId::parse(format!(
                "management-filler-{ordinal}"
            ))
            .unwrap(),
        };
        db.save_remote_subscription(&reference, Uuid::from_u128(ordinal))
            .await
            .unwrap();
    }

    let bounded = db
        .remote_subscriptions(Some(destination), 1024)
        .await
        .unwrap();
    assert_eq!(bounded.len(), 1024);
    assert!(bounded
        .iter()
        .all(|subscription| subscription.subscription_id != target_subscription));

    let active = db
        .remote_subscription_for_management(&target_reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.subscription_id, target_subscription);
    assert!(!active.pending_unsubscribe);

    db.begin_remote_unsubscribe(&target_reference, target_subscription)
        .await
        .unwrap();
    let pending = db
        .remote_subscription_for_management(&target_reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.subscription_id, target_subscription);
    assert!(pending.pending_unsubscribe);
}

fn single_fact() -> SubscriptionReplay {
    SubscriptionReplay::Facts {
        events: vec![FederatedTerminalFact {
            event_id: Uuid::new_v4(),
            sequence: cursor(1).unwrap(),
            occurrence: occurrence("exact-remote-occurrence"),
            source_transcript_id: TranscriptConversationId::parse("exact-remote-transcript")
                .unwrap(),
            outcome: TerminalFactOutcome::Failed,
            occurred_at_us: NOW,
        }],
    }
}
async fn fixture_navigation(
    db: &Database,
    remote: &RemoteSubscription,
    id: &str,
) -> (String, String) {
    let transcript: String = sqlx::query_scalar("SELECT COALESCE(f.source_transcript_id,g.source_transcript_id) FROM federation_remote_admissions a LEFT JOIN federation_received_facts f USING(admission_id) LEFT JOIN federation_received_gaps g USING(admission_id) WHERE a.admission_id = ?1")
        .bind(id).fetch_one(db.pool()).await.unwrap();
    let product =
        presentation::encoded_component(remote.reference.product_conversation_id.as_str());
    let transcript = presentation::encoded_component(&transcript);
    (
        format!("/c/{product}"),
        format!("/c/{transcript}?source_transcript={transcript}"),
    )
}

async fn fixture_authorize(db: &Database, remote: &RemoteSubscription, id: &str) {
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT authorization_id FROM federation_remote_admissions WHERE admission_id = ?1",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    if existing.is_some() {
        return;
    }
    db.request_remote_notification_authorization(remote, id)
        .await
        .unwrap();
    let (stable, source) = fixture_navigation(db, remote, id).await;
    db.confirm_remote_authorization(remote, id, Uuid::new_v4(), &stable, &source)
        .await
        .unwrap();
}
async fn steer(db: &Database, target: &str, id: &str) -> DbResult<()> {
    sqlx::query("INSERT INTO steering_messages(message_id,conversation_id,ordinal,text,origin_kind,origin_subscription_event_id) SELECT ?1,?2,COALESCE(MAX(ordinal),-1)+1,'typed remote fact','subscription_event',?1 FROM steering_messages WHERE conversation_id = ?2").bind(id).bind(target).execute(db.pool()).await?;
    Ok(())
}

#[tokio::test]
async fn received_stage_restart_windows_atomic_steering_admission_and_peer_independence() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    let replay = single_fact();
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    assert_eq!(
        db.remote_subscription_for_product(&remote.reference)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor,
        SubscriptionCursor::START
    );
    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].admission_id.starts_with("federation:"));
    fixture_authorize(&db, &remote, &pending[0].admission_id).await;
    let mut rollback = db.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO steering_messages(message_id,conversation_id,ordinal,text,origin_kind,origin_subscription_event_id) VALUES (?1,?2,0,'rollback','subscription_event',?1)").bind(&pending[0].admission_id).bind(&target).execute(&mut *rollback).await.unwrap();
    rollback.rollback().await.unwrap();
    assert_eq!(
        db.remote_subscription_for_product(&remote.reference)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor,
        SubscriptionCursor::START
    );
    fixture_authorize(&db, &remote, &pending[0].admission_id).await;
    steer(&db, &target, &pending[0].admission_id).await.unwrap();
    assert_eq!(
        db.remote_subscription_for_product(&remote.reference)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        1
    );
    assert!(db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .is_empty());
    db.end_remote_subscription(
        &remote.reference,
        remote.subscription_id,
        SubscriptionEnd::SourceClosed,
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_received_facts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM federation_remote_admissions WHERE accepted_transcript_id IS NOT NULL").fetch_one(db.pool()).await.unwrap(),1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn failed_gap_replay_survives_destination_prefix_cursor_progress() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let mut remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let pending = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    fixture_authorize(&db, &remote, &pending.admission_id).await;
    steer(&db, &target, &pending.admission_id).await.unwrap();
    remote = db
        .remote_subscription_for_product(&remote.reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote.admitted_cursor, cursor(1).unwrap());

    let gap = SubscriptionGap {
        last_acknowledged: SubscriptionCursor::START,
        last_produced: cursor(1).unwrap(),
        reason: GapReason::DisconnectedAge,
        failed_at_us: NOW,
        terminal_evidence_expires_at_us: NOW + TERMINAL_EVIDENCE_LIFETIME_US,
        snapshot: SubscriptionSnapshot {
            transcript_id: TranscriptConversationId::parse("continued-exact-head").unwrap(),
            state: SubscriptionCurrentState::Nonterminal,
        },
    };
    let replay = SubscriptionReplay::Gap { gap: gap.clone() };
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    db.stage_remote_replay(&remote, &replay).await.unwrap();

    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(matches!(
        &pending[0].content,
        RemoteNotificationContent::Gap { gap: stored } if stored == &gap
    ));
    assert_eq!(
        db.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor,
        cursor(1).unwrap()
    );
}

#[tokio::test]
async fn shorter_next_pull_preserves_previously_staged_fact_and_exact_next_cursor() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    let SubscriptionReplay::Facts { mut events } = single_fact() else {
        unreachable!()
    };
    let mut second = events[0].clone();
    second.event_id = Uuid::new_v4();
    second.sequence = cursor(2).unwrap();
    second.source_transcript_id = TranscriptConversationId::parse("second-exact-remote").unwrap();
    events.push(second.clone());

    db.stage_remote_replay(
        &remote,
        &SubscriptionReplay::Facts {
            events: events.clone(),
        },
    )
    .await
    .unwrap();
    let initial = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(initial.len(), 2);
    let first_id = initial[0].admission_id.clone();
    let second_id = initial[1].admission_id.clone();

    db.stage_remote_replay(
        &remote,
        &SubscriptionReplay::Facts {
            events: vec![events[0].clone()],
        },
    )
    .await
    .unwrap();
    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].admission_id, first_id);
    assert_eq!(pending[1].admission_id, second_id);

    fixture_authorize(&db, &remote, &first_id).await;
    steer(&db, &target, &first_id).await.unwrap();
    let next = db
        .remote_subscription_for_product(&remote.reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.admitted_cursor.get(), 1);
    db.stage_remote_replay(
        &next,
        &SubscriptionReplay::Facts {
            events: vec![second],
        },
    )
    .await
    .unwrap();
    let remaining = db.pending_remote_notifications(&next).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].admission_id, second_id);
}

#[tokio::test]
async fn ended_replay_suppresses_only_unrequested_unaccepted_facts() {
    let db = Database::open_in_memory().await.unwrap();
    let remote = remote_fixture(&db).await;
    let SubscriptionReplay::Facts { mut events } = single_fact() else {
        unreachable!()
    };
    let mut second = events[0].clone();
    second.event_id = Uuid::new_v4();
    second.sequence = cursor(2).unwrap();
    events.push(second);
    db.stage_remote_replay(&remote, &SubscriptionReplay::Facts { events })
        .await
        .unwrap();
    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    db.request_remote_notification_authorization(&remote, &pending[0].admission_id)
        .await
        .unwrap();

    db.stage_remote_replay(
        &remote,
        &SubscriptionReplay::Ended {
            reason: SubscriptionEnd::SourceClosed,
        },
    )
    .await
    .unwrap();
    let history = db
        .remote_notification_history(&remote, SubscriptionCursor::START)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert!(matches!(
        history[0].disposition,
        RemoteNotificationDisposition::Authorizing
    ));
    assert!(matches!(
        history[1].disposition,
        RemoteNotificationDisposition::Suppressed { .. }
    ));
    let ended = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, SubscriptionState::SourceClosed);
    assert_eq!(ended.admitted_cursor, SubscriptionCursor::START);
}

#[tokio::test]
async fn ordinary_durable_turn_admission_advances_cursor_in_accept_transaction() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let pending = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    fixture_authorize(&db, &remote, &pending.admission_id).await;
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: pending.admission_id.clone(),
            origin: InputOrigin::SubscriptionEvent {
                event_id: pending.admission_id.clone(),
            },
            text: "fact".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "fact".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    db.workflow_repository()
        .accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(&pending.admission_id).unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(target),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .established()
        .expect("federated admission must classify as a durable fact")
        .unwrap();
    assert_eq!(
        db.remote_subscription_for_product(&remote.reference)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn gap_is_not_a_fact_and_cursor_moves_only_when_notification_is_accepted() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    let SubscriptionReplay::Facts { mut events } = single_fact() else {
        unreachable!()
    };
    let mut second = events[0].clone();
    second.event_id = Uuid::new_v4();
    second.sequence = cursor(2).unwrap();
    events.push(second);
    db.stage_remote_replay(&remote, &SubscriptionReplay::Facts { events })
        .await
        .unwrap();
    let staged = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(staged.len(), 2);
    let gap = SubscriptionGap {
        last_acknowledged: SubscriptionCursor::START,
        last_produced: cursor(12).unwrap(),
        reason: GapReason::PayloadBytes,
        failed_at_us: NOW,
        terminal_evidence_expires_at_us: NOW + TERMINAL_EVIDENCE_LIFETIME_US,
        snapshot: SubscriptionSnapshot {
            transcript_id: TranscriptConversationId::parse("continued-exact-head").unwrap(),
            state: SubscriptionCurrentState::Nonterminal,
        },
    };
    db.stage_remote_replay(&remote, &SubscriptionReplay::Gap { gap: gap.clone() })
        .await
        .unwrap();
    db.stage_remote_replay(&remote, &SubscriptionReplay::Gap { gap: gap.clone() })
        .await
        .unwrap();
    let history = db
        .remote_notification_history(&remote, SubscriptionCursor::START)
        .await
        .unwrap();
    assert_eq!(history.len(), 3);
    for original in &staged {
        let saved = history
            .iter()
            .find(|note| note.admission_id == original.admission_id)
            .unwrap();
        assert!(matches!(
            saved.content,
            RemoteNotificationContent::Fact { .. }
        ));
        assert!(matches!(
            saved.disposition,
            RemoteNotificationDisposition::Pending
        ));
    }
    assert_eq!(
        db.next_remote_subscription_sync_batch(8)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut conflicting = gap.clone();
    conflicting.reason = GapReason::EventCount;
    assert!(db
        .stage_remote_replay(&remote, &SubscriptionReplay::Gap { gap: conflicting })
        .await
        .is_err());
    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 2);
    assert!(matches!(
        &pending[0].content,
        RemoteNotificationContent::Fact { fact } if fact.sequence == cursor(1).unwrap()
    ));
    assert!(matches!(
        &pending[1].content,
        RemoteNotificationContent::Fact { fact } if fact.sequence == cursor(2).unwrap()
    ));
    assert_eq!(
        db.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        0
    );
    db.suppress_remote_notification(
        &remote,
        &pending[0].admission_id,
        RemoteSuppressionReason::AuthorizationDenied,
    )
    .await
    .unwrap();
    let suffix = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(suffix.len(), 1);
    assert_eq!(suffix[0].admission_id, pending[1].admission_id);
    assert!(matches!(
        db.request_remote_notification_authorization(&remote, &suffix[0].admission_id)
            .await
            .unwrap(),
        RemoteAuthorizationRequest::TerminalAttempt { .. }
    ));
    let (stable, source) = fixture_navigation(&db, &remote, &suffix[0].admission_id).await;
    db.confirm_remote_authorization(
        &remote,
        &suffix[0].admission_id,
        Uuid::new_v4(),
        &stable,
        &source,
    )
    .await
    .unwrap();
    steer(&db, &target, &suffix[0].admission_id).await.unwrap();
    assert_eq!(
        db.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        0
    );
    let pending_gap = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending_gap.len(), 1);
    assert!(matches!(
        &pending_gap[0].content,
        RemoteNotificationContent::Gap { gap: stored } if stored == &gap
    ));
    assert!(db
        .request_remote_notification_authorization(&remote, &pending_gap[0].admission_id)
        .await
        .is_err());
    steer(&db, &target, &pending_gap[0].admission_id)
        .await
        .unwrap();
    assert_eq!(
        db.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        0
    );
    assert!(db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_remote_navigation WHERE admission_id = ?1"
        )
        .bind(&pending_gap[0].admission_id)
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn failed_gap_authorization_attempts_survive_restart_and_exhaust_into_suppression() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("failed-gap-attempts.sqlite")
        .to_string_lossy()
        .into_owned();
    let db = open_migrated(&path).await;
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let gap = SubscriptionGap {
        last_acknowledged: SubscriptionCursor::START,
        last_produced: cursor(1).unwrap(),
        reason: GapReason::DisconnectedAge,
        failed_at_us: NOW,
        terminal_evidence_expires_at_us: NOW + TERMINAL_EVIDENCE_LIFETIME_US,
        snapshot: SubscriptionSnapshot {
            transcript_id: TranscriptConversationId::parse("failed-gap-source").unwrap(),
            state: SubscriptionCurrentState::Terminal {
                outcome: TerminalFactOutcome::Failed,
            },
        },
    };
    db.stage_remote_replay(&remote, &SubscriptionReplay::Gap { gap })
        .await
        .unwrap();
    let fact = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    assert!(matches!(
        db.request_remote_notification_authorization(&remote, &fact.admission_id)
            .await
            .unwrap(),
        RemoteAuthorizationRequest::TerminalAttempt { attempt: 1 }
    ));
    db.pool.close().await;

    let db = open_migrated(&path).await;
    assert!(matches!(
        db.request_remote_notification_authorization(&remote, &fact.admission_id)
            .await
            .unwrap(),
        RemoteAuthorizationRequest::TerminalAttempt { attempt: 2 }
    ));
    assert!(matches!(
        db.request_remote_notification_authorization(&remote, &fact.admission_id)
            .await
            .unwrap(),
        RemoteAuthorizationRequest::TerminalAttempt { attempt: 3 }
    ));
    db.pool.close().await;

    let db = open_migrated(&path).await;
    assert_eq!(
        db.request_remote_notification_authorization(&remote, &fact.admission_id)
            .await
            .unwrap(),
        RemoteAuthorizationRequest::TerminalExhausted
    );
    let history = db
        .remote_notification_history(&remote, SubscriptionCursor::START)
        .await
        .unwrap();
    assert!(matches!(
        history[0].disposition,
        RemoteNotificationDisposition::Suppressed {
            reason: RemoteSuppressionReason::SourceUnreachable
        }
    ));
    assert!(matches!(
        history[1].content,
        RemoteNotificationContent::Gap { .. }
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT terminal_settlement_attempts FROM federation_remote_admissions WHERE admission_id = ?1"
        )
        .bind(&fact.admission_id)
        .fetch_one(db.pool())
        .await
        .unwrap(),
        3
    );
    assert!(db
        .confirm_remote_authorization(
            &remote,
            &fact.admission_id,
            Uuid::new_v4(),
            "/conversation/late",
            "/conversation/late?source_transcript=late",
        )
        .await
        .is_err());
    assert!(matches!(
        db.pending_remote_notifications(&remote)
            .await
            .unwrap()
            .as_slice(),
        [RemoteNotification {
            content: RemoteNotificationContent::Gap { .. },
            ..
        }]
    ));
}

#[tokio::test]
async fn malformed_replay_rolls_back_and_unsubscribe_blocks_unadmitted_but_preserves_identity() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    let SubscriptionReplay::Facts { mut events } = single_fact() else {
        unreachable!()
    };
    let mut bad = events[0].clone();
    bad.event_id = Uuid::new_v4();
    bad.sequence = cursor(3).unwrap();
    events.push(bad);
    assert!(db
        .stage_remote_replay(&remote, &SubscriptionReplay::Facts { events })
        .await
        .is_err());
    assert!(db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .is_empty());
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let pending = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    assert!(steer(&db, "missing-transcript", &pending.admission_id)
        .await
        .is_err());
    db.end_remote_subscription(
        &remote.reference,
        remote.subscription_id,
        SubscriptionEnd::Unsubscribed,
    )
    .await
    .unwrap();
    assert!(steer(&db, &target, &pending.admission_id).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT admitted_cursor FROM federation_remote_subscriptions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

async fn continuation_candidate(f: &Fixture) {
    use phoenix_core::domain::sm_state::ConvState;
    let mut tx = f.db.pool.begin().await.unwrap();
    append_terminal_tx(
        &mut tx,
        ClassifiedTerminal {
            occurrence_kind: "direct_turn",
            occurrence_id: "exhausted-source",
            generation: 0,
            transcript_id: &f.source.id,
            outcome: TerminalFactOutcome::Failed,
            context_exhausted: true,
            occurred_at_us: NOW,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    f.db.update_conversation_state(
        &f.source.id,
        &ConvState::ContextExhausted {
            summary: "source summary".into(),
        },
    )
    .await
    .unwrap();
    let summary =
        f.db.add_message(
            "federation-summary",
            &f.source.id,
            &crate::MessageContent::continuation("source summary"),
            None,
            None,
        )
        .await
        .unwrap();
    f.db.set_auto_continue_on_context_exhaustion(
        &f.source.product_conversation_id,
        phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion::Enabled,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO automatic_continuation_admissions(predecessor_conversation_id,product_conversation_id,summary_message_id,operation_id,first_message_id,admitted_at_unix_micros,updated_at_unix_micros) VALUES (?1,?2,?3,'federation-continuation','federation-successor-first',1,1)").bind(&f.source.id).bind(f.source.product_conversation_id.as_str()).bind(summary.message_id).execute(f.db.pool()).await.unwrap();
}

#[tokio::test]
async fn continuation_candidate_is_hidden_then_released_when_handoff_fails() {
    let f = Fixture::new().await;
    continuation_candidate(&f).await;
    assert!(facts(f.replay(0, NOW).await).is_empty());
    sqlx::query("UPDATE automatic_continuation_admissions SET phase = 'failed', last_error = 'handoff failed' WHERE predecessor_conversation_id = ?1").bind(&f.source.id).execute(f.db.pool()).await.unwrap();
    let released = facts(f.replay(0, NOW).await);
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].outcome, TerminalFactOutcome::Failed);
    assert_eq!(released[0].source_transcript_id.as_str(), f.source.id);
}

#[tokio::test]
async fn successful_handoff_suppresses_candidate_and_future_fact_keeps_exact_successor() {
    let f = Fixture::new().await;
    continuation_candidate(&f).await;
    assert!(facts(f.replay(0, NOW).await).is_empty());
    let (outcome, _) =
        f.db.continue_conversation_with_intent(
            &f.source.id,
            crate::NewContinuationDispatchIntent::generated_predecessor_context(
                ClientTurnKey::new("federation-successor-first").unwrap(),
                "source summary".into(),
            ),
        )
        .await
        .unwrap();
    let crate::ContinueOutcome::Created(successor) = outcome else {
        panic!("successor required")
    };
    f.db.add_message(
        "federation-successor-first",
        &successor.id,
        &crate::MessageContent::continuation("source summary"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(facts(f.replay(0, NOW).await).is_empty());
    f.db.update_conversation_state(
        &successor.id,
        &phoenix_core::domain::sm_state::ConvState::Idle,
    )
    .await
    .unwrap();
    let mut tx = f.db.pool.begin().await.unwrap();
    append_terminal_tx(
        &mut tx,
        ClassifiedTerminal {
            occurrence_kind: "steering",
            occurrence_id: "successor-completion",
            generation: 0,
            transcript_id: &successor.id,
            outcome: TerminalFactOutcome::Completed,
            context_exhausted: false,
            occurred_at_us: NOW + 1,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let events = facts(f.replay(0, NOW + 1).await);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source_transcript_id.as_str(), successor.id);
}

#[tokio::test]
async fn restart_preserves_staged_identity_and_admitted_cursor_after_lost_ack() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("subscriptions.sqlite")
        .to_string_lossy()
        .into_owned();
    let db = open_migrated(&path).await;
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    let replay = single_fact();
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    let id = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0)
        .admission_id;
    db.pool.close().await;
    let db = open_migrated(&path).await;
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    assert_eq!(
        db.pending_remote_notifications(&remote).await.unwrap()[0].admission_id,
        id
    );
    fixture_authorize(&db, &remote, &id).await;
    steer(&db, &target, &id).await.unwrap();
    db.pool.close().await;
    let db = open_migrated(&path).await;
    let resumed = db
        .remote_subscription_for_product(&remote.reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.admitted_cursor.get(), 1);
    assert!(db
        .pending_remote_notifications(&resumed)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn same_event_uuid_on_different_destinations_cannot_alias_local_or_each_other() {
    let db = Database::open_in_memory().await.unwrap();
    let local = db
        .create_conversation("same-as-local-product", "local", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&local.product_conversation_id)
        .await
        .unwrap();
    let first = remote_fixture(&db).await;
    let second = remote_fixture(&db).await;
    let replay = single_fact();
    db.stage_remote_replay(&first, &replay).await.unwrap();
    db.stage_remote_replay(&second, &replay).await.unwrap();
    let a = db
        .pending_remote_notifications(&first)
        .await
        .unwrap()
        .remove(0);
    let b = db
        .pending_remote_notifications(&second)
        .await
        .unwrap()
        .remove(0);
    assert_ne!(a.admission_id, b.admission_id);
    assert_ne!(a.reference.instance_id, b.reference.instance_id);
    assert_eq!(db.list_coordinator_watches().await.unwrap().len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_received_facts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2
    );
}
async fn source_turn(db: &Database, conversation: &str, key: &str) -> u64 {
    let payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            message_id: key.into(),
            origin: InputOrigin::UserApi,
            text: "source input".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::LiteralText,
        },
        PreparedDirectTurnDelivery {
            text: "source input".into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            user_agent: None,
            skill_invocation: None,
        },
    );
    let result = db
        .workflow_repository()
        .accept_authoritative_turn(&AcceptAuthoritativeTurn {
            client_key: ClientTurnKey::new(key).unwrap(),
            prepared: PreparedTurn::from_exact_payload(
                &ConversationAuthority(conversation.into()),
                payload.to_exact_bytes().unwrap(),
            ),
            disposition: AcceptedDisposition::Runtime,
            accepted_at: Timestamp(1),
        })
        .await
        .established()
        .expect("local turn must classify as a durable fact")
        .unwrap();
    let TurnOutcome::Created { turn_id, .. } = result.outcome else {
        panic!("turn was not created")
    };
    turn_id.0
}

fn terminal_input(
    turn: u64,
    state: phoenix_core::domain::sm_state::ConvState,
    outcome: TerminalFactOutcome,
) -> crate::workflow::direct_turn::TerminalizeAuthoritativeTurnInput {
    use phoenix_workflow::{TurnAuthorityId, TurnCommand};
    let turn_id = TurnAuthorityId(turn);
    crate::workflow::direct_turn::TerminalizeAuthoritativeTurnInput {
        command: match outcome {
            TerminalFactOutcome::Completed => TurnCommand::Complete {
                turn_id,
                expected_generation: 0,
            },
            TerminalFactOutcome::Failed => TurnCommand::Fail {
                turn_id,
                expected_generation: 0,
                reason: "source failure".into(),
            },
            TerminalFactOutcome::Cancelled => TurnCommand::Cancel {
                turn_id,
                expected_generation: 0,
            },
        },
        projection: Some(
            crate::workflow::direct_turn::PersistedConversationProjection {
                state,
                state_updated_at: chrono::Utc::now(),
            },
        ),
        provider_replay_settlement:
            phoenix_core::domain::provider_replay::ProviderReplaySettlement::Preserve,
    }
}

async fn authorize_source_at(
    f: &Fixture,
    delivery: &SubscriptionDelivery,
    observed_at_us: i64,
) -> SubscriptionAuthorization {
    f.db.authorize_federation_delivery(
        &f.authority,
        f.subscription,
        &f.source.product_conversation_id,
        delivery,
        observed_at_us,
    )
    .await
    .unwrap()
}

async fn authorize_source(
    f: &Fixture,
    delivery: &SubscriptionDelivery,
) -> SubscriptionAuthorization {
    authorize_source_at(f, delivery, NOW).await
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn terminal_evidence_expires_at_the_immutable_deadline_and_leaves_the_gap_tombstone() {
    let f = Fixture::new().await;
    f.emit(
        "leased-terminal-evidence",
        TerminalFactOutcome::Completed,
        NOW,
    )
    .await;
    let fact = facts(f.replay(0, NOW).await).remove(0);
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + MAX_DISCONNECTED_AGE_US).await else {
        panic!("disconnected source must freeze a terminal gap");
    };
    let expiry = gap.terminal_evidence_expires_at_us;
    assert_eq!(expiry, gap.failed_at_us + TERMINAL_EVIDENCE_LIFETIME_US);
    let delivery = SubscriptionDelivery::Fact { fact };

    assert!(matches!(
        authorize_source_at(&f, &delivery, expiry - 1).await,
        SubscriptionAuthorization::Authorized { .. }
    ));
    assert!(matches!(
        f.replay(0, expiry - 1).await,
        SubscriptionReplay::Gap { gap: replayed } if replayed == gap
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_terminal_payloads WHERE event_id IN (SELECT event_id FROM federation_terminal_identities WHERE subscription_id = ?1)"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_delivery_authorizations WHERE subscription_id = ?1"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        1
    );
    let release_reason: Option<String> = sqlx::query_scalar(
        "SELECT evidence_release_reason FROM federation_terminal_identities WHERE subscription_id = ?1 LIMIT 1",
    )
    .bind(f.subscription.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(release_reason, None);

    assert!(matches!(
        f.replay(0, expiry).await,
        SubscriptionReplay::Gap { gap: replayed } if replayed == gap
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_terminal_payloads WHERE event_id IN (SELECT event_id FROM federation_terminal_identities WHERE subscription_id = ?1)"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_delivery_authorizations WHERE subscription_id = ?1"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_terminal_identities WHERE subscription_id = ?1 AND evidence_release_reason = 'expired'"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        1
    );
    let snapshot_outcome = match gap.snapshot.state {
        SubscriptionCurrentState::Nonterminal => None,
        SubscriptionCurrentState::Terminal { outcome } => Some(outcome.as_str()),
    };
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_subscription_gaps WHERE subscription_id = ?1 AND last_acknowledged = ?2 AND last_produced = ?3 AND reason = ?4 AND failed_at_us = ?5 AND terminal_evidence_expires_at_us = ?6 AND snapshot_transcript_id = ?7 AND snapshot_outcome IS ?8"
        )
        .bind(f.subscription.to_string())
        .bind(gap.last_acknowledged.get())
        .bind(gap.last_produced.get())
        .bind(gap.reason.as_str())
        .bind(gap.failed_at_us)
        .bind(expiry)
        .bind(gap.snapshot.transcript_id.as_str())
        .bind(snapshot_outcome)
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        1
    );

    assert!(matches!(
        f.replay(0, expiry + 1).await,
        SubscriptionReplay::Gap { gap: replayed } if replayed == gap
    ));
    assert!(matches!(
        f.replay(0, expiry - 1).await,
        SubscriptionReplay::Gap { gap: replayed } if replayed == gap
    ));
    assert_eq!(
        authorize_source_at(&f, &delivery, expiry + 1).await,
        SubscriptionAuthorization::EvidenceExpired
    );
    assert_eq!(
        authorize_source_at(&f, &delivery, expiry - 1).await,
        SubscriptionAuthorization::Refresh
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_terminal_payloads WHERE event_id IN (SELECT event_id FROM federation_terminal_identities WHERE subscription_id = ?1)"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM federation_delivery_authorizations WHERE subscription_id = ?1"
        )
        .bind(f.subscription.to_string())
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        0
    );
    let release_reason: Option<String> = sqlx::query_scalar(
        "SELECT evidence_release_reason FROM federation_terminal_identities WHERE subscription_id = ?1 LIMIT 1",
    )
    .bind(f.subscription.to_string())
    .fetch_one(f.db.pool())
    .await
    .unwrap();
    assert_eq!(release_reason.as_deref(), Some("expired"));
}

#[tokio::test]
async fn cancel_close_restores_gap_authorization_without_reactivating_subscription() {
    let f = Fixture::new().await;
    f.emit("expired-before-close", TerminalFactOutcome::Completed, NOW)
        .await;
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + MAX_DISCONNECTED_AGE_US).await else {
        panic!("expired payload must produce a gap");
    };
    let expected_gap = gap.clone();
    let delivery = SubscriptionDelivery::Gap { gap };
    f.db.begin_close_foundation(
        &f.source.product_conversation_id,
        &TranscriptConversationId::parse(&f.source.id).unwrap(),
        "cancellable-gap-close",
    )
    .await
    .unwrap();
    assert_eq!(
        authorize_source(&f, &delivery).await,
        SubscriptionAuthorization::Refresh
    );
    f.db.cancel_close_before_retirement("cancellable-gap-close")
        .await
        .unwrap();
    assert!(matches!(
        authorize_source(&f, &delivery).await,
        SubscriptionAuthorization::Authorized { .. }
    ));
    f.emit(
        "after-cancelled-close",
        TerminalFactOutcome::Cancelled,
        NOW + MAX_DISCONNECTED_AGE_US + 1,
    )
    .await;
    assert!(matches!(
        f.replay(0, NOW + MAX_DISCONNECTED_AGE_US + 1).await,
        SubscriptionReplay::Gap { gap: retry } if retry == expected_gap
    ));
}

async fn consumer_for_source(f: &Fixture) -> (Database, RemoteSubscription, String) {
    let db = Database::open_in_memory().await.unwrap();
    let reference = SubscriptionReference {
        instance_id: InstanceId::new(),
        product_conversation_id: f.source.product_conversation_id.clone(),
    };
    db.save_remote_subscription(&reference, f.subscription)
        .await
        .unwrap();
    let remote = db
        .remote_subscription_for_product(&reference)
        .await
        .unwrap()
        .unwrap();
    let target = coordinator(&db).await;
    (db, remote, target)
}

async fn close_to_history(f: &Fixture) {
    use phoenix_core::domain::close::CloseAttemptId;
    let attempt = CloseAttemptId::parse("subscription-close").unwrap();
    f.db.begin_close_foundation(
        &f.source.product_conversation_id,
        &TranscriptConversationId::parse(&f.source.id).unwrap(),
        attempt.as_str(),
    )
    .await
    .unwrap();
    f.db.begin_close_idle_settlement(attempt.as_str())
        .await
        .unwrap();
    f.db.advance_close_settlement_when_quiescent(attempt.as_str())
        .await
        .unwrap();
    f.db.replace_close_inspection(crate::close_foundation::ReplaceCloseInspectionRequest {
        attempt_id: attempt.clone(),
        scopes: vec![],
    })
    .await
    .unwrap();
    let scopes: Vec<String> =
        sqlx::query_scalar("SELECT scope FROM close_attempt_scopes WHERE attempt_id = ?1")
            .bind(attempt.as_str())
            .fetch_all(f.db.pool())
            .await
            .unwrap();
    let snapshot =
        f.db.get_close_obligation(attempt.as_str())
            .await
            .unwrap()
            .snapshot()
            .unwrap()
            .clone();
    let resources =
        f.db.capture_close_retirement_inventory(
            crate::close_foundation::CaptureCloseRetirementInventoryRequest {
                attempt_id: attempt.clone(),
                snapshot,
                scopes: scopes
                    .into_iter()
                    .map(|scope| {
                        crate::close_foundation::CaptureCloseRetirementInventoryScopeRequest {
                            scope: phoenix_core::work_scope::WorkScopeId::parse(scope).unwrap(),
                            inventory: phoenix_core::domain::close::CloseOwnedResourceInventory {
                                worktree: None,
                                work_scopes: std::collections::BTreeSet::default(),
                                bash_process_groups: std::collections::BTreeSet::default(),
                                tmux_servers: std::collections::BTreeSet::default(),
                                pty_sessions: std::collections::BTreeSet::default(),
                                browser_sessions: std::collections::BTreeSet::default(),
                                equivalent_live_resources: std::collections::BTreeSet::default(),
                            },
                        }
                    })
                    .collect(),
            },
        )
        .await
        .unwrap();
    for resource in resources {
        f.db.record_close_retirement_dispatch(
            crate::close_foundation::RecordCloseRetirementDispatchRequest {
                attempt_id: attempt.clone(),
                scope: resource.scope.clone(),
                snapshot: resource.snapshot.clone(),
                resource: resource.resource.clone(),
            },
        )
        .await
        .unwrap();
        f.db.record_close_retirement_evidence(
            crate::close_foundation::RecordCloseRetirementEvidenceRequest {
                attempt_id: attempt.clone(),
                scope: resource.scope,
                snapshot: resource.snapshot,
                resource: resource.resource,
                outcome: phoenix_core::domain::close::RetirementOutcome::Retired,
                detail: None,
            },
        )
        .await
        .unwrap();
    }
    f.db.complete_close_retirement(&attempt).await.unwrap();
    let lifecycle: String =
        sqlx::query_scalar("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
            .bind(f.source.product_conversation_id.as_str())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert_eq!(lifecycle, "history");
}

#[tokio::test]
async fn real_direct_terminal_projection_and_fanout_commit_or_rollback_together() {
    use phoenix_core::domain::sm_state::ConvState;
    let f = Fixture::new().await;
    let turn = source_turn(&f.db, &f.source.id, "production-terminal").await;
    f.db.update_conversation_state(&f.source.id, &ConvState::LlmRequesting { attempt: 1 })
        .await
        .unwrap();
    let input = terminal_input(turn, ConvState::Idle, TerminalFactOutcome::Completed);
    sqlx::raw_sql("CREATE TRIGGER reject_subscription_payload BEFORE INSERT ON federation_terminal_payloads BEGIN SELECT RAISE(ABORT,'injected fanout failure'); END;").execute(f.db.pool()).await.unwrap();
    assert!(f
        .db
        .workflow_repository()
        .terminalize_authoritative_turn(&input)
        .await
        .is_err());
    let terminal: Option<String> =
        sqlx::query_scalar("SELECT terminal_kind FROM durable_turns WHERE turn_id = ?1")
            .bind(i64::try_from(turn).unwrap())
            .fetch_one(f.db.pool())
            .await
            .unwrap();
    assert!(terminal.is_none());
    assert!(matches!(
        f.db.get_conversation(&f.source.id).await.unwrap().state,
        ConvState::LlmRequesting { .. }
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_occurrences")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql("DROP TRIGGER reject_subscription_payload")
        .execute(f.db.pool())
        .await
        .unwrap();
    f.db.workflow_repository()
        .terminalize_authoritative_turn(&input)
        .await
        .unwrap();
    let events = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, TerminalFactOutcome::Completed);
}

#[tokio::test]
async fn pre_enrollment_production_terminal_is_not_a_future_fact() {
    use phoenix_core::domain::sm_state::ConvState;
    let db = Database::open_in_memory().await.unwrap();
    let source = db
        .create_conversation(
            "before-any-peer",
            "before-any-peer",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let historical = source_turn(&db, &source.id, "truly-before-enrollment").await;
    db.workflow_repository()
        .terminalize_authoritative_turn(&terminal_input(
            historical,
            ConvState::Idle,
            TerminalFactOutcome::Completed,
        ))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_enrollments")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    let caller = InstanceId::new();
    let verifier = FederationCredentialVerifier::from_bearer(b"later-peer");
    db.replace_federation_enrollment(caller, "later peer", &verifier)
        .await
        .unwrap();
    let enrollment = active_enrollment_id(&db, caller).await;
    let f = Fixture {
        db,
        source,
        caller,
        enrollment,
        authority: send_authority(caller, verifier),
        subscription: Uuid::new_v4(),
    };
    f.db.create_federation_subscription(
        &f.authority,
        f.subscription,
        &f.source.product_conversation_id,
        NOW,
    )
    .await
    .unwrap();
    assert!(facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await).is_empty());
    let future = source_turn(&f.db, &f.source.id, "actually-future").await;
    f.db.workflow_repository()
        .terminalize_authoritative_turn(&terminal_input(
            future,
            ConvState::Idle,
            TerminalFactOutcome::Cancelled,
        ))
        .await
        .unwrap();
    assert_eq!(
        facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await).len(),
        1
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn real_history_close_preserves_accepted_and_suppresses_every_unadmitted_fact() {
    let f = Fixture::new().await;
    for n in 0..3 {
        f.emit(
            &format!("close-{n}"),
            TerminalFactOutcome::Completed,
            NOW + n,
        )
        .await;
    }
    let (a, remote, target) = consumer_for_source(&f).await;
    a.stage_remote_replay(&remote, &f.replay(0, NOW + 3).await)
        .await
        .unwrap();
    let pending = a.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 3);
    assert!(
        steer(&a, &target, &pending[0].admission_id).await.is_err(),
        "staging is not admission authority"
    );
    for note in pending.iter().take(2) {
        a.request_remote_notification_authorization(&remote, &note.admission_id)
            .await
            .unwrap();
        let receipt = authorize_source(&f, &note.content).await;
        if note.admission_id == pending[0].admission_id {
            let SubscriptionAuthorization::Authorized { authorization_id } = receipt else {
                panic!("authorization required")
            };
            let (stable, source) = fixture_navigation(&a, &remote, &note.admission_id).await;
            a.confirm_remote_authorization(
                &remote,
                &note.admission_id,
                authorization_id,
                &stable,
                &source,
            )
            .await
            .unwrap();
            steer(&a, &target, &note.admission_id).await.unwrap();
        }
        // The second response is lost: only the source has its committed authorization.
    }
    close_to_history(&f).await;
    assert_eq!(
        f.replay(1, NOW + 3).await,
        SubscriptionReplay::Ended {
            reason: SubscriptionEnd::SourceClosed
        }
    );
    a.end_remote_subscription(
        &remote.reference,
        remote.subscription_id,
        SubscriptionEnd::SourceClosed,
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1
    );
    for note in pending.iter().skip(1) {
        assert_eq!(
            authorize_source(&f, &note.content).await,
            SubscriptionAuthorization::Rejected {
                reason: SubscriptionEnd::SourceClosed
            }
        );
        assert!(steer(&a, &target, &note.admission_id).await.is_err());
    }
    assert!(a
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .is_empty());
    let history = a
        .remote_notification_history(&remote, SubscriptionCursor::START)
        .await
        .unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|n| matches!(
                n.disposition,
                RemoteNotificationDisposition::Accepted { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        history
            .iter()
            .filter(|notification| matches!(
                notification.disposition,
                RemoteNotificationDisposition::Suppressed { .. }
            ))
            .count(),
        2
    );
    assert_eq!(
        a.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        1
    );
    assert!(matches!(
        f.replay(1, NOW + 3).await,
        SubscriptionReplay::Ended { .. }
    ));
    for table in [
        "federation_delivery_authorizations",
        "federation_terminal_payloads",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table}"
            )))
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
            1
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM messages WHERE message_id LIKE 'close-outcome:%'"
        )
        .fetch_one(f.db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn revoke_and_unsubscribe_serialize_against_authorization_in_both_orders() {
    for revoke in [false, true] {
        for authorization_first in [false, true] {
            let f = Fixture::new().await;
            f.emit("race", TerminalFactOutcome::Failed, NOW).await;
            let fact = facts(f.replay(0, NOW).await).remove(0);
            let delivery = SubscriptionDelivery::Fact { fact };
            let before = if authorization_first {
                Some(authorize_source(&f, &delivery).await)
            } else {
                None
            };
            if revoke {
                f.db.revoke_federation_enrollment(f.caller).await.unwrap();
            } else {
                f.db.unsubscribe_federation_subscription(
                    &f.authority,
                    f.subscription,
                    &f.source.product_conversation_id,
                )
                .await
                .unwrap();
            }
            let after = authorize_source(&f, &delivery).await;
            if let Some(before) = before {
                assert_eq!(after, before);
            } else {
                assert_eq!(
                    after,
                    SubscriptionAuthorization::Rejected {
                        reason: if revoke {
                            SubscriptionEnd::Revoked
                        } else {
                            SubscriptionEnd::Unsubscribed
                        }
                    }
                );
            }
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM federation_delivery_authorizations"
                )
                .fetch_one(f.db.pool())
                .await
                .unwrap(),
                i64::from(authorization_first)
            );
        }
    }
}

#[tokio::test]
async fn source_closed_suppression_survives_restart_and_preserves_admitted_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("source-closed.sqlite")
        .to_string_lossy()
        .into_owned();
    let db = open_migrated(&path).await;
    let remote = remote_fixture(&db).await;
    let target = coordinator(&db).await;
    let replay = SubscriptionReplay::Facts {
        events: vec![
            FederatedTerminalFact {
                event_id: Uuid::new_v4(),
                sequence: cursor(1).unwrap(),
                occurrence: occurrence("source-close-first"),
                source_transcript_id: TranscriptConversationId::parse("source-close-first")
                    .unwrap(),
                outcome: TerminalFactOutcome::Completed,
                occurred_at_us: NOW,
            },
            FederatedTerminalFact {
                event_id: Uuid::new_v4(),
                sequence: cursor(2).unwrap(),
                occurrence: occurrence("source-close-second"),
                source_transcript_id: TranscriptConversationId::parse("source-close-second")
                    .unwrap(),
                outcome: TerminalFactOutcome::Failed,
                occurred_at_us: NOW + 1,
            },
        ],
    };
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 2);

    for notification in &pending {
        db.request_remote_notification_authorization(&remote, &notification.admission_id)
            .await
            .unwrap();
        let (stable, source) = fixture_navigation(&db, &remote, &notification.admission_id).await;
        db.confirm_remote_authorization(
            &remote,
            &notification.admission_id,
            Uuid::new_v4(),
            &stable,
            &source,
        )
        .await
        .unwrap();
    }
    steer(&db, &target, &pending[0].admission_id).await.unwrap();
    db.end_remote_subscription(
        &remote.reference,
        remote.subscription_id,
        SubscriptionEnd::SourceClosed,
    )
    .await
    .unwrap();
    db.pool.close().await;

    let db = open_migrated(&path).await;
    let reopened = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reopened.state, SubscriptionState::SourceClosed);
    assert_eq!(reopened.admitted_cursor, cursor(1).unwrap());
    assert!(db
        .pending_remote_notifications(&reopened)
        .await
        .unwrap()
        .is_empty());
    let history = db
        .remote_notification_history(&reopened, SubscriptionCursor::START)
        .await
        .unwrap();
    assert!(matches!(
        history[0].disposition,
        RemoteNotificationDisposition::Accepted { .. }
    ));
    assert!(matches!(
        history[1].disposition,
        RemoteNotificationDisposition::Suppressed { .. }
    ));
    assert!(db
        .request_remote_notification_authorization(&reopened, &pending[1].admission_id)
        .await
        .is_err());
    let (stable, source) = fixture_navigation(&db, &reopened, &pending[1].admission_id).await;
    assert!(db
        .confirm_remote_authorization(
            &reopened,
            &pending[1].admission_id,
            Uuid::new_v4(),
            &stable,
            &source,
        )
        .await
        .is_err());
    assert!(steer(&db, &target, &pending[1].admission_id).await.is_err());
}

#[tokio::test]
async fn unsubscribe_identity_is_durable_retryable_and_never_reenrolls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("unsubscribe.sqlite")
        .to_string_lossy()
        .into_owned();
    let db = open_migrated(&path).await;
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    db.pool.close().await;
    let db = open_migrated(&path).await;
    let pending = db.next_remote_subscription_sync_batch(4).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].pending_unsubscribe);
    assert_eq!(pending[0].state, SubscriptionState::Unsubscribed);
    assert_eq!(pending[0].subscription_id, remote.subscription_id);
    db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    assert!(db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .save_remote_subscription(&remote.reference, Uuid::new_v4())
        .await
        .is_err());
    db.complete_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    assert!(db
        .next_remote_subscription_sync_batch(4)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn authorization_refresh_replaces_an_expired_attempt_without_ending_or_advancing() {
    let f = Fixture::new().await;
    f.emit("expires-before-authorize", TerminalFactOutcome::Failed, NOW)
        .await;
    let (a, remote, _) = consumer_for_source(&f).await;
    a.stage_remote_replay(&remote, &f.replay(0, NOW).await)
        .await
        .unwrap();
    let note = a
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    a.request_remote_notification_authorization(&remote, &note.admission_id)
        .await
        .unwrap();
    let fresh = f.replay(0, NOW + MAX_DISCONNECTED_AGE_US).await;
    assert_eq!(
        authorize_source(&f, &note.content).await,
        SubscriptionAuthorization::Refresh
    );
    a.suppress_remote_notification(
        &remote,
        &note.admission_id,
        RemoteSuppressionReason::AuthorizationDenied,
    )
    .await
    .unwrap();
    let unchanged = a
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.admitted_cursor, SubscriptionCursor::START);
    assert_eq!(unchanged.state, SubscriptionState::Active);
    a.stage_remote_replay(&unchanged, &fresh).await.unwrap();
    let replacement = a.pending_remote_notifications(&unchanged).await.unwrap();
    assert_eq!(replacement.len(), 1);
    assert!(matches!(
        replacement[0].content,
        SubscriptionDelivery::Gap { .. }
    ));
}

#[tokio::test]
async fn interrupted_authorization_rolls_back_navigation_and_retries_after_slug_rename() {
    let db = open_migrated(":memory:").await;
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let note = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    db.request_remote_notification_authorization(&remote, &note.admission_id)
        .await
        .unwrap();
    let (_, source) = fixture_navigation(&db, &remote, &note.admission_id).await;
    let authorization = Uuid::new_v4();
    sqlx::query("CREATE TRIGGER interrupt_confirmation BEFORE UPDATE OF authorization_id ON federation_remote_admissions BEGIN SELECT RAISE(ABORT, 'interrupted'); END")
        .execute(db.pool()).await.unwrap();
    assert!(db
        .confirm_remote_authorization(
            &remote,
            &note.admission_id,
            authorization,
            "/c/old-slug",
            &source
        )
        .await
        .is_err());
    let persisted: (Option<String>, Option<String>) = sqlx::query_as("SELECT a.authorization_id, n.stable_path FROM federation_remote_admissions a LEFT JOIN federation_remote_navigation n USING(admission_id) WHERE admission_id = ?1")
        .bind(&note.admission_id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(persisted, (None, None));
    sqlx::query("DROP TRIGGER interrupt_confirmation")
        .execute(db.pool())
        .await
        .unwrap();
    db.confirm_remote_authorization(
        &remote,
        &note.admission_id,
        authorization,
        "/c/renamed-slug",
        &source,
    )
    .await
    .unwrap();
    // A repeated source receipt may carry a newly canonical slug, not a new evidence identity.
    db.confirm_remote_authorization(
        &remote,
        &note.admission_id,
        authorization,
        "/c/renamed-again",
        &source,
    )
    .await
    .unwrap();
    let persisted: (String, String, String) = sqlx::query_as("SELECT a.authorization_id, n.stable_path, n.source_path FROM federation_remote_admissions a JOIN federation_remote_navigation n USING(admission_id) WHERE admission_id = ?1")
        .bind(&note.admission_id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        persisted,
        (
            authorization.to_string(),
            "/c/renamed-again".into(),
            source.clone()
        )
    );
    assert!(db
        .confirm_remote_authorization(
            &remote,
            &note.admission_id,
            Uuid::new_v4(),
            "/c/not-committed",
            &source
        )
        .await
        .is_err());
    let stable: String = sqlx::query_scalar(
        "SELECT stable_path FROM federation_remote_navigation WHERE admission_id = ?1",
    )
    .bind(&note.admission_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(stable, "/c/renamed-again");
}

#[tokio::test]
async fn received_navigation_is_canonical_durable_and_batched_with_authorization() {
    let db = open_migrated(":memory:").await;
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let note = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    db.request_remote_notification_authorization(&remote, &note.admission_id)
        .await
        .unwrap();
    let (stable, source) = fixture_navigation(&db, &remote, &note.admission_id).await;
    assert!(db
        .confirm_remote_authorization(&remote, &note.admission_id, Uuid::new_v4(), "", &source)
        .await
        .is_err());
    for path in [
        "//attacker.invalid/c/p",
        "/c/p?credential=secret",
        "/c/../p",
        "/c/p#secret",
    ] {
        assert!(db
            .confirm_remote_authorization(
                &remote,
                &note.admission_id,
                Uuid::new_v4(),
                path,
                &source,
            )
            .await
            .is_err());
    }
    db.confirm_remote_authorization(
        &remote,
        &note.admission_id,
        Uuid::new_v4(),
        &stable,
        &source,
    )
    .await
    .unwrap();
    let mut ids = vec![note.admission_id.clone(); 401];
    ids.push("federation:missing".into());
    let presentations = db.remote_notification_presentations(&ids).await.unwrap();
    assert_eq!(presentations.len(), 1);
    let (stored, paths) = &presentations[&note.admission_id];
    assert_eq!(stored.reference, remote.reference);
    assert!(matches!(
        stored.disposition,
        RemoteNotificationDisposition::Authorized { .. }
    ));
    let (stable, source) = paths.as_ref().unwrap();
    assert!(stable.starts_with("/c/"));
    assert!(source.contains("?source_transcript="));
}

#[tokio::test]
async fn failure_gap_snapshots_follow_real_projection_not_terminal_kind_whitelists() {
    use phoenix_core::domain::{
        db_schema::ErrorKind,
        sm_state::{ContinuationSummaryRequest, ConvState, RecoverableContinuationFailure},
    };
    let states = [
        (
            ConvState::Error {
                message: "provider failed".into(),
                error_kind: ErrorKind::ServerError,
                resets_at: None,
            },
            TerminalFactOutcome::Failed,
        ),
        (
            ConvState::CreationFailed {
                job_id: "create".into(),
                error: "provision failed".into(),
                error_kind: ErrorKind::ServerError,
            },
            TerminalFactOutcome::Failed,
        ),
        (
            ConvState::CreationCancelled {
                job_id: "create".into(),
            },
            TerminalFactOutcome::Cancelled,
        ),
        (
            ConvState::RecoverableContinuationFailure {
                failure: RecoverableContinuationFailure {
                    request: ContinuationSummaryRequest {
                        operation_id: "summary".into(),
                        rejected_tool_calls: Vec::new(),
                        attempt: 1,
                    },
                    message: "summary failed".into(),
                    error_kind: ErrorKind::ServerError,
                },
            },
            TerminalFactOutcome::Failed,
        ),
    ];
    for (state, outcome) in states {
        let f = Fixture::new().await;
        f.emit(
            "expired",
            TerminalFactOutcome::Completed,
            NOW - MAX_DISCONNECTED_AGE_US - 1,
        )
        .await;
        f.db.update_conversation_state(&f.source.id, &state)
            .await
            .unwrap();
        let SubscriptionReplay::Gap { gap } = f.replay(0, NOW).await else {
            panic!("expired payload requires gap")
        };
        assert_eq!(
            gap.snapshot.state,
            SubscriptionCurrentState::Terminal { outcome }
        );
        f.db.update_conversation_state(&f.source.id, &ConvState::Idle)
            .await
            .unwrap();
        let SubscriptionReplay::Gap { gap } = f.replay(0, NOW).await else {
            panic!("gap remains outstanding")
        };
        assert_eq!(
            gap.snapshot.state,
            SubscriptionCurrentState::Nonterminal,
            "resuming clears the stale terminal classification"
        );
    }
}

#[tokio::test]
async fn local_unsubscribe_fences_inflight_confirmation_after_stop_barrier() {
    let f = Fixture::new().await;
    f.emit("in-flight", TerminalFactOutcome::Completed, NOW)
        .await;
    let (db, remote, target) = consumer_for_source(&f).await;
    db.stage_remote_replay(&remote, &f.replay(0, NOW).await)
        .await
        .unwrap();
    let note = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    db.request_remote_notification_authorization(&remote, &note.admission_id)
        .await
        .unwrap();
    let authorized = authorize_source(&f, &note.content).await;
    let SubscriptionAuthorization::Authorized { authorization_id } = authorized else {
        panic!("source authorized")
    };
    let (stable, source) = fixture_navigation(&db, &remote, &note.admission_id).await;
    let stopped = tokio::sync::Barrier::new(2);
    let stop = async {
        db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
            .await
            .unwrap();
        stopped.wait().await;
    };
    let confirm = async {
        stopped.wait().await;
        assert!(db
            .confirm_remote_authorization(
                &remote,
                &note.admission_id,
                authorization_id,
                &stable,
                &source
            )
            .await
            .is_err());
        assert!(steer(&db, &target, &note.admission_id).await.is_err());
    };
    tokio::join!(stop, confirm);
    assert!(db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .is_empty());
    let current = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.admitted_cursor, SubscriptionCursor::START);
    assert!(current.pending_unsubscribe);
    assert!(db
        .stage_remote_replay(&remote, &f.replay(0, NOW).await)
        .await
        .is_err());
    assert!(db
        .request_remote_notification_authorization(&remote, &note.admission_id)
        .await
        .is_err());
    db.complete_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    assert!(db
        .next_remote_subscription_sync_batch(4)
        .await
        .unwrap()
        .is_empty());
    assert!(matches!(
        db.remote_notification_history(&remote, SubscriptionCursor::START)
            .await
            .unwrap()[0]
            .disposition,
        RemoteNotificationDisposition::Suppressed { .. }
    ));
}

#[tokio::test]
async fn revocation_settlement_suppresses_authorizing_work_and_clears_unsubscribe_retry() {
    let db = open_migrated(":memory:").await;
    let remote = remote_fixture(&db).await;
    let mut events = facts(single_fact());
    let mut second = events[0].clone();
    second.event_id = Uuid::new_v4();
    second.sequence = cursor(2).unwrap();
    second.source_transcript_id = TranscriptConversationId::parse("second-transcript").unwrap();
    events.push(second);
    db.stage_remote_replay(&remote, &SubscriptionReplay::Facts { events })
        .await
        .unwrap();
    let notes = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(notes.len(), 2);
    for note in &notes {
        db.request_remote_notification_authorization(&remote, &note.admission_id)
            .await
            .unwrap();
    }

    let wrong_reference = SubscriptionReference {
        instance_id: remote.reference.instance_id,
        product_conversation_id: ProductConversationId::parse("different-product").unwrap(),
    };
    assert!(db
        .settle_remote_subscription_revocation(&wrong_reference, remote.subscription_id)
        .await
        .is_err());
    assert_eq!(
        db.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SubscriptionState::Active
    );
    assert!(db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .iter()
        .all(|note| matches!(note.disposition, RemoteNotificationDisposition::Authorizing)));

    db.settle_remote_subscription_revocation(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    let revoked = db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revoked.state, SubscriptionState::Revoked);
    assert!(!revoked.pending_unsubscribe);
    assert!(db
        .remote_notification_history(&revoked, SubscriptionCursor::START)
        .await
        .unwrap()
        .iter()
        .all(|note| matches!(
            note.disposition,
            RemoteNotificationDisposition::Suppressed { .. }
        )));
    assert!(db
        .next_remote_subscription_sync_batch(4)
        .await
        .unwrap()
        .is_empty());

    let pending = remote_fixture(&db).await;
    db.begin_remote_unsubscribe(&pending.reference, pending.subscription_id)
        .await
        .unwrap();
    db.complete_remote_unsubscribe(&pending.reference, pending.subscription_id)
        .await
        .unwrap();
    let unsubscribed = db
        .remote_subscription_by_id(pending.reference.instance_id, pending.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unsubscribed.state, SubscriptionState::Unsubscribed);
    assert!(!unsubscribed.pending_unsubscribe);
    assert!(db
        .next_remote_subscription_sync_batch(4)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn local_unsubscribe_serializes_with_accepted_admission_in_both_barrier_orders() {
    for accepted_first in [false, true] {
        let f = Fixture::new().await;
        f.emit("accept-stop-race", TerminalFactOutcome::Completed, NOW)
            .await;
        let (db, remote, target) = consumer_for_source(&f).await;
        db.stage_remote_replay(&remote, &f.replay(0, NOW).await)
            .await
            .unwrap();
        let note = db
            .pending_remote_notifications(&remote)
            .await
            .unwrap()
            .remove(0);
        db.request_remote_notification_authorization(&remote, &note.admission_id)
            .await
            .unwrap();
        let SubscriptionAuthorization::Authorized { authorization_id } =
            authorize_source(&f, &note.content).await
        else {
            panic!("source authorized")
        };
        let (stable, source) = fixture_navigation(&db, &remote, &note.admission_id).await;
        db.confirm_remote_authorization(
            &remote,
            &note.admission_id,
            authorization_id,
            &stable,
            &source,
        )
        .await
        .unwrap();
        let first_done = tokio::sync::Barrier::new(2);
        let admit = async {
            if !accepted_first {
                first_done.wait().await;
            }
            let result = steer(&db, &target, &note.admission_id).await;
            assert_eq!(result.is_ok(), accepted_first);
            if accepted_first {
                first_done.wait().await;
            }
        };
        let stop = async {
            if accepted_first {
                first_done.wait().await;
            }
            db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
                .await
                .unwrap();
            if !accepted_first {
                first_done.wait().await;
            }
        };
        tokio::join!(admit, stop);
        assert!(db
            .pending_remote_notifications(&remote)
            .await
            .unwrap()
            .is_empty());
        let history = db
            .remote_notification_history(&remote, SubscriptionCursor::START)
            .await
            .unwrap();
        assert_eq!(
            matches!(
                history[0].disposition,
                RemoteNotificationDisposition::Accepted { .. }
            ),
            accepted_first
        );
        assert_eq!(
            matches!(
                history[0].disposition,
                RemoteNotificationDisposition::Suppressed { .. }
            ),
            !accepted_first
        );
        let current = db
            .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.admitted_cursor.get(), i64::from(accepted_first));
        assert!(current.pending_unsubscribe);
    }
}

#[tokio::test]
async fn drain_1025_with_gap_and_authorization_keeps_other_subscription_independent() {
    let f = Fixture::new().await;
    let other_caller = InstanceId::new();
    let other_verifier = FederationCredentialVerifier::from_bearer(b"other-drain");
    f.db.replace_federation_enrollment(other_caller, "other", &other_verifier)
        .await
        .unwrap();
    let other_authority = send_authority(other_caller, other_verifier);
    let other_subscription = Uuid::new_v4();
    f.db.create_federation_subscription(
        &other_authority,
        other_subscription,
        &f.source.product_conversation_id,
        NOW,
    )
    .await
    .unwrap();
    let mut tx = f.db.pool.begin().await.unwrap();
    for n in 0..1025 {
        append_terminal_tx(
            &mut tx,
            ClassifiedTerminal {
                occurrence_kind: "direct_turn",
                occurrence_id: &format!("drain-{n}"),
                generation: 0,
                transcript_id: &f.source.id,
                outcome: TerminalFactOutcome::Completed,
                context_exhausted: false,
                occurred_at_us: NOW + n,
            },
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let (a, mut remote, target) = consumer_for_source(&f).await;
    let mut admitted_rows = 0;
    while remote.admitted_cursor.get() < 1025 {
        let replay = f.replay(remote.admitted_cursor.get(), NOW + 1025).await;
        a.stage_remote_replay(&remote, &replay).await.unwrap();
        let notes = a.pending_remote_notifications(&remote).await.unwrap();
        assert!(!notes.is_empty());
        for note in notes {
            a.request_remote_notification_authorization(&remote, &note.admission_id)
                .await
                .unwrap();
            let SubscriptionAuthorization::Authorized { authorization_id } =
                authorize_source(&f, &note.content).await
            else {
                panic!("active source authorizes retained bytes")
            };
            let (stable, source) = fixture_navigation(&a, &remote, &note.admission_id).await;
            a.confirm_remote_authorization(
                &remote,
                &note.admission_id,
                authorization_id,
                &stable,
                &source,
            )
            .await
            .unwrap();
            steer(&a, &target, &note.admission_id).await.unwrap();
            admitted_rows += 1;
        }
        remote = a
            .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(admitted_rows, 1025);
    let other =
        f.db.replay_federation_subscription(
            &other_authority,
            other_subscription,
            &f.source.product_conversation_id,
            SubscriptionCursor::START,
            NOW + 1025,
        )
        .await
        .unwrap();
    assert!(matches!(other,SubscriptionReplay::Gap { gap } if gap.last_produced.get() == 1025));
    let progress: (i64,i64) = sqlx::query_as("SELECT acknowledged_through,(SELECT COUNT(*) FROM federation_delivery_authorizations WHERE subscription_id = ?1) FROM federation_subscriptions WHERE id = ?1").bind(other_subscription.to_string()).fetch_one(f.db.pool()).await.unwrap();
    assert_eq!(progress, (0, 0));
}

#[tokio::test]
async fn actual_local_and_remote_stable_id_collision_never_reinterprets_remote_fact_as_local_watch()
{
    use phoenix_core::domain::sm_state::ConvState;
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let local = db
        .create_conversation("colliding-transcript", "local", "/tmp", true, None, None)
        .await
        .unwrap();
    db.watch_product_conversation(&local.product_conversation_id)
        .await
        .unwrap();
    let reference = SubscriptionReference {
        instance_id: InstanceId::new(),
        product_conversation_id: local.product_conversation_id.clone(),
    };
    let subscription = Uuid::new_v4();
    db.save_remote_subscription(&reference, subscription)
        .await
        .unwrap();
    let remote = db
        .remote_subscription_for_product(&reference)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        remote.reference.product_conversation_id,
        local.product_conversation_id
    );
    let turn = source_turn(&db, &local.id, "local-only").await;
    db.workflow_repository()
        .terminalize_authoritative_turn(&terminal_input(
            turn,
            ConvState::Idle,
            TerminalFactOutcome::Completed,
        ))
        .await
        .unwrap();
    let local_event: String = sqlx::query_scalar("SELECT event_id FROM coordinator_watch_events")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let SubscriptionReplay::Facts { mut events } = single_fact() else {
        unreachable!()
    };
    events[0].source_transcript_id = TranscriptConversationId::parse(&local.id).unwrap();
    db.stage_remote_replay(&remote, &SubscriptionReplay::Facts { events })
        .await
        .unwrap();
    let note = db
        .pending_remote_notifications(&remote)
        .await
        .unwrap()
        .remove(0);
    fixture_authorize(&db, &remote, &note.admission_id).await;
    steer(&db, &target, &note.admission_id).await.unwrap();
    let accepted_local: Option<String> = sqlx::query_scalar(
        "SELECT accepted_transcript_id FROM coordinator_watch_events WHERE event_id = ?1",
    )
    .bind(&local_event)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(accepted_local.is_none());
    steer(&db, &target, &local_event).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM coordinator_watch_events WHERE accepted_transcript_id IS NOT NULL"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        db.remote_subscription_by_id(reference.instance_id, subscription)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        1
    );
}

async fn creation_claim_fixture(
    f: &Fixture,
) -> phoenix_core::domain::creation_protocol::CreationClaim {
    use phoenix_core::domain::creation_protocol::{
        CreationClaim, CreationClaimToken, CreationWorkerId,
    };
    let now = chrono::Utc::now();
    let lease = now + chrono::Duration::minutes(10);
    sqlx::query("INSERT INTO conversation_creation_jobs(id,conversation_id,status,stage,attempt,generation,claim_worker_id,claim_token,lease_until,intent_json,accepted_at,created_at,updated_at) VALUES (?1,?1,'claimed','bootstrap_initial_turn',1,1,'subscription-worker','subscription-claim',?2,?3,?4,?4,?4)")
        .bind(&f.source.id).bind(lease.to_rfc3339()).bind(crate::cleared_creation_intent_json()).bind(now.to_rfc3339()).execute(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(
        &f.source.id,
        &phoenix_core::domain::sm_state::ConvState::Provisioning {
            job_id: f.source.id.clone(),
            phase: phoenix_core::domain::db_schema::ConversationCreationPhase::Provisioning,
        },
    )
    .await
    .unwrap();
    CreationClaim {
        worker_id: CreationWorkerId("subscription-worker".into()),
        generation: 1,
        token: CreationClaimToken("subscription-claim".into()),
        lease_until: u64::try_from(lease.timestamp_millis()).unwrap(),
    }
}

#[tokio::test]
async fn production_creation_completion_failure_and_cancellation_fanout_are_atomic() {
    use phoenix_core::domain::{db_schema::ErrorKind, sm_state::ConvState};
    for outcome in [
        TerminalFactOutcome::Completed,
        TerminalFactOutcome::Failed,
        TerminalFactOutcome::Cancelled,
    ] {
        let f = Fixture::new().await;
        let claim = creation_claim_fixture(&f).await;
        sqlx::raw_sql("CREATE TRIGGER fail_creation_fanout BEFORE INSERT ON federation_terminal_payloads BEGIN SELECT RAISE(ABORT,'injected creation fanout failure'); END;").execute(f.db.pool()).await.unwrap();
        let first = match outcome {
            TerminalFactOutcome::Completed => {
                f.db.settle_conversation_creation_runtime(
                    &f.source.id,
                    &claim,
                    &f.source.id,
                    &ConvState::Idle,
                    chrono::Utc::now(),
                )
                .await
                .map(|_| ())
            }
            TerminalFactOutcome::Failed => {
                f.db.fail_conversation_creation_job(
                    &f.source.id,
                    &claim,
                    "provision failed",
                    &ErrorKind::ServerError,
                    chrono::Utc::now(),
                )
                .await
                .map(|_| ())
            }
            TerminalFactOutcome::Cancelled => {
                f.db.cancel_conversation_creation(&f.source.id, chrono::Utc::now())
                    .await
            }
        };
        assert!(first.is_err());
        assert!(matches!(
            f.db.get_conversation(&f.source.id).await.unwrap().state,
            ConvState::Provisioning { .. }
        ));
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM conversation_creation_jobs")
                .fetch_one(f.db.pool())
                .await
                .unwrap(),
            "claimed"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_occurrences")
                .fetch_one(f.db.pool())
                .await
                .unwrap(),
            0
        );
        sqlx::raw_sql("DROP TRIGGER fail_creation_fanout")
            .execute(f.db.pool())
            .await
            .unwrap();
        match outcome {
            TerminalFactOutcome::Completed => {
                f.db.settle_conversation_creation_runtime(
                    &f.source.id,
                    &claim,
                    &f.source.id,
                    &ConvState::Idle,
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
            }
            TerminalFactOutcome::Failed => {
                f.db.fail_conversation_creation_job(
                    &f.source.id,
                    &claim,
                    "provision failed",
                    &ErrorKind::ServerError,
                    chrono::Utc::now(),
                )
                .await
                .unwrap();
            }
            TerminalFactOutcome::Cancelled => {
                f.db.cancel_conversation_creation(&f.source.id, chrono::Utc::now())
                    .await
                    .unwrap();
            }
        }
        let events = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome, outcome);
    }
}

#[tokio::test]
async fn recovered_steering_installs_terminal_projection_before_atomic_fanout() {
    use phoenix_core::domain::{db_schema::MessageContent, sm_state::ConvState};
    let f = Fixture::new().await;
    f.db.add_message(
        "recovery-steering",
        &f.source.id,
        &MessageContent::user("steering"),
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO steering_execution_occurrences(conversation_id,message_id,source_kind) VALUES (?1,'recovery-steering','steering')").bind(&f.source.id).execute(f.db.pool()).await.unwrap();
    f.db.update_conversation_state(&f.source.id, &ConvState::LlmRequesting { attempt: 1 })
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TRIGGER fail_recovery_fanout BEFORE INSERT ON federation_terminal_payloads BEGIN SELECT RAISE(ABORT,'injected recovery fanout failure'); END;").execute(f.db.pool()).await.unwrap();
    assert!(f
        .db
        .settle_execution_occurrence(
            &f.source.id,
            "recovery-steering",
            &crate::ExecutionOccurrenceTerminal::Completed,
            &ConvState::Idle
        )
        .await
        .is_err());
    assert!(matches!(
        f.db.get_conversation(&f.source.id).await.unwrap().state,
        ConvState::LlmRequesting { .. }
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM steering_execution_occurrences")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1
    );
    sqlx::raw_sql("DROP TRIGGER fail_recovery_fanout")
        .execute(f.db.pool())
        .await
        .unwrap();
    assert!(f
        .db
        .settle_execution_occurrence(
            &f.source.id,
            "recovery-steering",
            &crate::ExecutionOccurrenceTerminal::Completed,
            &ConvState::Idle
        )
        .await
        .unwrap());
    let events = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, TerminalFactOutcome::Completed);
}

#[tokio::test]
async fn real_summary_failure_survives_when_successful_handoff_suppresses_candidate() {
    use phoenix_core::domain::{
        db_schema::ErrorKind,
        sm_state::{ContinuationSummaryRequest, ConvState, RecoverableContinuationFailure},
    };
    let f = Fixture::new().await;
    let request = ContinuationSummaryRequest {
        operation_id: "real-summary".into(),
        rejected_tool_calls: Vec::new(),
        attempt: 1,
    };
    f.db.update_conversation_state(
        &f.source.id,
        &ConvState::AwaitingContinuation {
            request: request.clone(),
        },
    )
    .await
    .unwrap();
    assert!(facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await).is_empty());
    f.db.update_conversation_state(
        &f.source.id,
        &ConvState::RecoverableContinuationFailure {
            failure: RecoverableContinuationFailure {
                request,
                error_kind: ErrorKind::ServerError,
                message: "summary provider failed".into(),
            },
        },
    )
    .await
    .unwrap();
    let failure = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
    assert_eq!(failure.len(), 1);
    continuation_candidate(&f).await;
    let (outcome, intent) =
        f.db.continue_conversation_with_intent(
            &f.source.id,
            crate::NewContinuationDispatchIntent::generated_predecessor_context(
                ClientTurnKey::new("federation-successor-first").unwrap(),
                "source summary".into(),
            ),
        )
        .await
        .unwrap();
    let intent = intent.expect("continuation dispatch intent");
    let crate::ContinueOutcome::Created(successor) = outcome else {
        panic!("successor required")
    };
    f.db.add_message(
        intent.message_id.as_str(),
        &successor.id,
        &crate::MessageContent::continuation("source summary"),
        None,
        None,
    )
    .await
    .unwrap();
    f.db.update_conversation_state(
        &f.source.id,
        &ConvState::HandedOff {
            successor_conv_id: successor.id,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1
    );
    let remaining = facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].event_id, failure[0].event_id);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM federation_terminal_occurrences WHERE occurrence_kind = 'continuation_summary'").fetch_one(f.db.pool()).await.unwrap(),1);
}

#[tokio::test]
async fn real_creation_wait_token_and_cancelled_cleanup_paths_never_add_terminal_facts() {
    use phoenix_core::domain::{db_schema::MessageContent, sm_state::ConvState};
    let f = Fixture::new().await;
    let claim = creation_claim_fixture(&f).await;
    let wait = ConvState::AwaitingTaskApproval {
        task_file: "tasks/approval.md".into(),
        title: "approval".into(),
        priority: phoenix_core::task_source::Priority::P2,
        plan: "pending approval".into(),
    };
    f.db.settle_conversation_creation_runtime(
        &f.source.id,
        &claim,
        &f.source.id,
        &wait,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert!(facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await).is_empty());
    f.db.add_message(
        "nonterminal-output",
        &f.source.id,
        &MessageContent::agent(vec![phoenix_core::domain::llm_types::ContentBlock::text(
            "intermediate output is not completion",
        )]),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await).is_empty());
    let cancelled = Fixture::new().await;
    creation_claim_fixture(&cancelled).await;
    cancelled
        .db
        .cancel_conversation_creation(&cancelled.source.id, chrono::Utc::now())
        .await
        .unwrap();
    assert_eq!(
        facts(
            cancelled
                .replay(0, chrono::Utc::now().timestamp_micros())
                .await
        )
        .len(),
        1
    );
    let now = chrono::Utc::now() + chrono::Duration::seconds(1);
    let cleanup = cancelled
        .db
        .claim_next_conversation_creation_cleanup(
            "cleanup-worker",
            "cleanup-claim",
            now,
            chrono::Duration::minutes(1),
        )
        .await
        .unwrap()
        .expect("cancelled creation requires cleanup");
    cancelled
        .db
        .finish_conversation_creation_cleanup(&cleanup, now)
        .await
        .unwrap();
    assert_eq!(
        facts(cancelled.replay(0, now.timestamp_micros()).await).len(),
        1
    );
}
