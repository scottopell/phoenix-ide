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

const NOW: i64 = RETENTION_US * 2;
struct Fixture {
    db: Database,
    source: crate::Conversation,
    caller: InstanceId,
    enrollment: Uuid,
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
        let enrollment = db
            .replace_federation_enrollment(caller, "caller", &verifier)
            .await
            .unwrap()
            .enrollment_id;
        let subscription = Uuid::new_v4();
        db.create_federation_subscription(
            enrollment,
            caller,
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
            subscription,
        }
    }
    async fn emit(&self, occurrence: &str, outcome: TerminalFactOutcome, now: i64) {
        let mut tx = self.db.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        append_terminal_tx(
            &mut tx,
            ClassifiedTerminal {
                occurrence_kind: "creation",
                occurrence_id: occurrence,
                generation: 0,
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
                self.enrollment,
                self.caller,
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

#[tokio::test]
async fn future_only_exact_identity_replay_ack_and_compact_dedupe() {
    let mut f = Fixture::new().await;
    f.emit("before", TerminalFactOutcome::Completed, NOW).await;
    let first = facts(f.replay(0, NOW).await);
    assert_eq!(first.len(), 1);
    assert_eq!(first, facts(f.replay(0, NOW).await));
    assert_eq!(first[0].source_transcript_id.as_str(), f.source.id);
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
    f.db.unsubscribe_federation_subscription(
        f.enrollment,
        f.caller,
        f.subscription,
        &f.source.product_conversation_id,
    )
    .await
    .unwrap();
    f.subscription = Uuid::new_v4();
    f.db.create_federation_subscription(
        f.enrollment,
        f.caller,
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
async fn payload_1025_produces_gap_with_snapshot_and_resume_and_never_reintroduces_pruned_fact() {
    let f = Fixture::new().await;
    for n in 0..1025 {
        f.emit(
            &format!("occurrence-{n}"),
            TerminalFactOutcome::Completed,
            NOW + n,
        )
        .await;
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1024
    );
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + 1025).await else {
        panic!("retention gap required")
    };
    assert_eq!(gap.resume_after.get(), 1);
    assert_eq!(gap.snapshot.transcript_id.as_str(), f.source.id);
    assert_eq!(
        gap.snapshot.state,
        SubscriptionCurrentState::Terminal {
            outcome: TerminalFactOutcome::Completed
        }
    );
    let batch = facts(f.replay(1, NOW + 1025).await);
    assert_eq!(batch.len(), REPLAY_BATCH);
    assert_eq!(batch[0].sequence.get(), 2);
    f.emit("occurrence-0", TerminalFactOutcome::Completed, NOW + 1026)
        .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_identities")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        1025
    );
}

#[tokio::test]
async fn expiry_is_logical_at_exact_thirty_days_with_injected_clock() {
    let f = Fixture::new().await;
    f.emit("expires", TerminalFactOutcome::Cancelled, NOW).await;
    assert_eq!(facts(f.replay(0, NOW + RETENTION_US - 1).await).len(), 1);
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + RETENTION_US).await else {
        panic!("expired history must not replay")
    };
    assert_eq!(gap.resume_after.get(), 1);
    assert_eq!(
        gap.snapshot.state,
        SubscriptionCurrentState::Terminal {
            outcome: TerminalFactOutcome::Cancelled
        }
    );
    assert!(facts(f.replay(1, NOW + RETENTION_US).await).is_empty());
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
            f.enrollment,
            InstanceId::new(),
            absent,
            &source.product_conversation_id,
        )
        .await
        .is_err());
    for _ in 0..2 {
        f.db.unsubscribe_federation_subscription(
            f.enrollment,
            f.caller,
            absent,
            &source.product_conversation_id,
        )
        .await
        .unwrap();
    }
    assert!(f
        .db
        .unsubscribe_federation_subscription(
            f.enrollment,
            f.caller,
            absent,
            &ProductConversationId::parse("different-source").unwrap(),
        )
        .await
        .is_err());
    assert!(f
        .db
        .create_federation_subscription(
            f.enrollment,
            f.caller,
            absent,
            &source.product_conversation_id,
            NOW,
        )
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
            f.enrollment,
            InstanceId::new(),
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
            Uuid::new_v4(),
            f.caller,
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
    let replacement =
        f.db.replace_federation_enrollment(f.caller, "replacement", &verifier)
            .await
            .unwrap();
    assert_ne!(replacement.enrollment_id, f.enrollment);
    let state: String = sqlx::query_scalar("SELECT state FROM federation_subscriptions")
        .fetch_one(f.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "revoked");
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
            replacement.enrollment_id,
            f.caller,
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
    assert!(f
        .db
        .create_federation_subscription(
            f.enrollment,
            f.caller,
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
        crate::conversation_goals::control::GoalControlInstallation::Disabled,
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
            crate::conversation_goals::control::GoalControlInstallation::Disabled,
        )
        .await
        .unwrap();
    }
    crate::coordinator_watches::record_terminal_event_tx(
        &mut tx,
        crate::coordinator_watches::TerminalEventSource {
            turn_id: 32,
            generation: 0,
        },
        &f.source.id,
        "Failed",
        None,
        false,
        crate::conversation_goals::control::GoalControlInstallation::Disabled,
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
        crate::conversation_goals::control::GoalControlInstallation::Disabled,
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
    let c = db
        .create_conversation(
            "federation-coordinator",
            "coordinator",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE product_conversations SET kind = 'coordinator', ordinary_lifecycle = NULL WHERE id = ?1").bind(c.product_conversation_id.as_str()).execute(db.pool()).await.unwrap();
    sqlx::query(
        "UPDATE conversations SET runtime_role = 'coordinator', coordinator_head = 1 WHERE id = ?1",
    )
    .bind(&c.id)
    .execute(db.pool())
    .await
    .unwrap();
    c.id
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
fn single_fact() -> SubscriptionReplay {
    SubscriptionReplay::Facts {
        events: vec![FederatedTerminalFact {
            event_id: Uuid::new_v4(),
            sequence: cursor(1).unwrap(),
            source_transcript_id: TranscriptConversationId::parse("exact-remote-transcript")
                .unwrap(),
            outcome: TerminalFactOutcome::Failed,
            occurred_at_us: NOW,
        }],
    }
}
async fn fixture_navigation(db: &Database, remote: &RemoteSubscription, id: &str) {
    let transcript: String = sqlx::query_scalar("SELECT COALESCE(f.source_transcript_id,g.source_transcript_id) FROM federation_remote_admissions a LEFT JOIN federation_received_facts f USING(admission_id) LEFT JOIN federation_received_gaps g USING(admission_id) WHERE a.admission_id = ?1")
        .bind(id).fetch_one(db.pool()).await.unwrap();
    let product =
        presentation::encoded_component(remote.reference.product_conversation_id.as_str());
    let transcript = presentation::encoded_component(&transcript);
    db.save_remote_notification_navigation(
        remote,
        id,
        &format!("/c/{product}"),
        &format!("/c/{transcript}?source_transcript={transcript}"),
    )
    .await
    .unwrap();
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
    fixture_navigation(db, remote, id).await;
    db.confirm_remote_authorization(remote, id, Uuid::new_v4())
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
async fn gap_is_not_a_fact_and_cursor_moves_only_when_notification_is_accepted() {
    let db = Database::open_in_memory().await.unwrap();
    let target = coordinator(&db).await;
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    let gap = SubscriptionGap {
        requested_after: SubscriptionCursor::START,
        resume_after: cursor(12).unwrap(),
        snapshot: SubscriptionSnapshot {
            transcript_id: TranscriptConversationId::parse("continued-exact-head").unwrap(),
            state: SubscriptionCurrentState::Nonterminal,
        },
    };
    db.stage_remote_replay(&remote, &SubscriptionReplay::Gap { gap: gap.clone() })
        .await
        .unwrap();
    let pending = db.pending_remote_notifications(&remote).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        matches!(&pending[0].content,RemoteNotificationContent::Gap { gap: stored } if stored == &gap)
    );
    assert_eq!(
        db.remote_subscription_for_product(&remote.reference)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        0
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
        12
    );
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
    let db = Database::open(&path).await.unwrap();
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
    let db = Database::open(&path).await.unwrap();
    db.stage_remote_replay(&remote, &replay).await.unwrap();
    assert_eq!(
        db.pending_remote_notifications(&remote).await.unwrap()[0].admission_id,
        id
    );
    fixture_authorize(&db, &remote, &id).await;
    steer(&db, &target, &id).await.unwrap();
    db.pool.close().await;
    let db = Database::open(&path).await.unwrap();
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

async fn authorize_source(
    f: &Fixture,
    delivery: &SubscriptionDelivery,
) -> SubscriptionAuthorization {
    f.db.authorize_federation_delivery(
        f.enrollment,
        f.caller,
        f.subscription,
        &f.source.product_conversation_id,
        delivery,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn cancel_close_restores_gap_authorization_without_ending_subscription() {
    let f = Fixture::new().await;
    f.emit("expired-before-close", TerminalFactOutcome::Completed, NOW)
        .await;
    let SubscriptionReplay::Gap { gap } = f.replay(0, NOW + RETENTION_US).await else {
        panic!("expired payload must produce a gap");
    };
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
        NOW + RETENTION_US + 1,
    )
    .await;
    let future = facts(f.replay(1, NOW + RETENTION_US + 1).await);
    assert_eq!(future.len(), 1);
    assert!(matches!(
        authorize_source(
            &f,
            &SubscriptionDelivery::Fact {
                fact: future[0].clone()
            }
        )
        .await,
        SubscriptionAuthorization::Authorized { .. }
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
    let enrollment = db
        .replace_federation_enrollment(
            caller,
            "later peer",
            &FederationCredentialVerifier::from_bearer(b"later-peer"),
        )
        .await
        .unwrap()
        .enrollment_id;
    let f = Fixture {
        db,
        source,
        caller,
        enrollment,
        subscription: Uuid::new_v4(),
    };
    f.db.create_federation_subscription(
        f.enrollment,
        f.caller,
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
async fn real_history_close_preserves_accepted_and_authorized_but_suppresses_uncommitted_bytes() {
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
            fixture_navigation(&a, &remote, &note.admission_id).await;
            a.confirm_remote_authorization(&remote, &note.admission_id, authorization_id)
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
    let SubscriptionAuthorization::Authorized { authorization_id } =
        authorize_source(&f, &pending[1].content).await
    else {
        panic!("committed obligation must survive Close")
    };
    fixture_navigation(&a, &remote, &pending[1].admission_id).await;
    a.confirm_remote_authorization(&remote, &pending[1].admission_id, authorization_id)
        .await
        .unwrap();
    steer(&a, &target, &pending[1].admission_id).await.unwrap();
    assert_eq!(
        authorize_source(&f, &pending[2].content).await,
        SubscriptionAuthorization::Rejected {
            reason: SubscriptionEnd::SourceClosed
        }
    );
    a.suppress_remote_notification(&remote, &pending[2].admission_id)
        .await
        .unwrap();
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
        2
    );
    assert!(matches!(
        history[2].disposition,
        RemoteNotificationDisposition::Suppressed
    ));
    assert_eq!(
        a.remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
            .await
            .unwrap()
            .unwrap()
            .admitted_cursor
            .get(),
        2
    );
    assert!(matches!(
        f.replay(2, NOW + 3).await,
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
            0
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
                    f.enrollment,
                    f.caller,
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
async fn unsubscribe_identity_is_durable_retryable_and_never_reenrolls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("unsubscribe.sqlite")
        .to_string_lossy()
        .into_owned();
    let db = Database::open(&path).await.unwrap();
    let remote = remote_fixture(&db).await;
    db.stage_remote_replay(&remote, &single_fact())
        .await
        .unwrap();
    db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    db.pool.close().await;
    let db = Database::open(&path).await.unwrap();
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
    let fresh = f.replay(0, NOW + RETENTION_US).await;
    assert_eq!(
        authorize_source(&f, &note.content).await,
        SubscriptionAuthorization::Refresh
    );
    a.suppress_remote_notification(&remote, &note.admission_id)
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
async fn received_navigation_is_canonical_durable_and_batched_before_authorization() {
    let db = Database::open(":memory:").await.unwrap();
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
    assert!(db
        .confirm_remote_authorization(&remote, &note.admission_id, Uuid::new_v4())
        .await
        .is_err());
    for path in [
        "//attacker.invalid/c/p",
        "/c/p?credential=secret",
        "/c/../p",
        "/c/p#secret",
    ] {
        assert!(db
            .save_remote_notification_navigation(
                &remote,
                &note.admission_id,
                path,
                "/c/t?source_transcript=t"
            )
            .await
            .is_err());
    }
    fixture_navigation(&db, &remote, &note.admission_id).await;
    db.confirm_remote_authorization(&remote, &note.admission_id, Uuid::new_v4())
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
            NOW - RETENTION_US - 1,
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
async fn staged_authorization_attempt_survives_local_unsubscribe_and_lost_reply() {
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
    db.begin_remote_unsubscribe(&remote.reference, remote.subscription_id)
        .await
        .unwrap();
    f.db.unsubscribe_federation_subscription(
        f.enrollment,
        f.caller,
        f.subscription,
        &f.source.product_conversation_id,
    )
    .await
    .unwrap();
    assert_eq!(authorize_source(&f, &note.content).await, authorized);
    assert!(matches!(
        db.pending_remote_notifications(&remote).await.unwrap()[0].disposition,
        RemoteNotificationDisposition::Authorizing
    ));
    let SubscriptionAuthorization::Authorized { authorization_id } = authorized else {
        panic!("source authorized")
    };
    fixture_navigation(&db, &remote, &note.admission_id).await;
    db.confirm_remote_authorization(&remote, &note.admission_id, authorization_id)
        .await
        .unwrap();
    steer(&db, &target, &note.admission_id).await.unwrap();
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
        RemoteNotificationDisposition::Accepted { .. }
    ));
}

#[tokio::test]
async fn drain_1025_with_gap_and_authorization_keeps_other_subscription_independent() {
    let f = Fixture::new().await;
    let other_caller = InstanceId::new();
    let other_enrollment =
        f.db.replace_federation_enrollment(
            other_caller,
            "other",
            &FederationCredentialVerifier::from_bearer(b"other-drain"),
        )
        .await
        .unwrap()
        .enrollment_id;
    let other_subscription = Uuid::new_v4();
    f.db.create_federation_subscription(
        other_enrollment,
        other_caller,
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
            fixture_navigation(&a, &remote, &note.admission_id).await;
            a.confirm_remote_authorization(&remote, &note.admission_id, authorization_id)
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
            other_enrollment,
            other_caller,
            other_subscription,
            &f.source.product_conversation_id,
            SubscriptionCursor::START,
            NOW + 1025,
        )
        .await
        .unwrap();
    assert!(matches!(other,SubscriptionReplay::Gap { gap } if gap.resume_after.get() == 1));
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
async fn real_summary_failure_is_distinct_and_successful_handoff_suppresses_uncommitted_summary() {
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
    assert_eq!(
        facts(f.replay(0, chrono::Utc::now().timestamp_micros()).await).len(),
        1
    );
    f.db.update_conversation_state(
        &f.source.id,
        &ConvState::HandedOff {
            successor_conv_id: "summary-successor".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(f.db.pool())
            .await
            .unwrap(),
        0
    );
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
