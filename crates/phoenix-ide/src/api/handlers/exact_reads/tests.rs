use crate::api::handlers::{create_router, hard_delete_cascade_tests::make_test_state};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::IntoResponse,
    Router,
};
use futures::StreamExt;
use phoenix_core::domain::db_schema::MessageContent;
use serde_json::{json, Value};
use std::time::Duration;
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
const ROUTE_READS: &[&str] = &[
    "/api/conversation-reads/route",
    "/api/conversation-reads/route/by-slug",
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
async fn exact_route_reads_keep_id_and_slug_domains_separate() {
    let state = make_test_state().await;
    for (index, reference) in REFERENCES.iter().enumerate() {
        let by_id = state
            .db
            .create_conversation(
                reference,
                &format!("id-owner-{index}"),
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let slug_owner = format!("slug-owner-{index}");
        state
            .db
            .create_conversation(&slug_owner, &slug_owner, "/tmp", true, None, None)
            .await
            .unwrap();
        // Persisted historical slugs need not match the current name allocator's projection.
        sqlx::query("UPDATE conversations SET slug = ?1 WHERE id = ?2")
            .bind(reference)
            .bind(&slug_owner)
            .execute(state.db.pool())
            .await
            .unwrap();
        let router = create_router(state.clone());
        assert_eq!(
            read(&router, ROUTE_READS[0], reference).await,
            json!({"id": reference, "slug": by_id.slug})
        );
        assert_eq!(
            read(&router, ROUTE_READS[1], reference).await,
            json!({"id": slug_owner, "slug": reference})
        );
    }
    let router = create_router(state);
    for uri in [
        "/api/conversations/slug-owner-0/route",
        "/api/conversations/by-slug/id-owner-0/route",
    ] {
        let response = router
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "unchanged route: {uri}");
    }
}

fn stream_request(reference: &str, options: &[(&str, &str)]) -> Request<Body> {
    let mut url = reqwest::Url::parse("http://phoenix.test/api/conversation-reads/stream").unwrap();
    url.query_pairs_mut()
        .append_pair("reference", reference)
        .extend_pairs(options.iter().copied());
    Request::get(format!("{}?{}", url.path(), url.query().unwrap()))
        .body(Body::empty())
        .unwrap()
}

async fn stream_init(router: &Router, request: Request<Body>) -> Value {
    let response = tokio::time::timeout(Duration::from_secs(5), router.clone().oneshot(request))
        .await
        .expect("stream response timeout")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .expect("init timeout")
        .expect("init chunk")
        .unwrap();
    let chunk = std::str::from_utf8(&chunk).unwrap();
    assert!(chunk.lines().any(|line| line == "event: init"));
    serde_json::from_str(
        chunk
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn exact_stream_preserves_opaque_identity_and_existing_init() {
    let state = make_test_state().await;
    for (index, reference) in REFERENCES.iter().enumerate() {
        state
            .db
            .create_conversation(
                reference,
                &format!("stream-{index}"),
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let message_id = format!("stream-message-{index}");
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
        let init = stream_init(
            &create_router(state.clone()),
            stream_request(reference, &[]),
        )
        .await;
        assert_eq!(init["conversation"]["id"], *reference);
        assert_message(&init, &message_id);
    }
}

#[tokio::test]
async fn exact_stream_retains_cursor_generation_and_last_event_id_semantics() {
    use crate::runtime::SseEvent;
    let state = make_test_state().await;
    let id = "a/b?雪#%";
    state
        .db
        .create_conversation(id, "cursor-stream", "/tmp", true, None, None)
        .await
        .unwrap();
    let persisted = state
        .db
        .add_message(
            "cursor-anchor",
            id,
            &MessageContent::user("anchor"),
            None,
            None,
        )
        .await
        .unwrap();
    let handle = state.runtime.get_or_create(id).await.unwrap();
    let broadcast = &handle.broadcast_tx;
    let _subscription = broadcast.subscribe();
    broadcast.send_persisted_message(persisted.clone()).unwrap();
    for text in ["first", "second"] {
        broadcast
            .send_seq(|sequence_id| SseEvent::Token {
                sequence_id,
                text: text.into(),
                request_id: "request".into(),
            })
            .unwrap();
    }
    let generation = state
        .db
        .get_conversation(id)
        .await
        .unwrap()
        .transcript_generation
        .to_string();
    let cursor = (persisted.sequence_id + 1).to_string();
    let router = create_router(state);
    for path in [false, true] {
        let query = [
            ("after_event_sequence", cursor.as_str()),
            ("after_sequence", "999999"),
            ("transcript_generation", generation.as_str()),
        ];
        let mut request = if path {
            // Safe path alias uses the same row without changing stream behavior.
            let encoded = super::super::percent_encode_query_value(id);
            Request::get(format!("/api/conversations/{encoded}/stream?after_event_sequence={cursor}&after_sequence=999999&transcript_generation={generation}")).body(Body::empty()).unwrap()
        } else {
            stream_request(id, &query)
        };
        request
            .headers_mut()
            .insert("last-event-id", "999999".parse().unwrap());
        let init = stream_init(&router, request).await;
        assert_eq!(init["conversation"]["id"], id);
        assert_eq!(init["transcript_coverage"], "preserve");
        assert!(init["messages"].as_array().unwrap().is_empty());
        assert_eq!(init["pending_events"].as_array().unwrap().len(), 1);
        assert_eq!(init["pending_events"][0]["text"], "second");
    }
    let mut legacy_only = stream_request(id, &[("after_sequence", "999999")]);
    legacy_only
        .headers_mut()
        .insert("last-event-id", "999999".parse().unwrap());
    let init = stream_init(&router, legacy_only).await;
    assert_message(&init, "cursor-anchor");
    assert_eq!(init["pending_events"].as_array().unwrap().len(), 2);
    let stale_generation = stream_init(
        &router,
        stream_request(
            id,
            &[
                ("after_event_sequence", cursor.as_str()),
                ("transcript_generation", "999999"),
            ],
        ),
    )
    .await;
    assert_message(&stale_generation, "cursor-anchor");
}

#[tokio::test]
async fn exact_stream_rejects_malformed_and_duplicate_selectors_before_subscribing() {
    let state = make_test_state().await;
    for id in [".", "..", "雪", "�", "%ZZ"] {
        state
            .db
            .create_conversation(id, &format!("invalid-query-{id}"), "/tmp", true, None, None)
            .await
            .unwrap();
    }
    let router = create_router(state);
    for query in [
        "", "reference=", "reference=missing", "reference=.&reference=..", "reference=.&reference=.",
        "reference=.&%72eference=..", "reference=%FF", "reference=%E9%9B", "reference=%ZZ", "reference=%", "reference=%+1",
        "reference=.&after_event_sequence=no", "reference=.&after_event_sequence=1&after_event_sequence=2",
        "reference=.&after_sequence=no", "reference=.&transcript_generation=no", "reference=.&transcript_generation=1&transcript_generation=2",
        "reference=.&open_id=no", "reference=.&open_id=00000000-0000-0000-0000-000000000000&open_id=00000000-0000-0000-0000-000000000001",
    ] {
        let request = Request::get(format!("/api/conversation-reads/stream?{query}")).body(Body::empty()).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), router.clone().oneshot(request)).await.expect("invalid query response timeout").unwrap();
        assert!(response.status().is_client_error(), "{query}: {}", response.status());
    }
}

#[tokio::test]
async fn exact_route_and_stream_aliases_require_owner_authentication() {
    let mut state = make_test_state().await;
    let conversation = state
        .db
        .create_conversation(".", "readable", "/tmp", true, None, None)
        .await
        .unwrap();
    state.password = Some(
        phoenix_core::domain::instance_identity::OwnerPassword::parse("secret".into()).unwrap(),
    );
    state
        .db
        .insert_auth_session("read-session", "", chrono::Duration::minutes(5))
        .await
        .unwrap();
    let router = create_router(state);
    for (uri, reference) in [
        (ROUTE_READS[0], "."),
        (ROUTE_READS[1], conversation.slug.as_str()),
    ] {
        let body = json!({"reference": reference}).to_string();
        let response = router.clone().oneshot(request(uri, &body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let mut authenticated = request(uri, &body);
        authenticated
            .headers_mut()
            .insert("cookie", "phoenix-auth=read-session".parse().unwrap());
        let response = router.clone().oneshot(authenticated).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for options in [&[][..], &[("password", "secret")][..]] {
        let response = router
            .clone()
            .oneshot(stream_request(".", options))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let mut authenticated = stream_request(".", &[]);
    authenticated
        .headers_mut()
        .insert("cookie", "phoenix-auth=read-session".parse().unwrap());
    assert_eq!(
        stream_init(&router, authenticated).await["conversation"]["id"],
        "."
    );
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
    for (index, reference) in REFERENCES.iter().enumerate() {
        let product_id =
            phoenix_core::domain::product_conversation::ProductConversationId::parse(*reference)
                .unwrap();
        assert_eq!(product_id.as_str(), *reference);
        let transcript_id = format!("product-transcript-{index}");
        let conversation = state
            .db
            .create_conversation(&transcript_id, &transcript_id, "/tmp", true, None, None)
            .await
            .unwrap();
        let scope_id = conversation.attached_work_scope_id.as_ref().unwrap();
        let mut tx = state.db.pool().begin().await.unwrap();
        // Membership updates are forbidden; stage the whole empty constructor row for reinsertion.
        sqlx::query("CREATE TEMP TABLE exact_read_product_fixture AS SELECT * FROM conversations WHERE id = ?1")
            .bind(&transcript_id).execute(&mut *tx).await.unwrap();
        sqlx::query("DELETE FROM conversations WHERE id = ?1")
            .bind(&transcript_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DELETE FROM product_conversation_work_scopes WHERE work_scope_id = ?1")
            .bind(scope_id.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE product_conversations SET id = ?1 WHERE id = ?2")
            .bind(product_id.as_str())
            .bind(conversation.product_conversation_id.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE exact_read_product_fixture SET product_conversation_id = ?1")
            .bind(product_id.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO conversations SELECT * FROM exact_read_product_fixture")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("DROP TABLE exact_read_product_fixture")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let stored = state.db.get_conversation(&transcript_id).await.unwrap();
        assert_eq!(stored.product_conversation_id, product_id);
        assert_eq!(
            stored.attached_work_scope_id,
            conversation.attached_work_scope_id
        );
        assert_eq!(stored.cwd, conversation.cwd);
        let owner: String = sqlx::query_scalar(
            "SELECT product_conversation_id FROM product_conversation_work_scopes WHERE work_scope_id = ?1",
        )
        .bind(scope_id.as_str())
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(owner, *reference);
    }
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(state.db.pool())
        .await
        .unwrap();
    assert!(violations.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(state.db.pool())
            .await
            .unwrap(),
        1
    );
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
async fn exact_read_coordinator_uuid_routes_and_history_match_legacy() {
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
}

#[tokio::test]
async fn exact_read_coordinator_schema_rekeys_keep_membership() {
    let state = make_test_state().await;
    let coordinator = state
        .db
        .get_or_create_coordinator(None, crate::llm_language::LlmLanguage::default())
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
    for uri in TRANSCRIPT_READS.iter().chain(ROUTE_READS).copied().chain([
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
        for uri in TRANSCRIPT_READS.iter().chain(ROUTE_READS).copied().chain([
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
async fn transcript_direct_get_serves_public_shell_without_exempting_exact_reads() {
    // The handler also returns an HTML diagnostic when no built UI is available.
    let shell = super::super::serve_spa().await.into_response();
    let shell_status = shell.status();
    let shell_content_type = shell.headers()[axum::http::header::CONTENT_TYPE].clone();
    assert_eq!(shell_content_type, "text/html; charset=utf-8");
    let shell_body = to_bytes(shell.into_body(), 1024 * 1024).await.unwrap();
    assert!(!shell_body.is_empty());

    let mut state = make_test_state().await;
    for password_required in [false, true] {
        state.password = password_required.then(|| {
            phoenix_core::domain::instance_identity::OwnerPassword::parse("secret".to_owned())
                .unwrap()
        });
        let router = create_router(state.clone());
        for uri in [
            "/transcript?source_transcript=.",
            "/transcript?source_transcript=a%2Fb%3Fc%23d",
        ] {
            let response = router
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                shell_status,
                "{uri}, password={password_required}"
            );
            assert_eq!(
                response.headers()[axum::http::header::CONTENT_TYPE],
                shell_content_type,
                "{uri}, password={password_required}"
            );
            assert_eq!(
                to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                shell_body,
                "{uri}, password={password_required}"
            );
        }
        if password_required {
            for uri in TRANSCRIPT_READS.iter().chain(ROUTE_READS).copied().chain([
                "/api/product-conversation-reads/snapshot",
                "/api/global/coordinator/reads/route",
                "/api/global/coordinator/reads/history",
            ]) {
                let response = router
                    .clone()
                    .oneshot(request(uri, r#"{"reference":"."}"#))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
            }
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
