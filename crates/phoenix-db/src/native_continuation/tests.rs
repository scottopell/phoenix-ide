use super::*;
use crate::{workflow::LocalAuthorityResult, ConvState, MessageContent, ProductConversationId};
use phoenix_core::{domain::db_schema::InputOrigin, llm_language::LlmLanguage};

const SUMMARY: &str = "generated summary for continuation";

struct Fixture {
    db: Database,
    source_id: String,
    predecessor_id: String,
    product_id: ProductConversationId,
    summary_message_id: String,
}

impl Fixture {
    async fn new() -> Self {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let predecessor = db
            .create_conversation(
                "continuation-predecessor",
                "continuation-predecessor",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        db.update_conversation_state(
            &predecessor.id,
            &ConvState::ContextExhausted {
                summary: SUMMARY.to_string(),
            },
        )
        .await
        .unwrap();
        let summary_message_id = "continuation-summary".to_string();
        db.add_message_with_seq_and_origin(
            &summary_message_id,
            &predecessor.id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();

        Self {
            db,
            source_id: coordinator.id,
            predecessor_id: predecessor.id,
            product_id: predecessor.product_conversation_id,
            summary_message_id,
        }
    }

    fn request(&self, first_message_id: &str) -> CurrentCoordinatorContinuationRequest<'_> {
        CurrentCoordinatorContinuationRequest {
            source_transcript_id: &self.source_id,
            product_conversation_id: &self.product_id,
            predecessor_transcript_id: &self.predecessor_id,
            summary_message_id: &self.summary_message_id,
            first_message_id: ClientTurnKey::try_from(first_message_id).unwrap(),
        }
    }

    async fn successor_count(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM conversations predecessor
             JOIN conversations successor
               ON predecessor.continued_in_conv_id = successor.id
              AND predecessor.product_conversation_id = successor.product_conversation_id
             WHERE predecessor.id = ?1",
        )
        .bind(&self.predecessor_id)
        .fetch_one(&self.db.pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn generated_continuation_replay_returns_exact_winner_not_retry_key() {
    let fixture = Fixture::new().await;

    let first = fixture
        .db
        .continue_from_current_coordinator(fixture.request("first-opening-key"))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((
        ContinueOutcome::Created(_),
        CoordinatorContinuationOpening::Pending(winner),
    ))) = first
    else {
        panic!("expected an established generated continuation with a pending opening");
    };
    assert_eq!(winner.message_id.as_str(), "first-opening-key");

    let replay = fixture
        .db
        .continue_from_current_coordinator(fixture.request("different-retry-key"))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((
        ContinueOutcome::AlreadyContinued(_),
        CoordinatorContinuationOpening::Pending(replayed),
    ))) = replay
    else {
        panic!("expected replay to return the existing pending opening");
    };
    assert_eq!(replayed.message_id, winner.message_id);
    assert_eq!(replayed.handoff, SUMMARY);
    assert_eq!(
        replayed.opening_authority,
        phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext
    );
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn stale_source_is_rejected_before_existing_continuation_replay() {
    let fixture = Fixture::new().await;
    let first = fixture
        .db
        .continue_from_current_coordinator(fixture.request("first-opening-key"))
        .await;
    assert!(matches!(
        first,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));

    let mut stale = fixture.request("retry-opening-key");
    stale.source_transcript_id = "not-the-current-coordinator";
    let replay = fixture.db.continue_from_current_coordinator(stale).await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::CurrentCoordinatorRequired))
    ));
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn lost_commit_acknowledgement_replays_the_durable_winner() {
    let fixture = Fixture::new().await;
    let result = fixture
        .db
        .continue_from_current_coordinator_with_commit(
            fixture.request("durable-opening-key"),
            ContinuationCommit::LoseAcknowledgement,
        )
        .await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(fixture.successor_count().await, 1);

    let replay = fixture
        .db
        .continue_from_current_coordinator(fixture.request("new-retry-key"))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((
        _,
        CoordinatorContinuationOpening::Pending(winner),
    ))) = replay
    else {
        panic!("expected replay to observe the committed pending opening");
    };
    assert_eq!(winner.message_id.as_str(), "durable-opening-key");
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn failed_precondition_rolls_back_without_creating_successor() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .update_conversation_state(&fixture.predecessor_id, &ConvState::Idle)
        .await
        .unwrap();

    let result = fixture
        .db
        .continue_from_current_coordinator(fixture.request("opening-key"))
        .await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);
}

#[tokio::test]
async fn mismatched_summary_message_rolls_back_without_creating_successor() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .add_message_with_seq_and_origin(
            "different-summary",
            &fixture.predecessor_id,
            2,
            &MessageContent::continuation("not the state summary"),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();
    let mut request = fixture.request("opening-key");
    request.summary_message_id = "different-summary";

    let result = fixture.db.continue_from_current_coordinator(request).await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);
}

#[tokio::test]
async fn exact_continuation_text_with_user_api_origin_is_rejected() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .add_message_with_seq_and_origin(
            "user-origin-summary",
            &fixture.predecessor_id,
            2,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::UserApi,
        )
        .await
        .unwrap();
    let mut request = fixture.request("opening-key");
    request.summary_message_id = "user-origin-summary";

    let result = fixture.db.continue_from_current_coordinator(request).await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);
}

