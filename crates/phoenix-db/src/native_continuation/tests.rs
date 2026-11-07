use super::*;
use crate::{
    workflow::{AcceptAuthoritativeTurn, LocalAuthorityResult, WorkflowRepository},
    ConvState, MessageContent, ProductConversationId,
};
use phoenix_core::domain::sm_event::{
    PreparedDirectTurnDelivery, PreparedDirectTurnPayload, SteerEntry,
    SubmittedDirectTurnExpansionPolicy, SubmittedDirectTurnIdentity,
};
use phoenix_core::{
    domain::{
        db_schema::InputOrigin,
        instance_identity::{FederationCredentialVerifier, InstanceId},
    },
    llm_language::LlmLanguage,
};
use phoenix_workflow::{AcceptedDisposition, ConversationAuthority, PreparedTurn};

const SUMMARY: &str = "generated summary for continuation";

struct Fixture {
    db: Database,
    remote_source: FederatedContinuationSource,
    source_id: String,
    predecessor_id: String,
    product_id: ProductConversationId,
    summary_message_id: String,
}

impl Fixture {
    async fn new() -> Self {
        Self::on(
            Database::open_in_memory().await.unwrap(),
            "continuation-predecessor",
        )
        .await
    }

    async fn on(db: Database, predecessor_id: &str) -> Self {
        let coordinator = db
            .get_or_create_coordinator(None, LlmLanguage::default())
            .await
            .unwrap();
        let predecessor = db
            .create_conversation(predecessor_id, predecessor_id, "/tmp", true, None, None)
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
        let summary_message_id = format!("{predecessor_id}-summary");
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

        let remote_source = FederatedContinuationSource {
            destination_instance_id: db.instance_id().await.unwrap(),
            product_conversation_id: ProductConversationId::new(),
            transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
                "remote-coordinator",
            )
            .unwrap(),
            message_id: "remote-message".into(),
            tool_use_id: "remote-tool".into(),
        };
        Self {
            db,
            remote_source,
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

    fn federated_request<'a>(
        &'a self,
        authority: &'a crate::FederationSendAuthority,
        first_message_id: &str,
    ) -> FederatedCoordinatorContinuationRequest<'a> {
        FederatedCoordinatorContinuationRequest {
            authority,
            source: &self.remote_source,
            product_conversation_id: &self.product_id,
            predecessor_transcript_id: &self.predecessor_id,
            summary_message_id: &self.summary_message_id,
            first_message_id: ClientTurnKey::try_from(first_message_id).unwrap(),
        }
    }

    async fn federated_authority(&self) -> (InstanceId, crate::FederationSendAuthority) {
        let caller = InstanceId::new();
        let bearer = format!("native-continuation-test-{caller}");
        let verifier = FederationCredentialVerifier::from_bearer(bearer.as_bytes());
        self.db
            .replace_federation_enrollment(caller, &"peer".parse().unwrap(), &verifier)
            .await
            .unwrap();
        (
            caller,
            crate::FederationSendAuthority::new(caller, verifier),
        )
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
async fn pending_continuation_identities_conflict_globally_in_every_federation_journal() {
    use crate::federation_enrollment::{
        require_federation_send_identity_owner_tx, FederationSendIdentityJournal,
    };
    for remote in [false, true] {
        let fixture = Fixture::new().await;
        let (_, authority) = fixture.federated_authority().await;
        let key = "pending-global-identity";
        let first = if remote {
            fixture
                .db
                .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
                .await
        } else {
            fixture
                .db
                .continue_from_current_coordinator(fixture.request(key))
                .await
        };
        assert!(matches!(
            first,
            LocalAuthorityResult::DurableFactEstablished(Ok(_))
        ));
        let other = Fixture::on(fixture.db.clone(), "second-pending-boundary").await;
        let mut attempt = other.federated_request(&authority, key);
        attempt.source = &fixture.remote_source;
        assert!(matches!(
            fixture
                .db
                .continue_from_federated_coordinator(attempt)
                .await,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::FederationSendIdentityConflict
            ))
        ));
        assert_eq!(other.successor_count().await, 0);
        for journal in [
            FederationSendIdentityJournal::DirectTurn,
            FederationSendIdentityJournal::SteeringReceipt,
            FederationSendIdentityJournal::Continuation,
        ] {
            let mut tx = fixture.db.pool().begin().await.unwrap();
            assert!(matches!(
                require_federation_send_identity_owner_tx(
                    &mut tx,
                    key,
                    &other.predecessor_id,
                    journal
                )
                .await,
                Err(DbError::FederationSendIdentityConflict)
            ));
            tx.rollback().await.unwrap();
        }
        let receipt_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM federated_continuation_receipts")
                .fetch_one(fixture.db.pool())
                .await
                .unwrap();
        assert_eq!(receipt_count, i64::from(remote));
    }
}

