use super::super::*;
use axum::{body::Body, http::Request, Router};
use phoenix_core::domain::{
    close::TranscriptConversationId, db_schema::InputOrigin,
    product_conversation::ProductConversationId, sm_event::PreparedDirectTurnPayload,
};
use serde_json::{json, Value};
use tower::ServiceExt as _;

fn request(destination: InstanceId, target: RemoteSendTarget) -> RemoteSendMessageRequest {
    RemoteSendMessageRequest {
        destination_instance_id: destination,
        target,
        message_id: uuid::Uuid::new_v4(),
        message: RemoteMessageText::parse("literal @conv:do-not-expand".into()).unwrap(),
        sender: sender(),
    }
}

fn sender() -> RemoteSenderAttestation {
    serde_json::from_value(json!({
        "kind": "conversation",
        "product_conversation_id": "stable-source",
        "transcript_id": "exact-source",
        "source_call": {"message_id": "source-assistant", "tool_use_id": "source-send"}
    }))
    .unwrap()
}

fn exact(id: &str) -> RemoteSendTarget {
    RemoteSendTarget::Transcript {
        transcript_id: TranscriptConversationId::parse(id).unwrap(),
    }
}

fn acceptance(response: &RemoteSendMessageResponse) -> &RemoteMessageAcceptance {
    match &response.receipt {
        RemoteSendReceipt::Accepted { acceptance }
        | RemoteSendReceipt::Duplicate { acceptance } => acceptance,
        RemoteSendReceipt::Rejected { .. } => {
            panic!(
                "expected acceptance-only receipt, got {:?}",
                response.receipt
            )
        }
    }
}

async fn fixture() -> (
    AppState,
    Router,
    InstanceId,
    String,
    crate::db::Conversation,
) {
    fixture_from_state(crate::api::handlers::hard_delete_cascade_tests::make_test_state().await)
        .await
}

async fn fixture_from_state(
    state: AppState,
) -> (
    AppState,
    Router,
    InstanceId,
    String,
    crate::db::Conversation,
) {
    let target = state
        .db
        .create_conversation("recipient", "recipient", "/tmp", true, None, None)
        .await
        .unwrap();
    let caller = InstanceId::new();
    let bearer = format!("phx_peer_{}", "b".repeat(43));
    state
        .db
        .replace_federation_enrollment(
            caller,
            "caller",
            &FederationCredentialVerifier::from_bearer(bearer.as_bytes()),
        )
        .await
        .unwrap();
    let app = crate::api::handlers::create_router(state.clone());
    (state, app, caller, bearer, target)
}

async fn route(app: &Router, body: &impl Serialize, bearer: Option<&str>) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/federation/peer/send-conversation-message")
        .header("content-type", "application/json");
    if let Some(bearer) = bearer {
        builder = builder.header("authorization", format!("Bearer {bearer}"));
    }
    app.clone()
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn send(
    app: &Router,
    body: &RemoteSendMessageRequest,
    bearer: &str,
) -> RemoteSendMessageResponse {
    let response = route(app, body, Some(bearer)).await;
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), MAX_REMOTE_RESPONSE_BYTES)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn effect_counts(state: &AppState) -> (i64, i64, i64) {
    let turns = sqlx::query_scalar("SELECT COUNT(*) FROM durable_turns")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    let effects = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    let steering = sqlx::query_scalar("SELECT COUNT(*) FROM steering_acceptance_receipts")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    (turns, effects, steering)
}

async fn stored_payload(state: &AppState, id: &str) -> PreparedDirectTurnPayload {
    let turn = state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(id.to_string()))
        .await
        .unwrap()
        .unwrap();
    PreparedDirectTurnPayload::from_exact_bytes(turn.prepared.payload()).unwrap()
}

async fn gate_strong_count(state: &AppState, admitted_id: uuid::Uuid) -> usize {
    state
        .remote_message_admission
        .gates
        .lock()
        .await
        .get(&admitted_id)
        .map_or(0, std::sync::Weak::strong_count)
}

#[test]
fn admission_uuid_is_stable_and_scoped_to_peer_and_operation() {
    let peer = InstanceId::new();
    let operation = uuid::Uuid::new_v4();
    let admitted = remote_admission_id(peer, operation);
    assert_eq!(admitted, remote_admission_id(peer, operation));
    assert_ne!(admitted, operation);
    assert_ne!(admitted, remote_admission_id(InstanceId::new(), operation));
    assert_ne!(admitted, remote_admission_id(peer, uuid::Uuid::new_v4()));
}

