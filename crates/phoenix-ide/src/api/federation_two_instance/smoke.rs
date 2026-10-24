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
        .post(receiver.endpoint("/api/federation/peer/query-database"))
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(receiver.state.db.instance_id().await.unwrap(), destination);
    receiver.stop().await;
}

async fn assert_authoritative_remote_hit(
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
    let product = receiver
        .state
        .db
        .get_ordinary_product_conversation(&ordinary.product_conversation_id)
        .await
        .unwrap();
    let stable_path = crate::api::product_conversations::product_route(
        product.root.conversation.slug.as_deref(),
        ordinary.product_conversation_id.as_str(),
    );
    let expected_navigation = receiver.endpoint(&stable_path);
    assert!(
        matches!(
            &hits[0].stable_navigation,
            Some(crate::api::global_read::QualifiedSearchNavigation::Peer { url })
                if url == &expected_navigation
        ),
        "stable navigation {:?}, expected {expected_navigation}",
        hits[0].stable_navigation
    );
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
        .post(receiver.endpoint("/api/federation/peer/search-conversations"))
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
    assert_authoritative_remote_hit(&hits, destination, &receiver, &ordinary, &ordinary_message)
        .await;
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
        .post(receiver.endpoint("/api/federation/peer/search-conversations"))
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

async fn assert_oversized_remote_target_is_bounded(
    caller: &TestInstance,
    receiver: &TestInstance,
    peer: &phoenix_db::FederationPeerConnection,
    conversation: &crate::db::Conversation,
) {
    use phoenix_core::domain::conversation_reference::{ConversationReferenceTarget, MessageId};

    let oversized = receiver
        .state
        .db
        .add_message(
            "oversized-remote-read-message",
            &conversation.id,
            &MessageContent::user("oversized remote target ".repeat(5_000)),
            None,
            None,
        )
        .await
        .unwrap();
    let target = ConversationReferenceTarget::Message {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
            conversation.id.clone(),
        )
        .unwrap(),
        message_id: MessageId::parse(oversized.message_id).unwrap(),
    };
    let read = crate::api::federation::read_remote_conversation(&caller.state.db, peer, target, 0)
        .await
        .unwrap();
    assert!(read.content.contains("oversized remote target"));
    assert!(read.content.contains("target message truncated"));
    assert!(!read.content.contains("](/c/"));
}

async fn assert_three_remote_pages(
    caller: &TestInstance,
    receiver: &TestInstance,
    peer: &phoenix_db::FederationPeerConnection,
    conversation: &crate::db::Conversation,
    target: ConversationReferenceTarget,
) {
    let oversized = receiver
        .state
        .db
        .add_message(
            "oversized-remote-page-message",
            &conversation.id,
            &MessageContent::user("oversized remote page ".repeat(5_000)),
            None,
            None,
        )
        .await
        .unwrap();
    for index in 0..130 {
        receiver
            .state
            .db
            .add_message(
                &format!("remote-page-message-{index}"),
                &conversation.id,
                &MessageContent::user(format!("remote page marker {index}")),
                None,
                None,
            )
            .await
            .unwrap();
    }
    let second =
        crate::api::federation::read_remote_conversation(&caller.state.db, peer, target.clone(), 1)
            .await
            .unwrap();
    assert!(second.content.contains("oversized remote page"));
    assert!(second.content.contains("message truncated"));
    assert_eq!(
        second.next_cursor,
        usize::try_from(oversized.sequence_id).ok()
    );
    let second = crate::api::federation::read_remote_conversation(
        &caller.state.db,
        peer,
        target.clone(),
        second.next_cursor.unwrap(),
    )
    .await
    .unwrap();
    assert!(second.content.contains("remote page marker 63"));
    let destination = receiver.state.db.instance_id().await.unwrap();
    assert!(!second.evidence.is_empty());
    assert!(second.evidence.iter().all(|reference| {
        reference.instance_id == destination
            && matches!(
                &reference.target,
                ConversationReferenceTarget::Message { .. }
            )
    }));
    let third = crate::api::federation::read_remote_conversation(
        &caller.state.db,
        peer,
        target,
        second.next_cursor.unwrap(),
    )
    .await
    .unwrap();
    assert!(third.content.contains("remote page marker 127"));
    assert!(!third.content.contains("](/c/"));
}