#[tokio::test]
async fn completed_local_continuation_identity_cannot_be_claimed_by_federation() {
    use crate::federation_enrollment::{
        require_federation_send_identity_owner_tx, FederationSendIdentityJournal,
    };

    let fixture = Fixture::new().await;
    let (_, authority) = fixture.federated_authority().await;
    let key = "completed-local-global-identity";
    let result = fixture
        .db
        .continue_from_current_coordinator(fixture.request(key))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((ContinueOutcome::Created(successor), _))) =
        result
    else {
        panic!("expected local continuation reservation");
    };
    fixture
        .db
        .add_message_with_seq_and_origin(
            key,
            &successor.id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();

    assert!(matches!(
        fixture
            .db
            .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::FederationSendIdentityConflict))
    ));
    for journal in [
        FederationSendIdentityJournal::DirectTurn,
        FederationSendIdentityJournal::SteeringReceipt,
        FederationSendIdentityJournal::Continuation,
    ] {
        let mut tx = fixture.db.pool().begin().await.unwrap();
        assert!(matches!(
            require_federation_send_identity_owner_tx(
                &mut tx,
                key,
                &fixture.predecessor_id,
                journal
            )
            .await,
            Err(DbError::FederationSendIdentityConflict)
        ));
        tx.rollback().await.unwrap();
    }
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM federated_continuation_receipts")
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    assert_eq!(receipt_count, 0);
}

