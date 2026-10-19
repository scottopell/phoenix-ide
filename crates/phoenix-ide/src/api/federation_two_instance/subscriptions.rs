use super::*;
use crate::api::federation::subscriptions::*;
use phoenix_core::domain::{
    federation_subscription::*, product_conversation::ProductConversationId,
};
use uuid::Uuid;

async fn source(instance: &TestInstance) -> phoenix_db::Conversation {
    instance
        .state
        .db
        .create_conversation("remote-terminal-source", "source", "/tmp", true, None, None)
        .await
        .unwrap()
}
async fn commit_terminal(
    instance: &TestInstance,
    source: &phoenix_db::Conversation,
    outcome: TerminalFactOutcome,
) {
    use crate::send_chat_service::{
        MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
    };
    use phoenix_core::domain::{
        db_schema::InputOrigin, provider_replay::ProviderReplaySettlement, sm_state::ConvState,
    };
    use phoenix_db::workflow::{
        PersistedConversationProjection, TerminalizeAuthoritativeTurnInput,
    };
    use phoenix_workflow::{ConversationAuthority, TurnCommand, TurnOutcome};
    let result =
        SendChatApplicationService::new(instance.state.db.clone(), instance.state.runtime.clone())
            .send(SendChatRequest {
                conversation_id: source.id.clone(),
                origin: InputOrigin::UserApi,
                message_id: Uuid::new_v4().to_string(),
                text: "execute a bounded source turn".into(),
                images: vec![],
                files: vec![],
                user_agent: None,
                expansion_policy: MessageExpansionPolicy::LiteralText,
            })
            .await
            .unwrap();
    assert_eq!(result, SendChatOutcome::Delivered);
    super::send::materialize_received(instance, &source.id).await;
    let repo = instance.state.db.workflow_repository();
    let turn = repo
        .load_active_runtime_turn(&ConversationAuthority(source.id.clone()))
        .await
        .unwrap()
        .unwrap();
    let command = match outcome {
        TerminalFactOutcome::Completed => TurnCommand::Complete {
            turn_id: turn.id,
            expected_generation: turn.generation,
        },
        TerminalFactOutcome::Failed => TurnCommand::Fail {
            turn_id: turn.id,
            expected_generation: turn.generation,
            reason: "bounded test failure".into(),
        },
        TerminalFactOutcome::Cancelled => TurnCommand::Cancel {
            turn_id: turn.id,
            expected_generation: turn.generation,
        },
    };
    let step = repo
        .terminalize_authoritative_turn(&TerminalizeAuthoritativeTurnInput {
            command,
            projection: Some(PersistedConversationProjection {
                state: ConvState::Idle,
                state_updated_at: chrono::Utc::now(),
            }),
            provider_replay_settlement: ProviderReplaySettlement::Preserve,
        })
        .await
        .unwrap();
    assert!(matches!(step.outcome, TurnOutcome::Terminal { .. }));
}

async fn refreshed(
    instance: &TestInstance,
    remote: &phoenix_db::federation_subscriptions::RemoteSubscription,
) -> phoenix_db::federation_subscriptions::RemoteSubscription {
    instance
        .state
        .db
        .remote_subscription_by_id(remote.reference.instance_id, remote.subscription_id)
        .await
        .unwrap()
        .unwrap()
}

