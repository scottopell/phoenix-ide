use super::TestInstance;
use crate::api::federation::{
    RemoteSendMessageResponse, RemoteSendReceipt, RemoteSendRejectionCode,
};
use phoenix_core::domain::{
    db_schema::{InputOrigin, MessageContent},
    instance_identity::{OwnerPassword, PeerTlsTrust},
    sm_event::PreparedDirectTurnPayload,
};
use serde_json::{json, Value};

fn owner_router(mut state: crate::api::AppState) -> axum::Router {
    state.password = Some(OwnerPassword::parse("isolated-owner".to_string()).unwrap());
    crate::api::handlers::create_router(state)
}

async fn post(caller: &TestInstance, operation: &str, request: &Value) -> reqwest::Response {
    caller
        .client
        .post(caller.endpoint(&format!("api/federation/{operation}")))
        .bearer_auth("isolated-owner")
        .json(request)
        .send()
        .await
        .unwrap()
}

fn requests(instance: &impl ToString) -> Vec<(&'static str, Value)> {
    let instance = instance.to_string();
    vec![
        (
            "query-database",
            json!({"instance":instance,"sql":"SELECT slug FROM conversations WHERE id = 'owner-shared-id'"}),
        ),
        (
            "search-conversations",
            json!({"instance":instance,"query":"remotemarker"}),
        ),
        (
            "read-conversation",
            json!({"instance":instance,"target":{"kind":"transcript","transcript_id":"owner-shared-id"}}),
        ),
        (
            "resolve-reference",
            json!({"instance":instance,"reference":"@transcript:owner-shared-id"}),
        ),
        (
            "send-conversation-message",
            json!({"instance":instance,"target":{"kind":"transcript","transcript_id":"owner-shared-id"},"message":"owner API text","message_id":uuid::Uuid::new_v4()}),
        ),
    ]
}