#[allow(
    clippy::too_many_lines,
    reason = "the mutation matrix must share one established receipt and replay boundary"
)]
#[tokio::test]
async fn federated_receipt_binds_all_attestation_fields_before_and_after_intent_deletion() {
    let fixture = Fixture::new().await;
    let (caller, authority) = fixture.federated_authority().await;
    let key = "immutable-payload-identity";
    let first = fixture
        .db
        .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((ContinueOutcome::Created(successor), _))) =
        first
    else {
        panic!("expected first reservation");
    };
    for accepted in [false, true] {
        if accepted {
            fixture
                .db
                .add_message_with_seq_and_origin(
                    key,
                    &successor.id,
                    1,
                    &MessageContent::continuation(SUMMARY),
                    None,
                    None,
                    &InputOrigin::SystemGenerated,
                )
                .await
                .unwrap();
            fixture
                .db
                .delete_continuation_dispatch_intent(&fixture.predecessor_id)
                .await
                .unwrap();
            assert!(fixture
                .db
                .continuation_dispatch_intent(&fixture.predecessor_id)
                .await
                .unwrap()
                .is_none());
        }
        for field in 0..5 {
            let mut source = fixture.remote_source.clone();
            match field {
                0 => source.product_conversation_id = ProductConversationId::new(),
                1 => {
                    source.transcript_id =
                        phoenix_core::domain::close::TranscriptConversationId::parse(
                            "changed-source",
                        )
                        .unwrap();
                }
                2 => source.message_id = "changed-message".into(),
                3 => source.tool_use_id = "changed-tool".into(),
                4 => source.destination_instance_id = InstanceId::new(),
                _ => unreachable!(),
            }
            let mut request = fixture.federated_request(&authority, key);
            request.source = &source;
            let result = fixture
                .db
                .continue_from_federated_coordinator(request)
                .await;
            assert!(
                matches!(
                    result,
                    LocalAuthorityResult::DurableFactEstablished(Err(
                        DbError::FederationSendIdentityConflict
                    ))
                ),
                "source field {field}, accepted={accepted}: {result:?}"
            );
        }
        let changed_product = ProductConversationId::new();
        let changed_predecessor = "changed-predecessor".to_string();
        let changed_summary = "changed-summary".to_string();
        for (field, request) in [
            (
                "product",
                FederatedCoordinatorContinuationRequest {
                    product_conversation_id: &changed_product,
                    ..fixture.federated_request(&authority, key)
                },
            ),
            (
                "predecessor",
                FederatedCoordinatorContinuationRequest {
                    predecessor_transcript_id: &changed_predecessor,
                    ..fixture.federated_request(&authority, key)
                },
            ),
            (
                "summary",
                FederatedCoordinatorContinuationRequest {
                    summary_message_id: &changed_summary,
                    ..fixture.federated_request(&authority, key)
                },
            ),
        ] {
            let result = fixture
                .db
                .continue_from_federated_coordinator(request)
                .await;
            assert!(
                matches!(
                    result,
                    LocalAuthorityResult::DurableFactEstablished(Err(
                        DbError::FederationSendIdentityConflict
                    ))
                ),
                "boundary field {field}, accepted={accepted}: {result:?}"
            );
        }
        let replay = fixture
            .db
            .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
            .await;
        assert!(matches!(
            replay,
            LocalAuthorityResult::DurableFactEstablished(Ok((
                ContinueOutcome::AlreadyContinued(_),
                _
            )))
        ));
    }
    let (_, other_authority) = fixture.federated_authority().await;
    let result = fixture
        .db
        .continue_from_federated_coordinator(fixture.federated_request(&other_authority, key))
        .await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::FederationSendIdentityConflict))
    ));
    fixture
        .db
        .revoke_federation_enrollment(caller)
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .db
            .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
            .await,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::FederationSendAuthorityInactive))
    ));
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn federated_receipt_digest_is_immutable_and_rejects_changed_summary_content() {
    let fixture = Fixture::new().await;
    let (_, authority) = fixture.federated_authority().await;
    let key = "content-bound-identity";
    assert!(matches!(
        fixture
            .db
            .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
            .await,
        LocalAuthorityResult::DurableFactEstablished(Ok(_))
    ));
    assert!(
        sqlx::query("UPDATE federated_continuation_receipts SET request_digest = ?1")
            .bind("a".repeat(64))
            .execute(fixture.db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM federated_continuation_receipts")
        .execute(fixture.db.pool())
        .await
        .is_err());
    fixture
        .db
        .update_conversation_state(
            &fixture.predecessor_id,
            &ConvState::ContextExhausted {
                summary: "altered summary".into(),
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET content = ?1 WHERE message_id = ?2")
        .bind(serde_json::json!({"summary": "altered summary"}).to_string())
        .bind(&fixture.summary_message_id)
        .execute(fixture.db.pool())
        .await
        .unwrap();
    let replay = fixture
        .db
        .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
        .await;
    assert!(
        matches!(
            replay,
            LocalAuthorityResult::DurableFactEstablished(Err(
                DbError::FederationSendIdentityConflict
            ))
        ),
        "{replay:?}"
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the cross-journal invariant requires one identity and one transaction history"
)]
#[tokio::test]
async fn continuation_identity_is_reserved_across_federated_direct_and_steering_journals() {
    let fixture = Fixture::new().await;
    let (_, authority) = fixture.federated_authority().await;
    let key = "federated-cross-journal-identity";
    let first = fixture
        .db
        .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((
        ContinueOutcome::Created(successor),
        CoordinatorContinuationOpening::Pending(opening),
    ))) = first
    else {
        panic!("expected federated continuation with a pending opening");
    };

    let direct_payload = PreparedDirectTurnPayload::from_parts(
        SubmittedDirectTurnIdentity {
            origin: InputOrigin::UnknownHistorical,
            text: "direct attempt".into(),
            images: Vec::new(),
            files: Vec::new(),
            message_id: key.into(),
            user_agent: None,
            skill_invocation: None,
            expansion_policy: SubmittedDirectTurnExpansionPolicy::ExpandReferences,
        },
        PreparedDirectTurnDelivery {
            text: "direct attempt".into(),
            llm_text: None,
            images: Vec::new(),
            files: Vec::new(),
            user_agent: None,
            skill_invocation: None,
        },
    );
    let target = ConversationAuthority(fixture.predecessor_id.clone());
    let direct = AcceptAuthoritativeTurn {
        client_key: ClientTurnKey::try_from(key).unwrap(),
        prepared: PreparedTurn::from_exact_payload(
            &target,
            direct_payload.to_exact_bytes().unwrap(),
        ),
        disposition: AcceptedDisposition::Runtime,
        accepted_at: phoenix_workflow::Timestamp(2),
    };
    let repo = WorkflowRepository::new(fixture.db.pool().clone());
    let steering = SteerEntry {
        origin: InputOrigin::UnknownHistorical,
        text: "steering attempt".into(),
        llm_text: None,
        images: Vec::new(),
        files: Vec::new(),
        message_id: key.into(),
        user_agent: None,
        skill_invocation: None,
    };

    let direct_result = repo
        .accept_federated_authoritative_turn(&direct, &authority)
        .await
        .established()
        .expect("direct-turn admission durability should be classified");
    assert!(matches!(
        direct_result,
        Err(DbError::FederationSendIdentityConflict)
    ));
    assert!(matches!(
        fixture
            .db
            .append_federated_steering_entry(
                &fixture.predecessor_id,
                &steering,
                "cross-journal-steering-fingerprint",
                &authority,
            )
            .await,
        Err(DbError::FederationSendIdentityConflict)
    ));

    let direct_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns WHERE client_turn_key = ?1")
            .bind(key)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    let steering_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM steering_acceptance_receipts WHERE message_id = ?1",
    )
    .bind(key)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    let steering_message_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM steering_messages WHERE message_id = ?1")
            .bind(key)
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    let receipt_before: (String, String, String, String) = sqlx::query_as(
        "SELECT caller_instance_id, request_digest, predecessor_transcript_id, successor_transcript_id
         FROM federated_continuation_receipts WHERE first_message_id = ?1",
    )
    .bind(key)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(direct_count, 0);
    assert_eq!(steering_count, 0);
    assert_eq!(steering_message_count, 0);
    assert_eq!(fixture.successor_count().await, 1);

    let replay = fixture
        .db
        .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((
        ContinueOutcome::AlreadyContinued(replayed_successor),
        CoordinatorContinuationOpening::Pending(replayed_opening),
    ))) = replay
    else {
        panic!("expected the original federated continuation to replay");
    };
    assert_eq!(replayed_successor.id, successor.id);
    assert_eq!(replayed_opening, opening);
    let receipt_after: (String, String, String, String) = sqlx::query_as(
        "SELECT caller_instance_id, request_digest, predecessor_transcript_id, successor_transcript_id
         FROM federated_continuation_receipts WHERE first_message_id = ?1",
    )
    .bind(key)
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(receipt_after, receipt_before);
    assert_eq!(fixture.successor_count().await, 1);
}