async fn close_source(instance: &TestInstance, source: &phoenix_db::Conversation) {
    let _response = crate::api::product_conversations::close_product_conversation(
        axum::extract::State(instance.state.clone()),
        axum::extract::Path(source.product_conversation_id.to_string()),
    )
    .await
    .unwrap();
    let lifecycle: String =
        sqlx::query_scalar("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
            .bind(source.product_conversation_id.as_str())
            .fetch_one(instance.state.db.pool())
            .await
            .unwrap();
    assert_eq!(lifecycle, "history");
}

fn create_request(
    caller: InstanceId,
    destination: InstanceId,
    product: ProductConversationId,
    id: Uuid,
) -> SubscriptionRequest {
    SubscriptionRequest {
        caller_instance_id: caller,
        reference: SubscriptionReference {
            instance_id: destination,
            product_conversation_id: product,
        },
        subscription_id: id,
        operation: SubscriptionOperation::Create,
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn https_subscription_future_only_nonempty_idle_and_busy_admission_ack_and_replacement() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let connection = a.connect_to(&b).await;
    let caller = a.state.db.instance_id().await.unwrap();
    let destination = b.state.db.instance_id().await.unwrap();
    let source = source(&b).await;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let id = Uuid::new_v4();
    let req = create_request(
        caller,
        destination,
        source.product_conversation_id.clone(),
        id,
    );
    let create = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/create"))
        .bearer_auth(connection.bearer_credential.expose())
        .json(&req)
        .send()
        .await
        .unwrap();
    assert!(create.status().is_success());
    let created: SubscriptionResponse = create.json().await.unwrap();
    assert_eq!(created.reference, req.reference);
    let mut replay = req.clone();
    replay.operation = SubscriptionOperation::Replay {
        after: SubscriptionCursor::START,
    };
    let response: SubscriptionResponse = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/replay"))
        .bearer_auth(connection.bearer_credential.expose())
        .json(&replay)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        matches!(response.result,SubscriptionResult::Replay { replay: SubscriptionReplay::Facts { events } } if events.is_empty())
    );
    let local_collision = self::source(&a).await;
    assert_eq!(local_collision.id, source.id);
    assert_eq!(
        local_collision.product_conversation_id,
        source.product_conversation_id
    );
    a.state
        .db
        .watch_product_conversation(&local_collision.product_conversation_id)
        .await
        .unwrap();
    // The explicit saved peer must win even with a real same-ID local conversation and watch.
    let remote = create_saved_peer_subscription(
        &a.state.db,
        destination.to_string(),
        source.product_conversation_id.clone(),
        Some(id),
    )
    .await
    .unwrap();
    assert_eq!(remote.snapshot.transcript_id.as_str(), source.id);
    assert_eq!(
        remote.snapshot.state,
        SubscriptionCurrentState::Terminal {
            outcome: TerminalFactOutcome::Completed
        }
    );
    let remote = remote.subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let remote = refreshed(&a, &remote).await;
    assert_eq!(remote.admitted_cursor.get(), 1);
    let history = a
        .state
        .db
        .remote_notification_history(&remote, SubscriptionCursor::START)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    let event_id = &history[0].admission_id;
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM durable_turns WHERE origin_kind = 'subscription_event' AND origin_subscription_event_id = ?1").bind(event_id).fetch_one(a.state.db.pool()).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(b.state.db.pool())
            .await
            .unwrap(),
        1
    );
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(b.state.db.pool())
            .await
            .unwrap(),
        0
    );
    let coordinator = a
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    super::send::materialize_received(&a, &coordinator).await;
    commit_terminal(&b, &source, TerminalFactOutcome::Failed).await;
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let remote = refreshed(&a, &remote).await;
    assert_eq!(remote.admitted_cursor.get(), 2);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM steering_messages WHERE conversation_id = ?1 AND origin_kind = 'subscription_event'").bind(&coordinator).fetch_one(a.state.db.pool()).await.unwrap(), 1);
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(b.state.db.pool())
            .await
            .unwrap(),
        0
    );
    assert!(a
        .state
        .db
        .pending_coordinator_watch_events(10)
        .await
        .unwrap()
        .is_empty());
    b.state
        .db
        .revoke_federation_enrollment(caller)
        .await
        .unwrap();
    let revoked = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/replay"))
        .bearer_auth(connection.bearer_credential.expose())
        .json(&replay)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), reqwest::StatusCode::UNAUTHORIZED);
    let replacement = b.enroll(caller).await;
    assert_ne!(replacement.expose(), connection.bearer_credential.expose());
    let replaced = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/replay"))
        .bearer_auth(replacement.expose())
        .json(&replay)
        .send()
        .await
        .unwrap();
    assert_eq!(replaced.status(), reqwest::StatusCode::CONFLICT);
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn peer_auth_owner_separation_caller_reference_body_bounds_and_unsubscribe() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let caller = a.state.db.instance_id().await.unwrap();
    let destination = b.state.db.instance_id().await.unwrap();
    let source = source(&b).await;
    let req = create_request(
        caller,
        destination,
        source.product_conversation_id,
        Uuid::new_v4(),
    );
    let endpoint = b.endpoint("/api/federation/peer/subscriptions/create");
    assert_eq!(
        b.client
            .post(&endpoint)
            .bearer_auth("test-password")
            .json(&req)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let mut mismatch = req.clone();
    mismatch.caller_instance_id = InstanceId::new();
    assert_eq!(
        b.client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .json(&mismatch)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut mismatch = req.clone();
    mismatch.reference.instance_id = InstanceId::new();
    assert_eq!(
        b.client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .json(&mismatch)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        b.client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .header("content-type", "application/json")
            .body("x".repeat(REQUEST_BYTES + 1))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut unknown = serde_json::to_value(&req).unwrap();
    unknown["scheduler"] = serde_json::json!(true);
    assert_eq!(
        b.client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .json(&unknown)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(b
        .client
        .post(&endpoint)
        .bearer_auth(peer.bearer_credential.expose())
        .json(&req)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let mut unsubscribe = req.clone();
    unsubscribe.operation = SubscriptionOperation::Unsubscribe;
    assert!(b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/unsubscribe"))
        .bearer_auth(peer.bearer_credential.expose())
        .json(&unsubscribe)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let mut replay = req;
    replay.operation = SubscriptionOperation::Replay {
        after: SubscriptionCursor::START,
    };
    let response: SubscriptionResponse = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/replay"))
        .bearer_auth(peer.bearer_credential.expose())
        .json(&replay)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(matches!(
        response.result,
        SubscriptionResult::Replay {
            replay: SubscriptionReplay::Ended {
                reason: SubscriptionEnd::Unsubscribed
            }
        }
    ));
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn tls_trust_failure_and_explicit_missing_instance_never_fall_back_to_local_id() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let mut peer = a.connect_to(&b).await;
    let source = source(&b).await;
    peer.tls_trust = PeerTlsTrust::PlatformRoots;
    a.state
        .db
        .save_federation_peer_connection(
            peer.peer_instance_id,
            "wrong trust",
            &peer.base_url,
            &peer.bearer_credential,
            &peer.tls_trust,
        )
        .await
        .unwrap();
    assert!(create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None
    )
    .await
    .is_err());
    assert!(create_saved_peer_subscription(
        &a.state.db,
        InstanceId::new().to_string(),
        source.product_conversation_id,
        None
    )
    .await
    .is_err());
    assert!(a
        .state
        .db
        .list_coordinator_watches()
        .await
        .unwrap()
        .is_empty());
    a.stop().await;
    b.stop().await;
}

async fn stage_one(
    a: &TestInstance,
    b: &TestInstance,
    peer: &phoenix_db::FederationPeerConnection,
    remote: &phoenix_db::federation_subscriptions::RemoteSubscription,
) -> phoenix_db::federation_subscriptions::RemoteNotification {
    let request = SubscriptionRequest {
        caller_instance_id: a.state.db.instance_id().await.unwrap(),
        reference: remote.reference.clone(),
        subscription_id: remote.subscription_id,
        operation: SubscriptionOperation::Replay {
            after: remote.admitted_cursor,
        },
    };
    let response = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/replay"))
        .bearer_auth(peer.bearer_credential.expose())
        .json(&request)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let response: SubscriptionResponse = response.json().await.unwrap();
    let SubscriptionResult::Replay {
        replay: SubscriptionReplay::Facts { events },
    } = response.result
    else {
        panic!("nonempty terminal replay required")
    };
    assert_eq!(events.len(), 1);
    a.state
        .db
        .stage_remote_replay(remote, &SubscriptionReplay::Facts { events })
        .await
        .unwrap();
    let pending = a
        .state
        .db
        .pending_remote_notifications(remote)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    pending.into_iter().next().unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cancel_close_keeps_authorization_retryable_and_future_watch_active() {
    use phoenix_core::domain::close::{CloseCompletionOutcome, TranscriptConversationId};
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let pending = stage_one(&a, &b, &peer, &remote).await;
    let attempt = "cancellable-subscription-close";
    b.state
        .db
        .begin_close_foundation(
            &source.product_conversation_id,
            &TranscriptConversationId::parse(&source.id).unwrap(),
            attempt,
        )
        .await
        .unwrap();
    let response = request_peer(
        &peer,
        &SubscriptionRequest {
            caller_instance_id: a.state.db.instance_id().await.unwrap(),
            reference: remote.reference.clone(),
            subscription_id: remote.subscription_id,
            operation: SubscriptionOperation::Authorize {
                delivery: pending.content,
            },
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        response.result,
        SubscriptionResult::Authorization {
            authorization: SubscriptionAuthorization::Refresh,
            navigation: None,
        }
    ));
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let row = refreshed(&a, &remote).await;
    assert_eq!(row.state, SubscriptionState::Active);
    assert_eq!(row.admitted_cursor, SubscriptionCursor::START);
    let cancelled = b
        .state
        .db
        .cancel_close_before_retirement(attempt)
        .await
        .unwrap();
    assert_eq!(
        cancelled.close_outcome(),
        Some(CloseCompletionOutcome::Cancelled)
    );
    let lifecycle: String =
        sqlx::query_scalar("SELECT ordinary_lifecycle FROM product_conversations WHERE id = ?1")
            .bind(source.product_conversation_id.as_str())
            .fetch_one(b.state.db.pool())
            .await
            .unwrap();
    assert_eq!(lifecycle, "open");
    sync_once(&a.state.db, &a.state.runtime, &row)
        .await
        .unwrap();
    assert_eq!(refreshed(&a, &remote).await.admitted_cursor.get(), 1);
    commit_terminal(&b, &source, TerminalFactOutcome::Cancelled).await;
    sync_once(&a.state.db, &a.state.runtime, &refreshed(&a, &remote).await)
        .await
        .unwrap();
    let row = refreshed(&a, &remote).await;
    assert_eq!(row.state, SubscriptionState::Active);
    assert_eq!(
        row.admitted_cursor.get(),
        2,
        "a cancelled Close must not end future observation"
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn actual_close_before_authorization_suppresses_a_staged_nonempty_replay() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let pending = stage_one(&a, &b, &peer, &remote).await;
    close_source(&b, &source).await;
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let row = refreshed(&a, &remote).await;
    assert_eq!(row.state, SubscriptionState::SourceClosed);
    assert_eq!(row.admitted_cursor, SubscriptionCursor::START);
    let history = a
        .state
        .db
        .remote_notification_history(&row, SubscriptionCursor::START)
        .await
        .unwrap();
    assert!(matches!(
        history[0].disposition,
        phoenix_db::federation_subscriptions::RemoteNotificationDisposition::Suppressed
    ));
    assert_eq!(history[0].admission_id, pending.admission_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM durable_turns WHERE origin_kind = 'subscription_event'"
        )
        .fetch_one(a.state.db.pool())
        .await
        .unwrap(),
        0
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn actual_close_after_source_authorization_recovers_a_lost_receipt_and_finishes_once() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Cancelled).await;
    let pending = stage_one(&a, &b, &peer, &remote).await;
    a.state
        .db
        .request_remote_notification_authorization(&remote, &pending.admission_id)
        .await
        .unwrap();
    let authorization = SubscriptionRequest {
        caller_instance_id: a.state.db.instance_id().await.unwrap(),
        reference: remote.reference.clone(),
        subscription_id: remote.subscription_id,
        operation: SubscriptionOperation::Authorize {
            delivery: pending.content.clone(),
        },
    };
    let response: SubscriptionResponse = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/authorize"))
        .bearer_auth(peer.bearer_credential.expose())
        .json(&authorization)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(matches!(
        response.result,
        SubscriptionResult::Authorization {
            authorization: SubscriptionAuthorization::Authorized { .. },
            ..
        }
    ));
    // No local confirmation: exercise the durable authorizing/crash window, not just a completed send.
    close_source(&b, &source).await;
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let row = refreshed(&a, &remote).await;
    assert_eq!(row.admitted_cursor.get(), 1);
    assert_eq!(row.state, SubscriptionState::SourceClosed);
    sync_once(&a.state.db, &a.state.runtime, &row)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM durable_turns WHERE origin_subscription_event_id = ?1"
        )
        .bind(&pending.admission_id)
        .fetch_one(a.state.db.pool())
        .await
        .unwrap(),
        1
    );
    let coordinator = a
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    let message = super::send::materialize_received(&a, &coordinator).await;
    let presentation =
        crate::api::event_presentation::EventPresentations::load(&a.state.db, [&message])
            .await
            .for_message(&message);
    assert!(matches!(
        presentation,
        Some(crate::api::event_presentation::EventPresentation::FederatedFact(_))
    ));
    let redacted =
        crate::api::event_presentation::EventPresentations::redacted().for_message(&message);
    assert!(matches!(
        redacted,
        Some(
            crate::api::event_presentation::EventPresentation::Unavailable {
                reason:
                    crate::api::event_presentation::EventPresentationUnavailableReason::Redacted,
                ..
            }
        )
    ));
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn unsubscribe_before_create_reaches_source_confirms_exact_retry_and_fences_late_create() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let request = create_request(
        a.state.db.instance_id().await.unwrap(),
        peer.peer_instance_id,
        source.product_conversation_id.clone(),
        Uuid::new_v4(),
    );
    a.state
        .db
        .save_remote_subscription(&request.reference, request.subscription_id)
        .await
        .unwrap();
    let never_created: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM federation_subscriptions")
        .fetch_one(b.state.db.pool())
        .await
        .unwrap();
    assert_eq!(never_created, 0);
    let receipt = unsubscribe_saved_peer(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
    )
    .await
    .unwrap();
    assert!(receipt.local_stopped && receipt.remote_confirmed);
    assert_eq!(receipt.subscription_id, request.subscription_id);
    let local = a
        .state
        .db
        .remote_subscription_by_id(peer.peer_instance_id, request.subscription_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local.state, SubscriptionState::Unsubscribed);
    assert!(!local.pending_unsubscribe && !local.enrolled);
    let mut retry = request.clone();
    retry.operation = SubscriptionOperation::Unsubscribe;
    assert!(matches!(
        request_peer(&peer, &retry).await.unwrap().result,
        SubscriptionResult::Unsubscribed
    ));
    assert!(matches!(
        request_peer(&peer, &request).await,
        Err(crate::api::federation::FederationClientError::RemoteRejected(_))
    ));
    let reopened =
        phoenix_db::Database::open(b.tempdir.path().join("instance.db").to_str().unwrap())
            .await
            .unwrap();
    let fences: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM federation_subscription_create_fences WHERE id = ?1 AND product_id = ?2")
        .bind(request.subscription_id.to_string()).bind(source.product_conversation_id.as_str()).fetch_one(reopened.pool()).await.unwrap();
    assert_eq!(fences, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_subscriptions")
            .fetch_one(reopened.pool())
            .await
            .unwrap(),
        0
    );
    drop(reopened);
    let replacement = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    assert_ne!(replacement.subscription_id, request.subscription_id);
    assert_eq!(replacement.state, SubscriptionState::Active);
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn offline_unsubscribe_is_locally_stopped_and_retries_the_exact_remote_identity() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let pending = stage_one(&a, &b, &peer, &remote).await;
    let unavailable = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let offline_url: PeerBaseUrl = format!(
        "https://localhost:{}",
        unavailable.local_addr().unwrap().port()
    )
    .parse()
    .unwrap();
    drop(unavailable);
    a.state
        .db
        .save_federation_peer_connection(
            peer.peer_instance_id,
            "offline",
            &offline_url,
            &peer.bearer_credential,
            &peer.tls_trust,
        )
        .await
        .unwrap();
    let receipt = unsubscribe_saved_peer(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
    )
    .await
    .unwrap();
    assert!(receipt.local_stopped);
    assert!(!receipt.remote_confirmed);
    assert_eq!(receipt.subscription_id, remote.subscription_id);
    let stopped = refreshed(&a, &remote).await;
    assert!(stopped.pending_unsubscribe);
    assert_eq!(stopped.state, SubscriptionState::Unsubscribed);
    assert!(a
        .state
        .db
        .pending_remote_notifications(&stopped)
        .await
        .unwrap()
        .is_empty());
    assert!(create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None
    )
    .await
    .is_err());
    a.save_connection(&b, &peer.bearer_credential).await;
    sync_once(&a.state.db, &a.state.runtime, &stopped)
        .await
        .unwrap();
    assert!(!refreshed(&a, &remote).await.pending_unsubscribe);
    let state: String =
        sqlx::query_scalar("SELECT state FROM federation_subscriptions WHERE id = ?1")
            .bind(remote.subscription_id.to_string())
            .fetch_one(b.state.db.pool())
            .await
            .unwrap();
    assert_eq!(state, "unsubscribed");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM durable_turns WHERE origin_subscription_event_id = ?1"
        )
        .bind(pending.admission_id)
        .fetch_one(a.state.db.pool())
        .await
        .unwrap(),
        0
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn subscription_https_transport_rejects_malformed_oversized_redirect_and_timed_out_responses()
{
    use crate::api::federation::FederationClientError;
    use axum::response::IntoResponse;
    for mode in ["malformed", "oversized", "redirect", "timeout"] {
        let b = TestInstance::start_with_router(false, move |_| {
            axum::Router::new().route(
                "/api/federation/peer/subscriptions/create",
                axum::routing::post(move || async move {
                    match mode {
                        "malformed" => (axum::http::StatusCode::OK, "not json").into_response(),
                        "oversized" => (axum::http::StatusCode::OK, vec![b'x'; RESPONSE_BYTES + 1])
                            .into_response(),
                        "redirect" => (
                            axum::http::StatusCode::TEMPORARY_REDIRECT,
                            [(axum::http::header::LOCATION, "https://redirect.invalid/")],
                        )
                            .into_response(),
                        "timeout" => {
                            // test-timing-allow: the delayed response must exceed the client deadline
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            axum::http::StatusCode::OK.into_response()
                        }
                        _ => unreachable!(),
                    }
                }),
            )
        })
        .await;
        let a = TestInstance::start().await;
        let peer = a.connect_to(&b).await;
        let request = create_request(
            a.state.db.instance_id().await.unwrap(),
            peer.peer_instance_id,
            "source".parse().unwrap(),
            Uuid::new_v4(),
        );
        let error = request_peer_with_timeout(
            &peer,
            &request,
            std::time::Duration::from_millis(if mode == "timeout" { 50 } else { 2000 }),
        )
        .await
        .unwrap_err();
        match mode {
            "malformed" => assert!(matches!(error, FederationClientError::InvalidResponse(_))),
            "oversized" => assert!(matches!(error, FederationClientError::ResponseTooLarge(_))),
            "redirect" => assert!(matches!(error, FederationClientError::RemoteRejected(_))),
            "timeout" => assert!(
                matches!(error, FederationClientError::Request(ref inner) if inner.is_timeout())
            ),
            _ => unreachable!(),
        }
        a.stop().await;
        b.stop().await;
    }
}

#[tokio::test]
async fn slow_https_subscription_pass_has_one_total_budget_before_local_direct_turn_discovery() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let b = TestInstance::start_with_router(false, move |_| {
        axum::Router::new().route(
            "/api/federation/peer/subscriptions/create",
            axum::routing::post(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    // test-timing-allow: the response must outlive the total sync-pass budget
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    axum::http::StatusCode::OK
                }
            }),
        )
    })
    .await;
    let a = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    for number in 0..4 {
        a.state
            .db
            .save_remote_subscription(
                &SubscriptionReference {
                    instance_id: peer.peer_instance_id,
                    product_conversation_id: format!("source-{number}").parse().unwrap(),
                },
                Uuid::new_v4(),
            )
            .await
            .unwrap();
    }
    let local = source(&a).await;
    let service = crate::send_chat_service::SendChatApplicationService::new(
        a.state.db.clone(),
        a.state.runtime.clone(),
    );
    let admitted = service
        .send(crate::send_chat_service::SendChatRequest {
            conversation_id: local.id.clone(),
            origin: phoenix_core::domain::db_schema::InputOrigin::UserApi,
            message_id: "local-budget-regression".into(),
            text: "local direct turn remains discoverable".into(),
            images: vec![],
            files: vec![],
            user_agent: None,
            expansion_policy: crate::send_chat_service::MessageExpansionPolicy::LiteralText,
        })
        .await
        .unwrap();
    assert_eq!(
        admitted,
        crate::send_chat_service::SendChatOutcome::Delivered
    );
    let started = std::time::Instant::now();
    let mut sync = SubscriptionSync::default();
    sync.sync_pass(&a.state.runtime).await;
    assert!(started.elapsed() < SYNC_PASS_BUDGET + std::time::Duration::from_millis(250));
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "the first slow peer must actually have been contacted"
    );
    let discovered = a
        .state
        .db
        .workflow_repository()
        .list_discoverable_accepted_runtime_direct_turns(None, 32)
        .await
        .unwrap();
    assert!(discovered
        .candidates
        .iter()
        .any(|turn| turn.conversation.0 == local.id));
    let started = std::time::Instant::now();
    sync.sync_pass(&a.state.runtime).await;
    assert!(started.elapsed() < SYNC_PASS_BUDGET + std::time::Duration::from_millis(250));
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "the pending request is retained, not restarted"
    );
    drop(sync);
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn healthy_300ms_subscription_requests_finish_across_bounded_passes_without_restarting_create(
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let creates = Arc::new(AtomicUsize::new(0));
    let seen = creates.clone();
    let b = TestInstance::start_with_router(false, move |state| {
        create_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let seen = seen.clone();
                async move {
                    if request
                        .uri()
                        .path()
                        .starts_with("/api/federation/peer/subscriptions/")
                    {
                        if request.uri().path().ends_with("/create") {
                            seen.fetch_add(1, Ordering::SeqCst);
                        }
                        // test-timing-allow: elapsed delay is the behavior under the bounded-pass test
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    }
                    next.run(request).await
                }
            },
        ))
    })
    .await;
    let a = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let reference = SubscriptionReference {
        instance_id: peer.peer_instance_id,
        product_conversation_id: source.product_conversation_id.clone(),
    };
    let id = Uuid::new_v4();
    a.state
        .db
        .save_remote_subscription(&reference, id)
        .await
        .unwrap();
    let mut sync = SubscriptionSync::default();
    let mut emitted = false;
    let mut passes = 0;
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let started = std::time::Instant::now();
            sync.sync_pass(&a.state.runtime).await;
            assert!(started.elapsed() < SYNC_PASS_BUDGET + std::time::Duration::from_millis(250));
            passes += 1;
            let remote = a
                .state
                .db
                .remote_subscription_by_id(reference.instance_id, id)
                .await
                .unwrap()
                .unwrap();
            if remote.enrolled && !emitted {
                commit_terminal(&b, &source, TerminalFactOutcome::Failed).await;
                emitted = true;
            }
            if remote.admitted_cursor.get() == 1 {
                assert_eq!(remote.state, SubscriptionState::Active);
                break;
            }
        }
    })
    .await
    .expect("healthy requests must make progress, not restart at each pass deadline");
    assert!(passes > 1);
    assert_eq!(creates.load(Ordering::SeqCst), 1);
    drop(sync);
    a.stop().await;
    b.stop().await;
}

