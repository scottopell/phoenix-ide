use super::*;
use crate::api::handlers::hard_delete_cascade_tests::make_test_state;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use phoenix_core::domain::db_schema::MessageContent;
use serde_json::{json, Value};
use tower::ServiceExt;

const REFERENCES: &[&str] = &[
    ".",
    "..",
    "雪",
    "%E9%9B%AA",
    "%2F",
    "/",
    "a/b?c#d",
    "a\\b",
    "a+b",
    "%",
    "%ZZ",
    " leading ",
];
const TRANSCRIPT_READS: &[&str] = &[
    "/api/conversation-reads/snapshot",
    "/api/conversation-reads/messages/latest?limit=2",
    "/api/conversation-reads/messages?before_message_sequence=2&limit=2",
    "/api/conversation-reads/messages?after_message_sequence=0&limit=2",
    "/api/conversation-reads/messages/range?start_message_sequence=1&end_message_sequence=1",
    "/api/conversation-reads/messages/around/2?before=2&after=2",
];

fn request(uri: &str, body: &str) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn read(router: &Router, uri: &str, reference: &str) -> Value {
    let response = router
        .clone()
        .oneshot(request(uri, &json!({"reference": reference}).to_string()))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{uri} {reference:?}: {bytes:?}");
    serde_json::from_slice(&bytes).unwrap()
}

fn assert_message(value: &Value, message_id: &str) {
    let messages = value
        .get("messages")
        .or_else(|| value.get("before"))
        .unwrap();
    assert_eq!(messages.as_array().unwrap().len(), 1, "{value}");
    assert_eq!(messages[0]["message_id"], message_id);
}