#[tokio::test]
async fn federated_continuation_rechecks_active_authority_and_replays_exact_winner() {
    let fixture = Fixture::new().await;
    let (_, authority) = fixture.federated_authority().await;

    let first = fixture
        .db
        .continue_from_federated_coordinator(
            fixture.federated_request(&authority, "federated-opening-key"),
        )
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((first_outcome, first_opening))) = first
    else {
        panic!("expected established federated continuation");
    };
    assert!(matches!(first_outcome, ContinueOutcome::Created(_)));
    let CoordinatorContinuationOpening::Pending(first_opening) = first_opening else {
        panic!("expected pending federated opening");
    };
    assert_eq!(first_opening.message_id.as_str(), "federated-opening-key");

    let replay = fixture
        .db
        .continue_from_federated_coordinator(
            fixture.federated_request(&authority, "federated-opening-key"),
        )
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((replay_outcome, replay_opening))) = replay
    else {
        panic!("expected established federated replay");
    };
    assert!(matches!(
        replay_outcome,
        ContinueOutcome::AlreadyContinued(_)
    ));
    let CoordinatorContinuationOpening::Pending(replay_opening) = replay_opening else {
        panic!("expected replay to return the existing pending opening");
    };
    assert_eq!(replay_opening.message_id, first_opening.message_id);

    let changed = fixture
        .db
        .continue_from_federated_coordinator(
            fixture.federated_request(&authority, "different-retry-key"),
        )
        .await;
    assert!(matches!(
        changed,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));
    assert_eq!(fixture.successor_count().await, 1);
    let pending = fixture
        .db
        .read_continuation_opening(&first_opening.successor_conversation_id)
        .await
        .unwrap();
    assert!(
        matches!(pending, crate::ContinuationOpeningRead::Pending { client_turn_key, .. }
        if client_turn_key == first_opening.message_id)
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the boundary replay checks must reuse one accepted opening identity"
)]
#[tokio::test]
async fn federated_accepted_opening_key_cannot_cross_continuation_boundaries() {
    let fixture = Fixture::new().await;
    let (_, authority) = fixture.federated_authority().await;
    let key = "globally-bound-opening-key";
    let first = fixture
        .db
        .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((
        ContinueOutcome::Created(successor),
        CoordinatorContinuationOpening::Pending(_),
    ))) = first
    else {
        panic!("expected established opening");
    };
    fixture
        .db
        .add_message_with_seq_and_origin(
            key,
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
        .continue_from_federated_coordinator(fixture.federated_request(&authority, key))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == key
    ));

    let changed_key = fixture
        .db
        .continue_from_federated_coordinator(
            fixture.federated_request(&authority, "different-accepted-key"),
        )
        .await;
    assert!(matches!(
        changed_key,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::ContinuationPrecondition(_)))
    ));

    let second = fixture
        .db
        .create_conversation(
            "other-predecessor",
            "other-predecessor",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    fixture
        .db
        .update_conversation_state(
            &second.id,
            &ConvState::ContextExhausted {
                summary: SUMMARY.to_string(),
            },
        )
        .await
        .unwrap();
    fixture
        .db
        .add_message_with_seq_and_origin(
            "other-summary",
            &second.id,
            1,
            &MessageContent::continuation(SUMMARY),
            None,
            None,
            &InputOrigin::SystemGenerated,
        )
        .await
        .unwrap();
    let result = fixture
        .db
        .continue_from_federated_coordinator(FederatedCoordinatorContinuationRequest {
            authority: &authority,
            source: &fixture.remote_source,
            product_conversation_id: &second.product_conversation_id,
            predecessor_transcript_id: &second.id,
            summary_message_id: "other-summary",
            first_message_id: ClientTurnKey::try_from(key).unwrap(),
        })
        .await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::FederationSendIdentityConflict))
    ));
    assert!(fixture
        .db
        .get_conversation(&second.id)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());
    assert_eq!(fixture.successor_count().await, 1);
    let intents: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM continuation_dispatch_intents WHERE parent_conversation_id = ?1",
    )
    .bind(&second.id)
    .fetch_one(&fixture.db.pool)
    .await
    .unwrap();
    assert_eq!(intents, 0);
}