async fn continue_member(instance: &TestInstance, predecessor: &str) -> phoenix_db::Conversation {
    instance
        .state
        .db
        .add_message(
            &Uuid::new_v4().to_string(),
            predecessor,
            &phoenix_core::domain::db_schema::MessageContent::continuation("handoff"),
            None,
            None,
        )
        .await
        .unwrap();
    instance
        .state
        .db
        .update_conversation_state(
            predecessor,
            &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
                summary: "handoff".into(),
            },
        )
        .await
        .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) = instance
        .state
        .db
        .continue_conversation(predecessor)
        .await
        .unwrap()
    else {
        panic!("continuation must create a successor")
    };
    successor
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn source_and_coordinator_continuation_preserve_exact_fact_and_follow_current_admission_target(
) {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    stage_one(&a, &b, &peer, &remote).await;
    let successor = continue_member(&b, &source.id).await;
    let original_coordinator = a
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    let current_coordinator = continue_member(&a, &original_coordinator).await;
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let row = refreshed(&a, &remote).await;
    assert_eq!(row.admitted_cursor.get(), 1);
    let history = a
        .state
        .db
        .remote_notification_history(&row, SubscriptionCursor::START)
        .await
        .unwrap();
    assert!(
        matches!(&history[0].content, SubscriptionDelivery::Fact { fact } if fact.source_transcript_id.as_str() == source.id)
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT conversation_id FROM durable_turns WHERE origin_subscription_event_id = ?1"
        )
        .bind(&history[0].admission_id)
        .fetch_one(a.state.db.pool())
        .await
        .unwrap(),
        current_coordinator.id
    );
    let message = super::send::materialize_received(&a, &current_coordinator.id).await;
    let presentation =
        crate::api::event_presentation::EventPresentations::load(&a.state.db, [&message])
            .await
            .for_message(&message)
            .unwrap();
    let crate::api::event_presentation::EventPresentation::FederatedFact(presentation) =
        presentation
    else {
        panic!("typed remote fact required")
    };
    assert_eq!(presentation.source_transcript_id, source.id);
    let crate::api::event_presentation::FederatedEventNavigation::Available {
        stable_url,
        source_url,
    } = presentation.navigation
    else {
        panic!("destination-qualified paths required")
    };
    let aggregate = b
        .state
        .db
        .get_ordinary_product_conversation(&source.product_conversation_id)
        .await
        .unwrap();
    assert_eq!(
        stable_url,
        b.origin
            .navigation_url(&crate::api::product_conversations::canonical_route(
                &aggregate,
            ))
            .unwrap()
            .as_url()
            .to_string()
    );
    assert_eq!(
        source_url,
        b.origin
            .navigation_url(&crate::api::product_conversations::exact_transcript_route(
                &phoenix_core::domain::close::TranscriptConversationId::parse(source.id.clone())
                    .unwrap(),
            ))
            .unwrap()
            .as_url()
            .to_string()
    );
    commit_terminal(&b, &successor, TerminalFactOutcome::Failed).await;
    sync_once(&a.state.db, &a.state.runtime, &row)
        .await
        .unwrap();
    let row = refreshed(&a, &row).await;
    assert_eq!(row.admitted_cursor.get(), 2);
    let history = a
        .state
        .db
        .remote_notification_history(&row, SubscriptionCursor::START)
        .await
        .unwrap();
    assert!(
        matches!(&history[1].content, SubscriptionDelivery::Fact { fact } if fact.source_transcript_id.as_str() == successor.id)
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn subscription_peer_bearer_cannot_use_owner_routes_and_owner_password_cannot_authorize_delivery(
) {
    use phoenix_core::domain::instance_identity::OwnerPassword;
    let b = TestInstance::start_with_router(false, |mut state| {
        state.password = Some(OwnerPassword::parse("owner-only-password".into()).unwrap());
        create_router(state)
    })
    .await;
    let a = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let pending = stage_one(&a, &b, &peer, &remote).await;
    let authorization = SubscriptionRequest {
        caller_instance_id: a.state.db.instance_id().await.unwrap(),
        reference: remote.reference.clone(),
        subscription_id: remote.subscription_id,
        operation: SubscriptionOperation::Authorize {
            delivery: pending.content,
        },
    };
    for operation in ["create", "replay", "authorize", "unsubscribe"] {
        let endpoint = b.endpoint(&format!("/api/federation/peer/subscriptions/{operation}"));
        let response = b
            .client
            .post(endpoint)
            .bearer_auth("owner-only-password")
            .json(&authorization)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
    let owner_list = b.endpoint("/api/federation/subscriptions");
    assert_eq!(
        b.client
            .get(&owner_list)
            .bearer_auth(peer.bearer_credential.expose())
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        b.client
            .get(owner_list)
            .bearer_auth("owner-only-password")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn source_revocation_without_pending_notifications_ends_create_or_replay_locally() {
    for enrolled in [false, true] {
        let a = TestInstance::start().await;
        let b = TestInstance::start().await;
        let peer = a.connect_to(&b).await;
        let source = source(&b).await;
        let reference = SubscriptionReference {
            instance_id: peer.peer_instance_id,
            product_conversation_id: source.product_conversation_id.clone(),
        };
        let id = Uuid::new_v4();
        a.state
            .db
            .save_remote_subscription(&reference, id)
            .await
            .unwrap();
        if enrolled {
            let remote = create_saved_peer_subscription(
                &a.state.db,
                peer.peer_instance_id.to_string(),
                source.product_conversation_id.clone(),
                Some(id),
            )
            .await
            .unwrap()
            .subscription;
            commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
            sync_once(&a.state.db, &a.state.runtime, &remote)
                .await
                .unwrap();
        }
        let remote = a
            .state
            .db
            .remote_subscription_by_id(peer.peer_instance_id, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remote.enrolled, enrolled);
        assert_eq!(remote.admitted_cursor.get(), i64::from(enrolled));
        assert!(a
            .state
            .db
            .pending_remote_notifications(&remote)
            .await
            .unwrap()
            .is_empty());
        let history = a
            .state
            .db
            .remote_notification_history(&remote, SubscriptionCursor::START)
            .await
            .unwrap();
        if enrolled {
            assert!(matches!(
                history[0].disposition,
                phoenix_db::federation_subscriptions::RemoteNotificationDisposition::Accepted { .. }
            ));
        }
        let history = serde_json::to_value(history).unwrap();
        b.state
            .db
            .revoke_federation_enrollment(a.state.db.instance_id().await.unwrap())
            .await
            .unwrap();
        let source_query = "SELECT state,last_sequence,acknowledged_through,pruned_through,(SELECT COUNT(*) FROM federation_terminal_payloads),(SELECT COUNT(*) FROM federation_delivery_authorizations) FROM federation_subscriptions ORDER BY id";
        let before: Vec<(String, i64, i64, i64, i64, i64)> = sqlx::query_as(source_query)
            .fetch_all(b.state.db.pool())
            .await
            .unwrap();
        assert!(sync_once(&a.state.db, &a.state.runtime, &remote)
            .await
            .is_err());
        let ended = refreshed(&a, &remote).await;
        assert_eq!(ended.state, SubscriptionState::Revoked);
        assert_eq!(ended.admitted_cursor, remote.admitted_cursor);
        assert_eq!(ended.enrolled, enrolled);
        assert!(!ended.pending_unsubscribe);
        assert_eq!(
            serde_json::to_value(
                a.state
                    .db
                    .remote_notification_history(&ended, SubscriptionCursor::START)
                    .await
                    .unwrap()
            )
            .unwrap(),
            history
        );
        let after: Vec<(String, i64, i64, i64, i64, i64)> = sqlx::query_as(source_query)
            .fetch_all(b.state.db.pool())
            .await
            .unwrap();
        assert_eq!(after, before);
        a.stop().await;
        b.stop().await;
    }
}

#[tokio::test]
async fn revocation_during_saved_peer_create_ends_the_persisted_retry_identity() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let b = TestInstance::start_with_router(false, {
        let entered = entered.clone();
        let release = release.clone();
        move |state| {
            create_router(state).layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        if request.uri().path() == "/api/federation/peer/subscriptions/create" {
                            entered.notify_one();
                            release.notified().await;
                        }
                        next.run(request).await
                    }
                },
            ))
        }
    })
    .await;
    let a = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let id = Uuid::new_v4();
    let create = tokio::spawn({
        let db = a.state.db.clone();
        let product = source.product_conversation_id.clone();
        async move {
            create_saved_peer_subscription(
                &db,
                peer.peer_instance_id.to_string(),
                product,
                Some(id),
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let remote = a
        .state
        .db
        .remote_subscription_by_id(peer.peer_instance_id, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote.state, SubscriptionState::Active);
    assert!(!remote.enrolled);
    b.state
        .db
        .revoke_federation_enrollment(a.state.db.instance_id().await.unwrap())
        .await
        .unwrap();
    release.notify_one();
    assert!(create.await.unwrap().is_err());
    let ended = refreshed(&a, &remote).await;
    assert_eq!(ended.state, SubscriptionState::Revoked);
    assert_eq!(ended.admitted_cursor, SubscriptionCursor::START);
    assert!(!ended.enrolled && !ended.pending_unsubscribe);
    assert!(a
        .state
        .db
        .remote_notification_history(&ended, SubscriptionCursor::START)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_subscriptions")
            .fetch_one(b.state.db.pool())
            .await
            .unwrap(),
        0
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn revoked_bearer_cannot_recover_an_already_authorized_delivery_receipt_over_https() {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let pending = stage_one(&a, &b, &peer, &remote).await;
    let request = SubscriptionRequest {
        caller_instance_id: a.state.db.instance_id().await.unwrap(),
        reference: remote.reference.clone(),
        subscription_id: remote.subscription_id,
        operation: SubscriptionOperation::Authorize {
            delivery: pending.content,
        },
    };
    let authorized = request_peer(&peer, &request).await.unwrap();
    assert!(matches!(
        authorized.result,
        SubscriptionResult::Authorization {
            authorization: SubscriptionAuthorization::Authorized { .. },
            ..
        }
    ));
    b.state
        .db
        .revoke_federation_enrollment(request.caller_instance_id)
        .await
        .unwrap();
    let response = b
        .client
        .post(b.endpoint("/api/federation/peer/subscriptions/authorize"))
        .bearer_auth(peer.bearer_credential.expose())
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(matches!(
        request_peer(&peer, &request).await,
        Err(crate::api::federation::FederationClientError::SubscriptionAuthenticationRejected)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_delivery_authorizations")
            .fetch_one(b.state.db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        refreshed(&a, &remote).await.admitted_cursor,
        SubscriptionCursor::START
    );
    let receipt = unsubscribe_remote(&a.state.db, &peer, &remote)
        .await
        .unwrap();
    assert!(receipt.local_stopped);
    assert!(!receipt.remote_confirmed);
    let unsubscribed = refreshed(&a, &remote).await;
    assert_eq!(unsubscribed.state, SubscriptionState::Unsubscribed);
    assert!(!unsubscribed.pending_unsubscribe);
    assert!(a
        .state
        .db
        .next_remote_subscription_sync_batch(4)
        .await
        .unwrap()
        .is_empty());
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn source_unsubscribe_or_revoke_before_authorization_suppresses_staged_admission() {
    for reason in [SubscriptionState::Unsubscribed, SubscriptionState::Revoked] {
        let a = TestInstance::start().await;
        let b = TestInstance::start().await;
        let peer = a.connect_to(&b).await;
        let source = source(&b).await;
        let remote = create_saved_peer_subscription(
            &a.state.db,
            peer.peer_instance_id.to_string(),
            source.product_conversation_id.clone(),
            None,
        )
        .await
        .unwrap()
        .subscription;
        commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
        let pending = stage_one(&a, &b, &peer, &remote).await;
        if reason == SubscriptionState::Revoked {
            b.state
                .db
                .revoke_federation_enrollment(a.state.db.instance_id().await.unwrap())
                .await
                .unwrap();
        } else {
            let request = SubscriptionRequest {
                caller_instance_id: a.state.db.instance_id().await.unwrap(),
                reference: remote.reference.clone(),
                subscription_id: remote.subscription_id,
                operation: SubscriptionOperation::Unsubscribe,
            };
            assert!(request_peer(&peer, &request).await.is_ok());
        }
        sync_once(&a.state.db, &a.state.runtime, &remote)
            .await
            .unwrap();
        let ended = refreshed(&a, &remote).await;
        assert_eq!(ended.state, reason);
        assert_eq!(ended.admitted_cursor, SubscriptionCursor::START);
        let history = a
            .state
            .db
            .remote_notification_history(&ended, SubscriptionCursor::START)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert!(matches!(
            history[0].disposition,
            phoenix_db::federation_subscriptions::RemoteNotificationDisposition::Suppressed
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM durable_turns WHERE origin_subscription_event_id = ?1"
            )
            .bind(pending.admission_id)
            .fetch_one(a.state.db.pool())
            .await
            .unwrap(),
            0
        );
        a.stop().await;
        b.stop().await;
    }
}

#[tokio::test]
async fn expired_source_payload_replays_an_explicit_gap_with_current_snapshot_and_remote_presentation(
) {
    let a = TestInstance::start().await;
    let b = TestInstance::start().await;
    let peer = a.connect_to(&b).await;
    let source = source(&b).await;
    let remote = create_saved_peer_subscription(
        &a.state.db,
        peer.peer_instance_id.to_string(),
        source.product_conversation_id.clone(),
        None,
    )
    .await
    .unwrap()
    .subscription;
    commit_terminal(&b, &source, TerminalFactOutcome::Completed).await;
    let verifier =
        FederationCredentialVerifier::from_bearer(peer.bearer_credential.expose().as_bytes());
    let enrollment = b
        .state
        .db
        .authenticate_federation_verifier(&verifier)
        .await
        .unwrap()
        .unwrap();
    let expired = b
        .state
        .db
        .replay_federation_subscription(
            enrollment.enrollment_id,
            enrollment.caller_instance_id,
            remote.subscription_id,
            &source.product_conversation_id,
            SubscriptionCursor::START,
            chrono::Utc::now().timestamp_micros() + RETENTION_US + 1,
        )
        .await
        .unwrap();
    assert!(matches!(expired, SubscriptionReplay::Gap { .. }));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM federation_terminal_payloads")
            .fetch_one(b.state.db.pool())
            .await
            .unwrap(),
        0
    );
    sync_once(&a.state.db, &a.state.runtime, &remote)
        .await
        .unwrap();
    let remote = refreshed(&a, &remote).await;
    assert_eq!(remote.admitted_cursor.get(), 1);
    let history = a
        .state
        .db
        .remote_notification_history(&remote, SubscriptionCursor::START)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert!(
        matches!(&history[0].content, SubscriptionDelivery::Gap { gap } if gap.requested_after == SubscriptionCursor::START && gap.resume_after.get() == 1 && matches!(gap.snapshot.state, SubscriptionCurrentState::Terminal { outcome: TerminalFactOutcome::Completed }))
    );
    let coordinator = a
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    let message = super::send::materialize_received(&a, &coordinator).await;
    let presentation =
        crate::api::event_presentation::EventPresentations::load(&a.state.db, [&message])
            .await
            .for_message(&message)
            .unwrap();
    assert!(matches!(
        presentation,
        crate::api::event_presentation::EventPresentation::FederatedGap(
            crate::api::event_presentation::FederatedGapPresentation {
                current: crate::api::event_presentation::FederatedCurrentState::Terminal {
                    outcome: crate::api::event_presentation::FederatedEventOutcome::Completed
                },
                navigation: crate::api::event_presentation::FederatedEventNavigation::Available { .. },
                ..
            }
        )
    ));
    a.stop().await;
    b.stop().await;
}