#[tokio::test]
async fn detached_send_rechecks_exact_credential_at_direct_admission_and_preserves_prior_replay() {
    let (state, app, caller, bearer, target) = fixture().await;
    let verifier = FederationCredentialVerifier::from_bearer(bearer.as_bytes());
    let authority = phoenix_db::FederationSendAuthority::new(caller, verifier);
    let destination = state.db.instance_id().await.unwrap();
    let admitted = request(destination, exact(&target.id));
    let accepted = send(&app, &admitted, &bearer).await;
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    let fresh = state
        .db
        .create_conversation("revoked-target", "revoked-target", "/tmp", true, None, None)
        .await
        .unwrap();
    state.db.revoke_federation_enrollment(caller).await.unwrap();
    let denied = request(destination, exact(&fresh.id));
    assert_eq!(
        route(&app, &denied, Some(&bearer)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let rejected = admit_remote_message(
        &state,
        caller,
        remote_admission_id(caller, denied.message_id),
        &denied,
        &authority,
    )
    .await;
    assert!(matches!(
        rejected,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::AuthorityInactive,
            ..
        }
    ));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    let replay = admit_remote_message(
        &state,
        caller,
        remote_admission_id(caller, admitted.message_id),
        &admitted,
        &authority,
    )
    .await;
    assert!(matches!(replay, RemoteSendReceipt::Duplicate { .. }));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    let replacement_bearer = format!("phx_peer_{}", "d".repeat(43));
    let replacement = FederationCredentialVerifier::from_bearer(replacement_bearer.as_bytes());
    state
        .db
        .replace_federation_enrollment(caller, "peer", &replacement)
        .await
        .unwrap();
    let again = request(destination, exact(&fresh.id));
    let rejected = admit_remote_message(
        &state,
        caller,
        remote_admission_id(caller, again.message_id),
        &again,
        &authority,
    )
    .await;
    assert!(matches!(
        rejected,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::AuthorityInactive,
            ..
        }
    ));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));
    assert_eq!(
        route(&app, &again, Some(&bearer)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let accepted_with_replacement = send(&app, &again, &replacement_bearer).await;
    assert!(matches!(
        accepted_with_replacement.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_eq!(effect_counts(&state).await, (2, 2, 0));
}

#[tokio::test]
async fn steering_admission_rechecks_exact_credential_after_authentication() {
    let (state, _app, caller, bearer, target) = fixture().await;
    state
        .runtime
        .inject_handle_for_test(
            &target.id,
            crate::db::ConvState::LlmRequesting { attempt: 1 },
        )
        .await;
    let destination = state.db.instance_id().await.unwrap();
    let body = request(destination, exact(&target.id));
    let revoked_authority = phoenix_db::FederationSendAuthority::new(
        caller,
        FederationCredentialVerifier::from_bearer(bearer.as_bytes()),
    );
    state.db.revoke_federation_enrollment(caller).await.unwrap();

    let rejected = admit_remote_message(
        &state,
        caller,
        remote_admission_id(caller, body.message_id),
        &body,
        &revoked_authority,
    )
    .await;
    assert!(matches!(
        rejected,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::AuthorityInactive,
            ..
        }
    ));
    assert!(state
        .db
        .get_steering_queue(&target.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(effect_counts(&state).await, (0, 0, 0));

    let replacement_bearer = format!("phx_peer_{}", "e".repeat(43));
    let replacement = FederationCredentialVerifier::from_bearer(replacement_bearer.as_bytes());
    state
        .db
        .replace_federation_enrollment(caller, "peer", &replacement)
        .await
        .unwrap();
    let still_rejected = admit_remote_message(
        &state,
        caller,
        remote_admission_id(caller, body.message_id),
        &body,
        &revoked_authority,
    )
    .await;
    assert!(matches!(
        still_rejected,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::AuthorityInactive,
            ..
        }
    ));

    let current = phoenix_db::FederationSendAuthority::new(caller, replacement);
    let accepted = admit_remote_message(
        &state,
        caller,
        remote_admission_id(caller, body.message_id),
        &body,
        &current,
    )
    .await;
    assert!(matches!(
        accepted,
        RemoteSendReceipt::Accepted {
            acceptance: RemoteMessageAcceptance {
                disposition: RemoteSendDisposition::Queued,
                ..
            }
        }
    ));
    assert_eq!(
        state.db.get_steering_queue(&target.id).await.unwrap().len(),
        1
    );
    assert_eq!(effect_counts(&state).await, (0, 0, 1));
}

#[tokio::test]
async fn registered_send_route_requires_peer_auth_and_destination_match() {
    let (state, app, caller, bearer, target) = fixture().await;
    let mut body = request(InstanceId::new(), exact(&target.id));
    for credential in [None, Some("phx_peer_invalid")] {
        assert_eq!(
            route(&app, &body, credential).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let response = send(&app, &body, &bearer).await;
    assert_eq!(response.caller_instance_id, caller);
    assert_eq!(
        response.destination_instance_id,
        state.db.instance_id().await.unwrap()
    );
    assert!(matches!(
        response.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::DestinationMismatch,
            ..
        }
    ));
    assert_eq!(effect_counts(&state).await, (0, 0, 0));

    body.destination_instance_id = state.db.instance_id().await.unwrap();
    let valid = serde_json::to_value(&body).unwrap();
    for (field, value) in [
        ("source_instance_id", json!(InstanceId::new())),
        ("message", json!("x".repeat(MAX_REMOTE_MESSAGE_BYTES + 1))),
        ("message_id", json!("invalid")),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert_eq!(
            route(&app, &invalid, Some(&bearer)).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(effect_counts(&state).await, (0, 0, 0));
}

#[tokio::test]
async fn send_decoder_requires_operation_and_distinct_admitted_identity_and_provenance() {
    let (state, app, caller, bearer, target) = fixture().await;
    let body = request(state.db.instance_id().await.unwrap(), exact(&target.id));
    let accepted = send(&app, &body, &bearer).await;
    let valid = serde_json::to_value(&accepted).unwrap();
    let decode = |wire: &Value| {
        decode_remote_send_response(
            StatusCode::OK,
            &serde_json::to_vec(wire).unwrap(),
            caller,
            &body,
        )
    };
    assert!(decode(&valid).is_ok());
    assert_eq!(accepted.operation_id, body.message_id);
    assert_eq!(
        acceptance(&accepted).admitted_message_id,
        format!(
            "{}:{}",
            target.id,
            remote_admission_id(caller, body.message_id)
        )
    );
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("operation_id");
    assert!(decode(&missing).is_err());
    let mut missing = valid.clone();
    missing["receipt"]["acceptance"]
        .as_object_mut()
        .unwrap()
        .remove("admitted_message_id");
    assert!(decode(&missing).is_err());
    for (path, replacement) in [
        ("/operation_id", json!(uuid::Uuid::new_v4())),
        ("/destination_instance_id", json!(InstanceId::new())),
        ("/caller_instance_id", json!(InstanceId::new())),
        ("/target/transcript_id", json!("different")),
        (
            "/receipt/acceptance/admitted_message_id",
            json!(body.message_id),
        ),
        (
            "/receipt/acceptance/admitted_message_id",
            json!(format!("{}:{}", target.id, body.message_id)),
        ),
        (
            "/receipt/acceptance/accepting_transcript",
            json!("different"),
        ),
        (
            "/receipt/acceptance/stable_destination/instance_id",
            json!(InstanceId::new()),
        ),
        (
            "/receipt/acceptance/accepting_message_navigation/path",
            json!("/c/recipient#message-other"),
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(path).unwrap() = replacement;
        assert!(decode(&invalid).is_err(), "accepted mismatched {path}");
    }
}

#[tokio::test]
async fn retries_bind_original_target_and_text_without_reattributing_sender() {
    let (state, app, caller, bearer, target) = fixture().await;
    let body = request(
        state.db.instance_id().await.unwrap(),
        RemoteSendTarget::ProductConversation {
            product_conversation_id: target.product_conversation_id.clone(),
        },
    );
    let accepted = send(&app, &body, &bearer).await;
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    let payload = stored_payload(&state, &target.id).await;
    assert_eq!(
        payload.submitted.origin,
        body.sender.clone().into_origin(caller)
    );
    assert!(matches!(
        payload.submitted.origin,
        InputOrigin::RemoteConversation { .. }
    ));
    assert_eq!(payload.submitted.text, "literal @conv:do-not-expand");
    let duplicate = send(&app, &body, &bearer).await;
    assert!(matches!(
        duplicate.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&accepted), acceptance(&duplicate));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    let mut conflicting_requests = Vec::new();
    let mut conflicting_request = body.clone();
    conflicting_request.message = RemoteMessageText::parse("changed text".into()).unwrap();
    conflicting_requests.push(conflicting_request);
    for target in [
        exact(&target.id),
        exact("other-transcript"),
        RemoteSendTarget::ProductConversation {
            product_conversation_id: ProductConversationId::parse("other-product").unwrap(),
        },
    ] {
        let mut conflicting_request = body.clone();
        conflicting_request.target = target;
        conflicting_requests.push(conflicting_request);
    }
    for conflicting_request in conflicting_requests {
        let rejected = send(&app, &conflicting_request, &bearer).await;
        assert!(
            matches!(
                rejected.receipt,
                RemoteSendReceipt::Rejected {
                    code: RemoteSendRejectionCode::IdempotencyConflict,
                    ..
                }
            ),
            "changed target or content admitted: {rejected:?}"
        );
        assert_eq!(effect_counts(&state).await, (1, 1, 0));
        assert_eq!(stored_payload(&state, &target.id).await, payload);
    }

    let sender_wire = serde_json::to_value(&body.sender).unwrap();
    let mut sender_variants = Vec::new();
    for (path, value) in [
        ("/kind", json!("coordinator")),
        ("/product_conversation_id", json!("other-source")),
        ("/transcript_id", json!("other-exact-source")),
        ("/source_call/message_id", json!("other-assistant")),
        ("/source_call/tool_use_id", json!("other-call")),
        ("/source_call", Value::Null),
    ] {
        let mut wire = sender_wire.clone();
        *wire.pointer_mut(path).unwrap() = value;
        let mut changed = body.clone();
        changed.sender = serde_json::from_value(wire).unwrap();
        sender_variants.push(changed);
    }
    let mut changed = body.clone();
    changed.sender = RemoteSenderAttestation::RemoteApi {};
    sender_variants.push(changed);
    for sender_variant in sender_variants {
        let replayed = send(&app, &sender_variant, &bearer).await;
        assert!(
            matches!(replayed.receipt, RemoteSendReceipt::Duplicate { .. }),
            "same peer, target, and content did not replay: {replayed:?}"
        );
        assert_eq!(acceptance(&accepted), acceptance(&replayed));
        assert_eq!(effect_counts(&state).await, (1, 1, 0));
        assert_eq!(stored_payload(&state, &target.id).await, payload);
    }
}

async fn state_from_database_path(path: &std::path::Path) -> AppState {
    let db = crate::db::Database::open(path.to_str().unwrap())
        .await
        .unwrap();
    crate::db::run_pending_migrations(db.pool()).await.unwrap();
    crate::api::handlers::hard_delete_cascade_tests::make_test_state_with_db(db)
}

async fn close_state(state: AppState, app: Router) {
    state.db.pool().close().await;
    drop(app);
    drop(state);
}

async fn reopened_state(path: &std::path::Path) -> (AppState, Router) {
    let state = state_from_database_path(path).await;
    let app = crate::api::handlers::create_router(state.clone());
    (state, app)
}

#[tokio::test]
async fn peer_scoped_sends_replay_after_database_and_runtime_reconstruction() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("federation-send.db");
    let state = state_from_database_path(&path).await;
    let (state, app, _caller, bearer, target) = fixture_from_state(state).await;
    let destination = state.db.instance_id().await.unwrap();
    let body = request(destination, exact(&target.id));

    let accepted = send(&app, &body, &bearer).await;
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    let original_acceptance = acceptance(&accepted).clone();
    let original_payload = stored_payload(&state, &target.id).await;
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    close_state(state, app).await;

    let (state, app) = reopened_state(&path).await;
    assert_eq!(state.db.instance_id().await.unwrap(), destination);

    let duplicate = send(&app, &body, &bearer).await;
    assert!(matches!(
        duplicate.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&duplicate), &original_acceptance);
    assert_eq!(stored_payload(&state, &target.id).await, original_payload);
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    let mut changed = body.clone();
    changed.message = RemoteMessageText::parse("changed after restart".into()).unwrap();
    let conflict = send(&app, &changed, &bearer).await;
    assert!(matches!(
        conflict.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::IdempotencyConflict,
            ..
        }
    ));
    assert_eq!(stored_payload(&state, &target.id).await, original_payload);
    assert_eq!(effect_counts(&state).await, (1, 1, 0));

    let second_target = state
        .db
        .create_conversation(
            "second-recipient",
            "second-recipient",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let second_caller = InstanceId::new();
    let second_bearer = format!("phx_peer_{}", "c".repeat(43));
    state
        .db
        .replace_federation_enrollment(
            second_caller,
            "second-caller",
            &FederationCredentialVerifier::from_bearer(second_bearer.as_bytes()),
        )
        .await
        .unwrap();
    let mut second_body = body.clone();
    second_body.target = exact(&second_target.id);
    let second_accepted = send(&app, &second_body, &second_bearer).await;
    assert!(matches!(
        second_accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_ne!(
        acceptance(&second_accepted).admitted_message_id,
        original_acceptance.admitted_message_id
    );
    let second_acceptance = acceptance(&second_accepted).clone();
    let second_payload = stored_payload(&state, &second_target.id).await;
    assert_eq!(
        second_payload.submitted.origin,
        second_body.sender.clone().into_origin(second_caller)
    );
    assert_eq!(effect_counts(&state).await, (2, 2, 0));

    close_state(state, app).await;

    let (state, app) = reopened_state(&path).await;
    let first_duplicate = send(&app, &body, &bearer).await;
    let second_duplicate = send(&app, &second_body, &second_bearer).await;
    assert!(matches!(
        first_duplicate.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert!(matches!(
        second_duplicate.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&first_duplicate), &original_acceptance);
    assert_eq!(acceptance(&second_duplicate), &second_acceptance);
    assert_eq!(stored_payload(&state, &target.id).await, original_payload);
    assert_eq!(
        stored_payload(&state, &second_target.id).await,
        second_payload
    );
    assert_eq!(effect_counts(&state).await, (2, 2, 0));

    state.db.pool().close().await;
}

#[tokio::test]
async fn busy_destination_queues_once_and_replays_the_exact_remote_acceptance() {
    let (state, app, caller, bearer, target) = fixture().await;
    let persisted = state.db.get_conversation(&target.id).await.unwrap();
    assert!(matches!(persisted.state, crate::db::ConvState::Idle));
    state
        .runtime
        .inject_handle_for_test(
            &target.id,
            crate::db::ConvState::LlmRequesting { attempt: 1 },
        )
        .await;

    let body = request(state.db.instance_id().await.unwrap(), exact(&target.id));
    let accepted = send(&app, &body, &bearer).await;
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    assert_eq!(
        acceptance(&accepted).disposition,
        RemoteSendDisposition::Queued
    );
    assert_eq!(
        acceptance(&accepted).accepting_transcript.as_str(),
        target.id
    );
    assert_eq!(effect_counts(&state).await, (0, 0, 1));

    let queue = state.db.get_steering_queue(&target.id).await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue[0].message_id,
        acceptance(&accepted).admitted_message_id
    );
    assert_eq!(queue[0].text, body.message.0);
    assert_eq!(queue[0].origin, body.sender.clone().into_origin(caller));

    let duplicate = send(&app, &body, &bearer).await;
    assert!(matches!(
        duplicate.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&duplicate), acceptance(&accepted));
    assert_eq!(effect_counts(&state).await, (0, 0, 1));
    let duplicate_queue = state.db.get_steering_queue(&target.id).await.unwrap();
    assert_eq!(duplicate_queue.len(), 1);
    assert_eq!(duplicate_queue[0].message_id, queue[0].message_id);
    assert_eq!(duplicate_queue[0].text, queue[0].text);
    assert_eq!(duplicate_queue[0].origin, queue[0].origin);

    let mut changed_sender = body.clone();
    changed_sender.sender = RemoteSenderAttestation::RemoteApi {};
    let sender_replay = send(&app, &changed_sender, &bearer).await;
    assert!(matches!(
        sender_replay.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&sender_replay), acceptance(&accepted));
    let sender_replay_queue = state.db.get_steering_queue(&target.id).await.unwrap();
    assert_eq!(sender_replay_queue.len(), 1);
    assert_eq!(sender_replay_queue[0].message_id, queue[0].message_id);
    assert_eq!(sender_replay_queue[0].text, queue[0].text);
    assert_eq!(sender_replay_queue[0].origin, queue[0].origin);

    let mut changed = body.clone();
    changed.message = RemoteMessageText::parse("changed text".into()).unwrap();
    let rejected = send(&app, &changed, &bearer).await;
    assert!(matches!(
        rejected.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::IdempotencyConflict,
            ..
        }
    ));
    assert_eq!(effect_counts(&state).await, (0, 0, 1));
    let conflict_queue = state.db.get_steering_queue(&target.id).await.unwrap();
    assert_eq!(conflict_queue.len(), 1);
    assert_eq!(conflict_queue[0].message_id, queue[0].message_id);
    assert_eq!(conflict_queue[0].text, queue[0].text);
    assert_eq!(conflict_queue[0].origin, queue[0].origin);
}

#[tokio::test]
async fn cancelled_request_keeps_detached_admission_and_gate_alive_until_durable() {
    let (state, app, caller, bearer, target) = fixture().await;
    let body = request(state.db.instance_id().await.unwrap(), exact(&target.id));
    let admitted_id = remote_admission_id(caller, body.message_id);
    let blocker = state.remote_message_admission.lock(admitted_id).await;

    let request_task = {
        let (app, body, bearer) = (app.clone(), body.clone(), bearer.clone());
        tokio::spawn(async move { route(&app, &body, Some(&bearer)).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while gate_strong_count(&state, admitted_id).await < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached admission did not queue on the keyed gate");

    request_task.abort();
    assert!(request_task.await.unwrap_err().is_cancelled());
    drop(blocker);

    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.remote_message_admission.lock(admitted_id),
    )
    .await
    .expect("detached admission did not finish");
    drop(completed);

    assert_eq!(effect_counts(&state).await, (1, 1, 0));
    let replay = send(&app, &body, &bearer).await;
    assert!(matches!(
        replay.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));
}

#[tokio::test]
async fn simultaneous_same_key_requests_admit_only_one_effect() {
    let (state, app, _, bearer, target) = fixture().await;
    let body = request(state.db.instance_id().await.unwrap(), exact(&target.id));
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (app, body, bearer, barrier) =
            (app.clone(), body.clone(), bearer.clone(), barrier.clone());
        tasks.spawn(async move {
            barrier.wait().await;
            send(&app, &body, &bearer).await
        });
    }
    let mut accepted = 0;
    let mut duplicate = 0;
    let mut prior = None;
    while let Some(result) = tasks.join_next().await {
        let response = result.unwrap();
        match response.receipt {
            RemoteSendReceipt::Accepted { .. } => accepted += 1,
            RemoteSendReceipt::Duplicate { .. } => duplicate += 1,
            RemoteSendReceipt::Rejected { .. } => {
                panic!("unexpected simultaneous receipt: {:?}", response.receipt)
            }
        }
        if let Some(prior) = &prior {
            assert_eq!(acceptance(&response), prior);
        } else {
            prior = Some(acceptance(&response).clone());
        }
    }
    assert_eq!((accepted, duplicate), (1, 7));
    assert_eq!(effect_counts(&state).await, (1, 1, 0));
}

#[tokio::test]
async fn unchanged_stable_retry_after_continuation_keeps_original_exact_acceptance() {
    let (state, app, _, bearer, original) = fixture().await;
    let body = request(
        state.db.instance_id().await.unwrap(),
        RemoteSendTarget::ProductConversation {
            product_conversation_id: original.product_conversation_id.clone(),
        },
    );
    let accepted = send(&app, &body, &bearer).await;
    assert!(matches!(
        accepted.receipt,
        RemoteSendReceipt::Accepted { .. }
    ));
    state
        .db
        .add_message(
            "handoff",
            &original.id,
            &crate::db::MessageContent::continuation("handoff"),
            None,
            None,
        )
        .await
        .unwrap();
    state
        .db
        .update_conversation_state(
            &original.id,
            &crate::db::ConvState::ContextExhausted {
                summary: "handoff".into(),
            },
        )
        .await
        .unwrap();
    let crate::db::ContinueOutcome::Created(successor) =
        state.db.continue_conversation(&original.id).await.unwrap()
    else {
        panic!("expected continuation")
    };
    assert_ne!(successor.id, original.id);
    assert_eq!(
        successor.product_conversation_id,
        original.product_conversation_id
    );
    let before = effect_counts(&state).await;
    let duplicate = send(&app, &body, &bearer).await;
    assert!(matches!(
        duplicate.receipt,
        RemoteSendReceipt::Duplicate { .. }
    ));
    assert_eq!(acceptance(&duplicate), acceptance(&accepted));
    assert_eq!(
        acceptance(&duplicate).accepting_transcript.as_str(),
        original.id
    );
    assert_eq!(effect_counts(&state).await, before);
    assert!(state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(successor.id),)
        .await
        .unwrap()
        .is_none());
}