#[tokio::test]
async fn federated_continuation_rejects_revoked_authority_without_mutation() {
    let fixture = Fixture::new().await;
    let (caller, authority) = fixture.federated_authority().await;
    fixture
        .db
        .revoke_federation_enrollment(caller)
        .await
        .unwrap();

    let result = fixture
        .db
        .continue_from_federated_coordinator(
            fixture.federated_request(&authority, "revoked-opening-key"),
        )
        .await;
    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactEstablished(Err(DbError::FederationSendAuthorityInactive))
    ));
    assert_eq!(fixture.successor_count().await, 0);
    assert!(fixture
        .db
        .get_conversation(&fixture.predecessor_id)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());
}

#[tokio::test]
async fn local_generated_continuation_replays_losing_key_without_replacing_winner() {
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
        .continue_from_current_coordinator(fixture.request("first-opening-key"))
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
    let changed = fixture
        .db
        .continue_from_current_coordinator(fixture.request("different-retry-key"))
        .await;
    assert!(matches!(
        changed,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Pending(ref intent),
        ))) if intent.message_id == winner.message_id
    ));
    assert_eq!(fixture.successor_count().await, 1);
    let pending = fixture
        .db
        .read_continuation_opening(&winner.successor_conversation_id)
        .await
        .unwrap();
    assert!(
        matches!(pending, crate::ContinuationOpeningRead::Pending { client_turn_key, .. }
        if client_turn_key == winner.message_id)
    );
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
        .continue_from_coordinator_transaction(
            ContinuationReservation {
                admission: ContinuationAdmission::LocalCurrentCoordinator(&fixture.source_id),
                product_conversation_id: &fixture.product_id,
                predecessor_transcript_id: &fixture.predecessor_id,
                summary_message_id: &fixture.summary_message_id,
                first_message_id: ClientTurnKey::try_from("durable-opening-key").unwrap(),
            },
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
        .continue_from_current_coordinator(fixture.request("durable-opening-key"))
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
        .continue_from_current_coordinator(fixture.request("accepted-opening-key"))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == "accepted-opening-key"
    ));
    let changed = fixture
        .db
        .continue_from_current_coordinator(fixture.request("retry-opening-key"))
        .await;
    assert!(matches!(
        changed,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == "accepted-opening-key"
    ));
    assert_eq!(fixture.successor_count().await, 1);
    let accepted = fixture
        .db
        .read_continuation_opening(&successor.id)
        .await
        .unwrap();
    assert!(
        matches!(accepted, crate::ContinuationOpeningRead::Accepted { accepted_message_id, .. }
        if accepted_message_id == "accepted-opening-key")
    );
}