#[tokio::test]
async fn summary_message_from_another_transcript_is_rejected() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .add_message_with_seq_and_origin(
            "other-transcript-summary",
            &fixture.source_id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();
    let mut request = fixture.request("opening-key");
    request.summary_message_id = "other-transcript-summary";

    let result = fixture.db.continue_from_current_coordinator(request).await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);
}

#[tokio::test]
async fn older_equal_text_boundary_is_rejected_but_newest_boundary_succeeds() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .add_message_with_seq_and_origin(
            "newer-continuation-summary",
            &fixture.predecessor_id,
            2,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();

    let mut older = fixture.request("older-opening-key");
    older.summary_message_id = &fixture.summary_message_id;
    let rejected = fixture.db.continue_from_current_coordinator(older).await;
    assert!(matches!(
        rejected,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);

    let mut newer = fixture.request("newer-opening-key");
    newer.summary_message_id = "newer-continuation-summary";
    let accepted = fixture.db.continue_from_current_coordinator(newer).await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((ContinueOutcome::Created(successor), _))) =
        accepted
    else {
        panic!("expected the newest continuation boundary to create a successor");
    };
    assert_eq!(successor.product_conversation_id, fixture.product_id);
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn closed_product_rejects_existing_pending_opening_replay() {
    let fixture = Fixture::new().await;
    let first = fixture
        .db
        .continue_from_current_coordinator(fixture.request("pending-opening-key"))
        .await;
    assert!(matches!(
        first,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1")
        .bind(fixture.product_id.as_str())
        .execute(&fixture.db.pool)
        .await
        .unwrap();

    let replay = fixture
        .db
        .continue_from_current_coordinator(fixture.request("retry-opening-key"))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ProductConversationUnavailable(
            _
        )))
    ));
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn completed_generated_replay_returns_persisted_accepted_message_id() {
    let fixture = Fixture::new().await;
    let first = fixture
        .db
        .continue_from_current_coordinator(fixture.request("accepted-opening-key"))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((ContinueOutcome::Created(successor), _))) =
        first
    else {
        panic!("expected generated continuation to create a successor");
    };
    fixture
        .db
        .add_message_with_seq_and_origin(
            "accepted-opening-key",
            &successor.id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();

    let replay = fixture
        .db
        .continue_from_current_coordinator(fixture.request("retry-opening-key"))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == "accepted-opening-key"
    ));
}

async fn insert_residual_generated_intent(fixture: &Fixture, successor_id: &str, message_id: &str) {
    sqlx::query(
        "INSERT INTO continuation_dispatch_intents (
            parent_conversation_id, successor_conversation_id, message_id, handoff,
            user_agent, opening_authority, created_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, 'generated_predecessor_context', CURRENT_TIMESTAMP)",
    )
    .bind(&fixture.predecessor_id)
    .bind(successor_id)
    .bind(message_id)
    .bind(SUMMARY)
    .execute(&fixture.db.pool)
    .await
    .unwrap();
}

async fn complete_generated_opening(fixture: &Fixture, message_id: &str) -> String {
    let first = fixture
        .db
        .continue_from_current_coordinator(fixture.request(message_id))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((ContinueOutcome::Created(successor), _))) =
        first
    else {
        panic!("expected generated continuation to create a successor");
    };
    fixture
        .db
        .add_message_with_seq_and_origin(
            message_id,
            &successor.id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();
    successor.id
}

#[tokio::test]
async fn completed_receipt_with_matching_residual_intent_replays_as_accepted() {
    let fixture = Fixture::new().await;
    let successor_id = complete_generated_opening(&fixture, "matching-residual-key").await;
    insert_residual_generated_intent(&fixture, &successor_id, "matching-residual-key").await;

    let replay = fixture
        .db
        .continue_from_current_coordinator(fixture.request("retry-opening-key"))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == "matching-residual-key"
    ));
}

#[tokio::test]
async fn completed_receipt_with_conflicting_residual_intent_is_rejected() {
    let fixture = Fixture::new().await;
    let successor_id = complete_generated_opening(&fixture, "completed-opening-key").await;
    insert_residual_generated_intent(&fixture, &successor_id, "conflicting-residual-key").await;

    let replay = fixture
        .db
        .continue_from_current_coordinator(fixture.request("retry-opening-key"))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::Serialization(_)))
    ));
}

#[tokio::test]
async fn requested_product_must_match_predecessor_product() {
    let fixture = Fixture::new().await;
    let other_product = fixture
        .db
        .create_conversation("other-product", "other-product", "/tmp", true, None, None)
        .await
        .unwrap()
        .product_conversation_id;
    let mut request = fixture.request("opening-key");
    request.product_conversation_id = &other_product;

    let result = fixture.db.continue_from_current_coordinator(request).await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);
}

#[tokio::test]
async fn coordinator_product_cannot_be_used_as_ordinary_continuation_target() {
    let fixture = Fixture::new().await;
    let coordinator = fixture
        .db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap();
    let mut request = fixture.request("opening-key");
    request.product_conversation_id = &coordinator.product_conversation_id;

    let result = fixture.db.continue_from_current_coordinator(request).await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ConversationNotFound(_)))
    ));
    assert_eq!(fixture.successor_count().await, 0);
}