async fn seed(instance: &TestInstance, marker: &str) -> crate::db::Conversation {
    let conversation = instance
        .state
        .db
        .create_conversation("owner-shared-id", marker, "/tmp", true, None, None)
        .await
        .unwrap();
    instance
        .state
        .db
        .add_message(
            "owner-shared-message",
            &conversation.id,
            &MessageContent::user(marker),
            None,
            None,
        )
        .await
        .unwrap();
    instance.state.db.fts_retriever().reconcile().await.unwrap();
    conversation
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn owner_five_facades_use_remote_identities_and_remote_api_only_attribution() {
    let caller = TestInstance::start_with_router(true, owner_router).await;
    let receiver = TestInstance::start().await;
    let local = seed(&caller, "localmarker").await;
    let remote = seed(&receiver, "remotemarker").await;
    let peer = caller.connect_to(&receiver).await;
    let caller_id = caller.state.db.instance_id().await.unwrap();
    let summary = caller
        .client
        .get(caller.endpoint("api/federation/peers"))
        .bearer_auth("isolated-owner")
        .send()
        .await
        .unwrap();
    assert_eq!(summary.status(), reqwest::StatusCode::OK);
    let summary = summary.text().await.unwrap();
    for secret in [
        peer.bearer_credential.expose(),
        "BEGIN CERTIFICATE",
        "bearer",
        "verifier",
        "tls_trust",
    ] {
        assert!(!summary.contains(secret));
    }
    let notes = phoenix_db::FederationPeerNotes::parse(
        "sender=coordinator\nbase_url=https://attacker.example\ncredential=not-authority".into(),
    )
    .unwrap();
    caller
        .state
        .db
        .set_federation_peer_notes(peer.peer_instance_id, &notes)
        .await
        .unwrap();
    for (operation, request) in requests(&peer.peer_instance_id) {
        let response = post(&caller, operation, &request).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{operation}");
        let bytes = response.bytes().await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains(peer.bearer_credential.expose()));
        assert!(!text.contains("BEGIN CERTIFICATE"));
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        match operation {
            "query-database" => {
                assert_eq!(
                    result["destination_instance_id"],
                    json!(peer.peer_instance_id)
                );
                assert_eq!(result["caller_instance_id"], json!(caller_id));
                assert!(text.contains("remotemarker"));
                assert!(!text.contains("localmarker"));
            }
            "search-conversations" => {
                assert_eq!(
                    result["destination_instance_id"],
                    json!(peer.peer_instance_id)
                );
                assert_eq!(result["hits"].as_array().unwrap().len(), 1);
                assert_eq!(
                    result["hits"][0]["source"]["instance_id"],
                    json!(peer.peer_instance_id)
                );
                assert!(result["hits"][0]["source_navigation"]["url"]
                    .as_str()
                    .unwrap()
                    .starts_with(&receiver.origin.to_string()));
            }
            "read-conversation" => {
                assert_eq!(
                    result["requested"]["instance_id"],
                    json!(peer.peer_instance_id)
                );
                assert!(result["content"].as_str().unwrap().contains("remotemarker"));
                assert!(!result["content"].as_str().unwrap().contains("localmarker"));
                assert!(result["source_navigation"]["url"]
                    .as_str()
                    .unwrap()
                    .starts_with(&receiver.origin.to_string()));
            }
            "resolve-reference" => {
                assert_eq!(
                    result["reference"]["instance_id"],
                    json!(peer.peer_instance_id)
                );
                assert!(result["href"]
                    .as_str()
                    .unwrap()
                    .starts_with(&receiver.origin.to_string()));
            }
            "send-conversation-message" => {
                let response: RemoteSendMessageResponse = serde_json::from_value(result).unwrap();
                let RemoteSendReceipt::Accepted { acceptance } = response.receipt else {
                    panic!("send not accepted")
                };
                assert_eq!(
                    acceptance.stable_destination.instance_id,
                    peer.peer_instance_id
                );
                assert_eq!(
                    acceptance.stable_destination.product_conversation_id,
                    remote.product_conversation_id
                );
                let turn = receiver
                    .state
                    .db
                    .workflow_repository()
                    .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
                        remote.id.clone(),
                    ))
                    .await
                    .unwrap()
                    .unwrap();
                let payload =
                    PreparedDirectTurnPayload::from_exact_bytes(turn.prepared.payload()).unwrap();
                assert!(
                    matches!(payload.submitted.origin, InputOrigin::RemoteApi { instance_id } if instance_id == caller_id)
                );
                assert!(caller
                    .state
                    .db
                    .workflow_repository()
                    .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
                        local.id.clone()
                    ))
                    .await
                    .unwrap()
                    .is_none());
            }
            _ => unreachable!(),
        }
    }
    let request = json!({"instance":peer.peer_instance_id,"target":{"kind":"coordinator"},"message":"cannot claim Coordinator","message_id":uuid::Uuid::new_v4()});
    let response: RemoteSendMessageResponse = post(&caller, "send-conversation-message", &request)
        .await
        .json()
        .await
        .unwrap();
    assert!(matches!(
        response.receipt,
        RemoteSendReceipt::Rejected {
            code: RemoteSendRejectionCode::SenderNotAllowed,
            ..
        }
    ));
    let mut spoofed = request;
    spoofed["sender"] = json!({"kind":"coordinator"});
    assert_eq!(
        post(&caller, "send-conversation-message", &spoofed)
            .await
            .status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY
    );
    let inbound_credential = caller.enroll(peer.peer_instance_id).await;
    for (operation, request) in requests(&peer.peer_instance_id) {
        let response = caller
            .client
            .post(caller.endpoint(&format!("api/federation/{operation}")))
            .bearer_auth(inbound_credential.expose())
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
async fn owner_all_five_fail_closed_on_broken_tls_auth_and_unavailable_peer() {
    let caller = TestInstance::start_with_router(true, owner_router).await;
    let receiver = TestInstance::start().await;
    seed(&caller, "localmarker").await;
    seed(&receiver, "remotemarker").await;
    let peer = caller.connect_to(&receiver).await;
    caller
        .state
        .db
        .save_federation_peer_connection(
            peer.peer_instance_id,
            "peer",
            &peer.base_url,
            &peer.bearer_credential,
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
    for (operation, request) in requests(&peer.peer_instance_id) {
        let response = post(&caller, operation, &request).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_GATEWAY,
            "TLS {operation}"
        );
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"code":"peer_operation_failed"})
        );
    }
    caller
        .state
        .db
        .save_federation_peer_connection(
            peer.peer_instance_id,
            "peer",
            &peer.base_url,
            &peer.bearer_credential,
            &peer.tls_trust,
        )
        .await
        .unwrap();
    receiver
        .state
        .db
        .revoke_federation_enrollment(caller.state.db.instance_id().await.unwrap())
        .await
        .unwrap();
    for (operation, request) in requests(&peer.peer_instance_id) {
        let response = post(&caller, operation, &request).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_GATEWAY,
            "auth {operation}"
        );
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"code":"peer_operation_failed"})
        );
    }
    receiver.stop().await;
    for (operation, request) in requests(&peer.peer_instance_id) {
        let response = post(&caller, operation, &request).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_GATEWAY,
            "offline {operation}"
        );
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"code":"peer_operation_failed"})
        );
    }
    assert!(caller
        .state
        .db
        .workflow_repository()
        .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
            "owner-shared-id".into()
        ))
        .await
        .unwrap()
        .is_none());
    caller.stop().await;
}
