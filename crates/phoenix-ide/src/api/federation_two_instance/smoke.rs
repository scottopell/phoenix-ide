use phoenix_core::domain::{
    conversation_reference::ConversationReferenceTarget, db_schema::MessageContent,
    instance_identity::InstanceId,
};

use super::TestInstance;

#[tokio::test]
async fn real_router_rejects_unpaired_https_query_without_touching_production() {
    let receiver = TestInstance::start().await;
    let destination = receiver.state.db.instance_id().await.unwrap();
    let request = serde_json::json!({
        "caller_instance_id": InstanceId::new(),
        "destination_instance_id": destination,
        "sql": "SELECT 1",
    });
    let response = receiver
        .client
        .post(receiver.endpoint("api/federation/peer/query-database"))
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(receiver.state.db.instance_id().await.unwrap(), destination);
    receiver.stop().await;
}

fn assert_authoritative_remote_hit(
    hits: &[crate::api::global_read::QualifiedSearchHit],
    destination: InstanceId,
    receiver: &TestInstance,
    ordinary: &crate::db::Conversation,
    ordinary_message: &crate::db::Message,
) {
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source.instance_id, destination);
    assert!(matches!(
        &hits[0].source_navigation,
        Some(crate::api::global_read::QualifiedSearchNavigation::Peer { url })
            if url == &format!(
                "{}c/{}?source_transcript={}#message-{}",
                receiver.origin,
                ordinary.id,
                ordinary.id,
                ordinary_message.message_id,
            )
    ));
    assert!(matches!(
        &hits[0].source.target,
        ConversationReferenceTarget::Message {
            transcript_id,
            message_id,
        } if transcript_id.as_str() == ordinary.id && message_id.as_str() == ordinary_message.message_id
    ));
    assert!(matches!(
        hits[0].stable_conversation.as_ref(),
        Some(reference)
            if reference.instance_id == destination
                && matches!(
                    &reference.target,
                    ConversationReferenceTarget::ProductConversation { product_conversation_id }
                        if product_conversation_id == &ordinary.product_conversation_id
                )
    ));
    assert!(matches!(
        &hits[0].stable_navigation,
        Some(crate::api::global_read::QualifiedSearchNavigation::Peer { url })
            if url == &format!("{}c/{}", receiver.origin, ordinary.product_conversation_id)
    ));
}

#[tokio::test]
async fn authenticated_https_search_returns_authoritative_identities() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let caller_id = caller.state.db.instance_id().await.unwrap();
    let destination = receiver.state.db.instance_id().await.unwrap();
    let ordinary = receiver
        .state
        .db
        .create_conversation(
            "federation-search-transcript",
            "federation-search",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let ordinary_message = receiver
        .state
        .db
        .add_message(
            "federation-search-message",
            &ordinary.id,
            &MessageContent::user("federation searchable marker"),
            None,
            None,
        )
        .await
        .unwrap();
    let coordinator_id = receiver
        .state
        .db
        .coordinator_conversation_id()
        .await
        .unwrap()
        .unwrap();
    receiver
        .state
        .db
        .add_message(
            "federation-coordinator-message",
            &coordinator_id,
            &MessageContent::user("federation searchable marker"),
            None,
            None,
        )
        .await
        .unwrap();

    let unauthenticated = receiver
        .client
        .post(receiver.endpoint("api/federation/peer/search-conversations"))
        .json(&crate::api::federation::RemoteSearchRequest {
            caller_instance_id: caller_id,
            destination_instance_id: destination,
            query: "federation searchable marker".to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);

    let peer = caller.connect_to(&receiver).await;
    let hits = crate::api::federation::search_remote_conversations(
        &caller.state.db,
        &peer,
        "federation searchable marker",
    )
    .await
    .unwrap();
    assert_authoritative_remote_hit(&hits, destination, &receiver, &ordinary, &ordinary_message);
    assert_ne!(caller_id, destination);
    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
async fn authenticated_https_search_preserves_index_unavailable_reason() {
    let caller = TestInstance::start().await;
    let receiver = TestInstance::start_with_search_ready(false).await;
    let caller_id = caller.state.db.instance_id().await.unwrap();
    let destination = receiver.state.db.instance_id().await.unwrap();
    let credential = receiver.enroll(caller_id).await;
    let response = receiver
        .client
        .post(receiver.endpoint("api/federation/peer/search-conversations"))
        .bearer_auth(credential.expose())
        .json(&crate::api::federation::RemoteSearchRequest {
            caller_instance_id: caller_id,
            destination_instance_id: destination,
            query: "still warming".to_string(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({
            "code": "search_index_unavailable",
            "message": "the global message index is still warming; try again after startup reconciliation completes",
        })
    );

    let peer = caller.save_connection(&receiver, &credential).await;
    let error = crate::api::federation::search_remote_conversations(
        &caller.state.db,
        &peer,
        "still warming",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        crate::api::federation::FederationClientError::SearchUnavailable(message)
            if message.contains("still warming")
    ));

    caller.stop().await;
    receiver.stop().await;
}