#[tokio::test]
async fn exact_read_transcript_and_ordinary_history_preserve_opaque_references() {
    let state = make_test_state().await;
    for (index, reference) in REFERENCES.iter().enumerate() {
        let conversation = state
            .db
            .create_conversation(
                reference,
                &format!("opaque-{index}"),
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(conversation.id, *reference);
        let message_id = format!("message-{index}");
        state
            .db
            .add_message(
                &message_id,
                reference,
                &MessageContent::user(*reference),
                None,
                None,
            )
            .await
            .unwrap();
    }
    let router = create_router(state.clone());
    for (index, reference) in REFERENCES.iter().enumerate() {
        for uri in TRANSCRIPT_READS {
            let value = read(&router, uri, reference).await;
            assert_message(&value, &format!("message-{index}"));
            if uri.ends_with("snapshot") {
                assert_eq!(value["conversation"]["id"], *reference);
            }
        }
        for suffix in ["", "?boundary_representation=original_messages"] {
            let value = read(
                &router,
                &format!("/api/product-conversation-reads/snapshot{suffix}"),
                reference,
            )
            .await;
            assert_eq!(value["requested_transcript_row_id"], *reference);
            assert_eq!(value["segments"][0]["transcript_row_id"], *reference);
            assert_message(&value["segments"][0], &format!("message-{index}"));
        }
    }
    state
        .db
        .create_conversation("ordinary", "ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    for (old_uri, new_uri) in [
        (
            "/api/conversations/ordinary",
            "/api/conversation-reads/snapshot",
        ),
        (
            "/api/product-conversations/ordinary",
            "/api/product-conversation-reads/snapshot",
        ),
        (
            "/api/product-conversations/ordinary?boundary_representation=original_messages",
            "/api/product-conversation-reads/snapshot?boundary_representation=original_messages",
        ),
    ] {
        let old = router
            .clone()
            .oneshot(Request::get(old_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(old.status(), StatusCode::OK);
        let old: Value =
            serde_json::from_slice(&to_bytes(old.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert_eq!(read(&router, new_uri, "ordinary").await, old);
    }
}

#[tokio::test]
async fn exact_read_product_ids_follow_the_nonblank_text_schema_contract() {
    let state = make_test_state().await;
    state
        .db
        .create_conversation("template", "template", "/tmp", true, None, None)
        .await
        .unwrap();
    for (index, reference) in REFERENCES.iter().enumerate() {
        let product_id =
            phoenix_core::domain::product_conversation::ProductConversationId::parse(*reference)
                .unwrap();
        sqlx::query("INSERT INTO product_conversations (id, kind, ordinary_lifecycle) VALUES (?1, 'ordinary', 'open')")
            .bind(product_id.as_str()).execute(state.db.pool()).await.unwrap();
        let transcript_id = format!("product-transcript-{index}");
        sqlx::query("INSERT INTO conversations (id, product_conversation_id, slug, title, user_initiated, state, state_kind, state_updated_at, created_at, updated_at, archived, transcript_generation, model, llm_language, cm_kind, runtime_role)
            SELECT ?1, ?2, ?1, title, user_initiated, state, state_kind, state_updated_at, created_at, updated_at, archived, transcript_generation, model, llm_language, cm_kind, runtime_role FROM conversations WHERE id = 'template'")
            .bind(&transcript_id).bind(product_id.as_str()).execute(state.db.pool()).await.unwrap();
    }
    let router = create_router(state);
    for (index, reference) in REFERENCES.iter().enumerate() {
        for suffix in ["", "?boundary_representation=original_messages"] {
            let value = read(
                &router,
                &format!("/api/product-conversation-reads/snapshot{suffix}"),
                reference,
            )
            .await;
            assert_eq!(value["product_conversation_id"], *reference);
            assert_eq!(
                value["segments"][0]["transcript_row_id"],
                format!("product-transcript-{index}")
            );
        }
    }
}

#[tokio::test]
async fn exact_read_coordinator_routes_and_history_keep_membership() {
    let state = make_test_state().await;
    let coordinator = state
        .db
        .get_or_create_coordinator(None, crate::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    state
        .db
        .add_message(
            "global-constructor",
            &coordinator.id,
            &MessageContent::user("Constructor-created Coordinator evidence"),
            None,
            None,
        )
        .await
        .unwrap();
    let router = create_router(state.clone());
    for (old_path, new_uri) in [
        (
            "/api/global/coordinator/route",
            "/api/global/coordinator/reads/route",
        ),
        (
            "/api/global/coordinator/history",
            "/api/global/coordinator/reads/history",
        ),
    ] {
        let exact = read(&router, new_uri, &coordinator.id).await;
        if new_uri.ends_with("route") {
            assert_eq!(exact["coordinator_id"], coordinator.id);
        } else {
            assert_eq!(exact["segments"][0]["transcript_row_id"], coordinator.id);
            assert_message(&exact["segments"][0], "global-constructor");
        }
        let legacy = router
            .clone()
            .oneshot(
                Request::get(format!("{old_path}/{}", coordinator.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::OK);
        let legacy: Value =
            serde_json::from_slice(&to_bytes(legacy.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(exact, legacy);
    }
    sqlx::query("DELETE FROM messages WHERE conversation_id = ?1")
        .bind(&coordinator.id)
        .execute(state.db.pool())
        .await
        .unwrap();

    // No messages or continuation edges exist yet; changing this fixture key exercises the TEXT schema contract.
    let mut current = coordinator.id;
    for reference in REFERENCES {
        sqlx::query("UPDATE conversations SET id = ?1 WHERE id = ?2")
            .bind(reference)
            .bind(&current)
            .execute(state.db.pool())
            .await
            .unwrap();
        current = (*reference).to_owned();
        state
            .db
            .add_message(
                "global-exact",
                reference,
                &MessageContent::user(*reference),
                None,
                None,
            )
            .await
            .unwrap();
        let router = create_router(state.clone());
        let route = read(&router, "/api/global/coordinator/reads/route", reference).await;
        assert_eq!(route["coordinator_id"], *reference);
        let history = read(&router, "/api/global/coordinator/reads/history", reference).await;
        assert_eq!(history["segments"][0]["transcript_row_id"], *reference);
        assert_message(&history["segments"][0], "global-exact");
        let rejected = router
            .oneshot(request(
                "/api/product-conversation-reads/snapshot",
                &json!({"reference": reference}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
        sqlx::query("DELETE FROM messages WHERE conversation_id = ?1")
            .bind(reference)
            .execute(state.db.pool())
            .await
            .unwrap();
    }
    let ordinary = state
        .db
        .create_conversation("foreign", "foreign", "/tmp", true, None, None)
        .await
        .unwrap();
    let router = create_router(state.clone());
    assert!(
        read(&router, "/api/global/coordinator/reads/route", &ordinary.id).await["coordinator_id"]
            .is_null()
    );
    let rejected = router
        .oneshot(request(
            "/api/global/coordinator/reads/history",
            &json!({"reference": ordinary.id}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(state.db.pool())
        .await
        .unwrap();
    assert!(violations.is_empty());
}

#[tokio::test]
async fn exact_read_history_cursor_retains_its_original_membership() {
    let state = make_test_state().await;
    for (index, reference) in [".", ".."].iter().enumerate() {
        state
            .db
            .create_conversation(
                reference,
                &format!("cursor-{index}"),
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        for ordinal in 0..2 {
            state
                .db
                .add_message(
                    &format!("cursor-{index}-{ordinal}"),
                    reference,
                    &MessageContent::user("evidence"),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
    }
    let router = create_router(state);
    let first = read(&router, "/api/product-conversation-reads/snapshot?message_limit=1&boundary_representation=original_messages", ".").await;
    let cursor = first["before"].as_str().unwrap();
    let uri = format!("/api/product-conversation-reads/snapshot?message_limit=1&boundary_representation=original_messages&before={cursor}");
    let older = read(&router, &uri, ".").await;
    assert_message(&older["segments"][0], "cursor-0-0");
    let foreign = router
        .oneshot(request(&uri, r#"{"reference":".."}"#))
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn exact_read_rejects_malformed_duplicate_and_missing_references() {
    let router = create_router(make_test_state().await);
    for uri in TRANSCRIPT_READS.iter().copied().chain([
        "/api/product-conversation-reads/snapshot",
        "/api/global/coordinator/reads/route",
        "/api/global/coordinator/reads/history",
    ]) {
        for body in [
            "{",
            "{}",
            r#"{"reference":null}"#,
            r#"{"reference":1}"#,
            r#"{"reference":".","reference":".."}"#,
            r#"{"reference":".","source_transcript":"foreign"}"#,
        ] {
            let response = router.clone().oneshot(request(uri, body)).await.unwrap();
            assert!(response.status().is_client_error(), "{uri} {body}");
        }
    }
}

#[tokio::test]
async fn exact_read_blank_and_missing_references_do_not_fall_back_to_latest() {
    let state = make_test_state().await;
    state
        .db
        .create_conversation(
            "latest-ordinary",
            "latest-ordinary",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let coordinator = state
        .db
        .get_or_create_coordinator(None, crate::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let router = create_router(state);
    assert_eq!(
        read(
            &router,
            "/api/global/coordinator/reads/route",
            &coordinator.id
        )
        .await["coordinator_id"],
        coordinator.id
    );
    for reference in ["", " ", "\t\n", "missing-reference"] {
        assert_eq!(
            read(&router, "/api/global/coordinator/reads/route", reference).await,
            json!({"coordinator_id": null})
        );
        for uri in TRANSCRIPT_READS.iter().copied().chain([
            "/api/product-conversation-reads/snapshot",
            "/api/global/coordinator/reads/history",
        ]) {
            let response = router
                .clone()
                .oneshot(request(uri, &json!({"reference": reference}).to_string()))
                .await
                .unwrap();
            assert!(response.status().is_client_error(), "{uri} {reference:?}");
        }
    }
}

#[tokio::test]
async fn exact_read_aliases_remain_owner_authenticated() {
    let mut state = make_test_state().await;
    state
        .db
        .create_conversation(".", "owner-transcript", "/tmp", true, None, None)
        .await
        .unwrap();
    let coordinator = state
        .db
        .get_or_create_coordinator(None, crate::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    for reference in [".", coordinator.id.as_str()] {
        state
            .db
            .add_message(
                &format!("owner-{reference}"),
                reference,
                &MessageContent::user("Owner evidence"),
                None,
                None,
            )
            .await
            .unwrap();
    }
    let baseline = create_router(state.clone());
    state.password = Some(
        phoenix_core::domain::instance_identity::OwnerPassword::parse("secret".to_owned()).unwrap(),
    );
    let router = create_router(state);
    for uri in TRANSCRIPT_READS.iter().copied().chain([
        "/api/product-conversation-reads/snapshot",
        "/api/global/coordinator/reads/route",
        "/api/global/coordinator/reads/history",
    ]) {
        let reference = if uri.starts_with("/api/global/") {
            coordinator.id.as_str()
        } else {
            "."
        };
        let body = json!({"reference": reference}).to_string();
        let response = router.clone().oneshot(request(uri, &body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");

        let mut authenticated = request(uri, &body);
        authenticated.headers_mut().insert(
            axum::http::header::AUTHORIZATION,
            "Bearer secret".parse().unwrap(),
        );
        let response = router.clone().oneshot(authenticated).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(value, read(&baseline, uri, reference).await, "{uri}");
    }
}