async fn assert_exact_remote_read(
    caller: &TestInstance,
    receiver: &TestInstance,
    peer: &phoenix_db::FederationPeerConnection,
    conversation: &crate::db::Conversation,
    message: &crate::db::Message,
    destination: InstanceId,
) {
    use phoenix_core::domain::conversation_reference::{ConversationReferenceTarget, MessageId};

    let target = ConversationReferenceTarget::Message {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
            conversation.id.clone(),
        )
        .unwrap(),
        message_id: MessageId::parse(message.message_id.clone()).unwrap(),
    };
    let exact = crate::api::federation::read_remote_conversation(
        &caller.state.db,
        peer,
        target.clone(),
        usize::MAX,
    )
    .await
    .unwrap();
    assert_eq!(exact.requested.target, target);
    assert_eq!(exact.resolved.target, exact.requested.target);
    assert!(matches!(
        exact.stable_conversation.as_ref(),
        Some(reference)
            if reference.instance_id == destination
                && matches!(
                    &reference.target,
                    ConversationReferenceTarget::ProductConversation { product_conversation_id }
                        if product_conversation_id.as_str()
                            == conversation.product_conversation_id.as_str()
                )
    ));
    assert!(exact.content.contains("exact evidence:"));
    assert!(exact
        .content
        .contains("target_message: remote-read-message"));
    assert_oversized_remote_target_is_bounded(caller, receiver, peer, conversation).await;
}