#[tokio::test]
async fn canonical_accepted_message_id_replays_only_its_original_key() {
    let fixture = Fixture::new().await;
    let first = fixture
        .db
        .continue_from_current_coordinator(fixture.request("canonical-opening-key"))
        .await;
    let LocalAuthorityResult::DurableFactEstablished(Ok((ContinueOutcome::Created(successor), _))) =
        first
    else {
        panic!("expected generated continuation to create a successor");
    };
    let canonical_id = format!("{}:canonical-opening-key", successor.id);
    fixture
        .db
        .add_message_with_seq_and_origin(
            &canonical_id,
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
        .continue_from_current_coordinator(fixture.request("canonical-opening-key"))
        .await;
    assert!(matches!(
        replay,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == canonical_id
    ));
    let changed = fixture
        .db
        .continue_from_current_coordinator(fixture.request("different-opening-key"))
        .await;
    assert!(matches!(
        changed,
        LocalAuthorityResult::DurableFactEstablished(Ok((
            ContinueOutcome::AlreadyContinued(_),
            CoordinatorContinuationOpening::Accepted { accepted_message_id },
        ))) if accepted_message_id == canonical_id
    ));
    assert_eq!(fixture.successor_count().await, 1);
    let accepted = fixture
        .db
        .read_continuation_opening(&successor.id)
        .await
        .unwrap();
    assert!(
        matches!(accepted, crate::ContinuationOpeningRead::Accepted { accepted_message_id, .. }
        if accepted_message_id == canonical_id)
    );
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
        .continue_from_current_coordinator(fixture.request("matching-residual-key"))
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