#[tokio::test]
async fn authenticated_https_read_preserves_stable_and_exact_identity() {
    use phoenix_core::domain::{
        conversation_reference::ConversationReferenceTarget,
        product_conversation::ProductConversationId,
    };

    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let destination = receiver.state.db.instance_id().await.unwrap();
    let conversation = receiver
        .state
        .db
        .create_conversation(
            "remote-read-transcript",
            "remote-read",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let message = receiver
        .state
        .db
        .add_message(
            "remote-read-message",
            &conversation.id,
            &MessageContent::user("remote read marker"),
            None,
            None,
        )
        .await
        .unwrap();
    let peer = caller.connect_to(&receiver).await;
    let stable_target = ConversationReferenceTarget::ProductConversation {
        product_conversation_id: ProductConversationId::parse(
            conversation.product_conversation_id.to_string(),
        )
        .unwrap(),
    };
    let stable = crate::api::federation::read_remote_conversation(
        &caller.state.db,
        &peer,
        stable_target.clone(),
        0,
    )
    .await
    .unwrap();
    assert_eq!(stable.requested.instance_id, destination);
    assert_eq!(stable.requested.target, stable_target);
    assert!(matches!(
        &stable.resolved.target,
        ConversationReferenceTarget::Transcript { transcript_id }
            if transcript_id.as_str() == conversation.id
    ));
    assert!(stable.content.contains("remote read marker"));
    assert!(!stable.content.contains("conversation link:"));
    assert!(matches!(
        &stable.source_navigation,
        crate::api::global_read::QualifiedSearchNavigation::Peer { url }
            if url.starts_with(
                receiver.origin.navigation_url("/").unwrap().as_url().as_str()
            )
    ));

    assert_three_remote_pages(
        &caller,
        &receiver,
        &peer,
        &conversation,
        stable.requested.target.clone(),
    )
    .await;

    assert_exact_remote_read(
        &caller,
        &receiver,
        &peer,
        &conversation,
        &message,
        destination,
    )
    .await;

    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
async fn authenticated_https_resolve_returns_destination_provenance() {
    use phoenix_core::domain::{
        conversation_reference::ConversationReferenceTarget,
        product_conversation::ProductConversationId,
    };

    let caller = TestInstance::start().await;
    let receiver = TestInstance::start().await;
    let destination = receiver.state.db.instance_id().await.unwrap();
    let conversation = receiver
        .state
        .db
        .create_conversation(
            "remote-resolve-transcript",
            "remote-resolve",
            "/receiver-owned/worktree",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let product_id =
        ProductConversationId::parse(conversation.product_conversation_id.to_string()).unwrap();
    let peer = caller.connect_to(&receiver).await;

    let resolution = crate::api::federation::resolve_remote_reference(
        &caller.state.db,
        &peer,
        format!("@conv:{product_id}"),
    )
    .await
    .unwrap();
    assert_eq!(resolution.reference.instance_id, destination);
    assert_eq!(resolution.path_owner_instance_id, Some(destination));
    assert!(matches!(
        &resolution.work_scope,
        crate::api::global_read::ResolvedWorkScope::Available {
            effective_path: Some(path),
            ..
        } if path == "/receiver-owned/worktree"
    ));
    assert!(!resolution.related_references.is_empty());
    assert!(resolution
        .related_references
        .iter()
        .all(|reference| reference.instance_id == destination));
    assert!(matches!(
        resolution.reference.target,
        ConversationReferenceTarget::ProductConversation {
            product_conversation_id,
        } if product_conversation_id == product_id
    ));
    assert!(resolution.href.as_deref().is_some_and(
        |href| href.starts_with("https://localhost:") && href.ends_with("/c/remote-resolve")
    ));

    let error = crate::api::federation::resolve_remote_reference(
        &caller.state.db,
        &peer,
        "@conv:missing-product".to_string(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        crate::api::federation::FederationClientError::ResolveInvalidReference(_)
    ));

    caller.stop().await;
    receiver.stop().await;
}

#[tokio::test]
async fn owner_https_facades_preserve_current_client_envelopes_and_auth_separation() {
    let (caller, receiver, peer) = owner_facade_instances().await;
    let destination = peer.peer_instance_id;
    let caller_id = caller.state.db.instance_id().await.unwrap();
    let conversation = receiver
        .state
        .db
        .create_conversation(
            "owner-facade-transcript",
            "owner-facade",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    receiver
        .state
        .db
        .add_message(
            "owner-facade-message",
            &conversation.id,
            &MessageContent::user("uniquefacademarker"),
            None,
            None,
        )
        .await
        .unwrap();
    receiver.state.db.fts_retriever().reconcile().await.unwrap();
    let target = ConversationReferenceTarget::Transcript {
        transcript_id: phoenix_core::domain::close::TranscriptConversationId::parse(
            conversation.id.clone(),
        )
        .unwrap(),
    };
    for (operation, request) in [
        (
            "query-database",
            serde_json::json!({"instance": destination, "sql": "SELECT 1 AS n"}),
        ),
        (
            "search-conversations",
            serde_json::json!({"instance": destination, "query": "uniquefacademarker"}),
        ),
        (
            "read-conversation",
            serde_json::json!({"instance": destination, "target": target, "cursor": 0}),
        ),
        (
            "resolve-reference",
            serde_json::json!({"instance": destination, "reference": format!("@transcript:{}", conversation.id)}),
        ),
        (
            "send-conversation-message",
            serde_json::json!({"instance": destination, "target": target, "message": "literal owner API message", "message_id": uuid::Uuid::new_v4()}),
        ),
        ("openapi", serde_json::json!({"instance": destination})),
    ] {
        let endpoint = caller.endpoint(&format!("/api/federation/{operation}"));
        let rejected = caller
            .client
            .post(&endpoint)
            .bearer_auth(peer.bearer_credential.expose())
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
        let response = caller
            .client
            .post(&endpoint)
            .bearer_auth("local-owner-password")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{operation}: {}",
            response.status()
        );
        let response: serde_json::Value = response.json().await.unwrap();
        assert!(!response
            .to_string()
            .contains(peer.bearer_credential.expose()));
        assert_owner_facade_response(operation, &response, &request, destination, caller_id);
    }
    let rejected = receiver
        .client
        .post(receiver.endpoint("/api/federation/peer/query-database"))
        .bearer_auth("remote-owner-password")
        .json(&serde_json::json!({"destination_instance_id": destination, "sql": "SELECT 1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
    caller.stop().await;
    receiver.stop().await;
}

async fn owner_facade_instances() -> (
    TestInstance,
    TestInstance,
    phoenix_db::FederationPeerConnection,
) {
    use phoenix_core::domain::instance_identity::OwnerPassword;
    let caller = TestInstance::start_with_router(true, |mut state| {
        state.password = Some(OwnerPassword::parse("local-owner-password".into()).unwrap());
        super::create_router(state)
    })
    .await;
    let receiver = TestInstance::start_with_router(true, |mut state| {
        state.password = Some(OwnerPassword::parse("remote-owner-password".into()).unwrap());
        super::create_router(state)
    })
    .await;
    let peer = caller.connect_to(&receiver).await;
    (caller, receiver, peer)
}

fn assert_owner_facade_response(
    operation: &str,
    response: &serde_json::Value,
    request: &serde_json::Value,
    destination: InstanceId,
    caller_id: InstanceId,
) {
    match operation {
        "query-database" => {
            assert_eq!(response["destination_instance_id"], destination.to_string());
            assert_eq!(response["caller_instance_id"], caller_id.to_string());
            for key in [
                "columns",
                "rows",
                "truncated",
                "row_limit",
                "byte_limit",
                "elapsed_ms",
            ] {
                assert!(response["result"].get(key).is_some(), "{key}");
            }
        }
        "search-conversations" => {
            assert_eq!(response["destination_instance_id"], destination.to_string());
            let hits = response["hits"].as_array().unwrap();
            assert_eq!(hits.len(), 1);
            for key in [
                "source",
                "source_navigation",
                "stable_conversation",
                "stable_navigation",
                "role",
                "sender",
                "created_at",
                "snippet",
            ] {
                assert!(hits[0].get(key).is_some(), "{key}");
            }
            assert_eq!(hits[0]["source"]["instance_id"], destination.to_string());
        }
        "read-conversation" => {
            assert_eq!(
                response["requested"]["instance_id"],
                destination.to_string()
            );
            assert_eq!(response["resolved"]["target"], request["target"]);
            for key in [
                "stable_conversation",
                "evidence",
                "content",
                "next_cursor",
                "source_navigation",
                "stable_navigation",
            ] {
                assert!(response.get(key).is_some(), "{key}");
            }
            assert!(response.get("read").is_none());
        }
        "resolve-reference" => {
            for key in [
                "reference",
                "href",
                "title",
                "work_scope",
                "path_owner_instance_id",
                "related_references",
                "summary",
            ] {
                assert!(response.get(key).is_some(), "{key}");
            }
        }
        "send-conversation-message" => {
            assert_eq!(response["destination_instance_id"], destination.to_string());
            assert_eq!(response["caller_instance_id"], caller_id.to_string());
            assert_eq!(response["target"], request["target"]);
            assert_eq!(response["message_id"], request["message_id"]);
            assert!(response.get("receipt").is_some());
        }
        "openapi" => {
            assert_eq!(response["destination_instance_id"], destination.to_string());
            assert_eq!(
                response["document"]["x-phoenix-instance-id"],
                destination.to_string()
            );
            assert_eq!(response["document"]["openapi"], "3.1.0");
            assert!(response.get("openapi").is_none());
            assert!(!response["document"]["paths"]
                .as_object()
                .unwrap()
                .is_empty());
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn owner_openapi_https_rejects_oversized_wrong_identity_malformed_and_redirect_documents() {
    for bad in ["oversized", "identity", "malformed", "redirect"] {
        let caller = TestInstance::start().await;
        let receiver = TestInstance::start_with_router(false, move |state| {
            axum::Router::new().route("/openapi.json", axum::routing::get(move || async move {
                use axum::response::IntoResponse;
                match bad {
                    "oversized" => (axum::http::StatusCode::OK, "x".repeat(65537)).into_response(),
                    "identity" => axum::Json(serde_json::json!({"openapi": "3.1.0", "x-phoenix-instance-id": InstanceId::new(), "info": {"title": "reference", "version": "1", "description": "reference"}, "paths": {"/openapi.json": {}}, "components": {"schemas": {}, "securitySchemes": {}}})).into_response(),
                    "malformed" => axum::Json(serde_json::json!({"x-phoenix-instance-id": state.db.instance_id().await.unwrap()})).into_response(),
                    "redirect" => (axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, "https://external.invalid/openapi.json")]).into_response(),
                    _ => unreachable!(),
                }
            }))
        }).await;
        let peer = caller.connect_to(&receiver).await;
        let response = caller
            .client
            .post(caller.endpoint("/api/federation/openapi"))
            .json(&serde_json::json!({"instance": peer.peer_instance_id}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY, "{bad}");
        caller.stop().await;
        receiver.stop().await;
    }
}
