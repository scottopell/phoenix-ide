use super::*;
use axum::{body::Body, http::Request};
use tower::ServiceExt;

async fn observation() -> (ApplicationState, String, String) {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let target = db
        .create_conversation("target", "target-slug", "/fixture", true, None, None)
        .await
        .unwrap();
    let other = db
        .create_conversation("other", "other-slug", "/fixture", true, None, None)
        .await
        .unwrap();
    let application = ObservationApplication::for_target(db, &target.id)
        .await
        .unwrap();
    (
        ApplicationState::Observation(application),
        target.id,
        other.id,
    )
}

fn request(path: &str, peer: Option<&str>, forwarding: Option<&str>) -> Request<Body> {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    if let Some(peer) = peer {
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    }
    if let Some(header) = forwarding {
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(header.as_bytes()).unwrap(),
            "127.0.0.1".parse().unwrap(),
        );
    }
    request
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn live_capability_bootstrap_requires_no_owner_cookie_but_grants_no_owner_access() {
    let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let password = "bootstrap-owner-password";
    state.password = Some(
        phoenix_core::domain::instance_identity::OwnerPassword::parse(password.to_owned()).unwrap(),
    );
    state.sessions = crate::api::auth::SessionStore::new(state.db.clone(), password.to_owned());
    let conversation = state
        .db
        .create_conversation("shared", "shared-slug", "/fixture", true, None, None)
        .await
        .unwrap();
    let token = state.db.create_share_token(&conversation.id).await.unwrap();
    let application = ApplicationState::Live(state);
    let app = crate::api::create_router(application.clone());

    assert_eq!(
        read_json(&application, "/api/capabilities").await,
        serde_json::json!({
            "persisted_member_read": "available",
            "live_stream": "available",
            "conversation_activation": "available",
            "user_input": "available",
            "telemetry": "available",
            "provider_catalog": "available",
            "host_resource_observations": "available"
        })
    );
    assert_eq!(
        read_json(&application, "/api/auth/status").await,
        serde_json::json!({ "auth_required": true, "authenticated": false })
    );
    for path in [
        "/api/models",
        "/api/observation/target",
        "/api/capabilities/extra",
    ] {
        let response = app
            .clone()
            .oneshot(request(path, None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    for method in ["POST", "PUT", "DELETE", "HEAD", "OPTIONS"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method}");
    }
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/share/{token}/conversation"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/status")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        status,
        serde_json::json!({ "auth_required": true, "authenticated": true })
    );
}

#[tokio::test]
async fn observation_requires_direct_loopback_even_for_public_routes() {
    let (application, _, _) = observation().await;
    let app = crate::api::create_router(application);
    for (peer, forwarding) in [
        (None, None),
        (Some("192.0.2.1:1234"), None),
        (Some("127.0.0.1:1234"), Some("x-forwarded-for")),
        (Some("[::1]:1234"), Some("forwarded")),
    ] {
        let response = app
            .clone()
            .oneshot(request("/api/capabilities", peer, forwarding))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for peer in ["127.0.0.1:1234", "[::1]:1234"] {
        let response = app
            .clone()
            .oneshot(request("/api/capabilities", Some(peer), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn observation_cannot_extract_live_authority() {
    let (application, _, _) = observation().await;
    let (mut parts, _) = request("/", Some("127.0.0.1:1234"), None).into_parts();
    assert!(LiveState::from_request_parts(&mut parts, &application)
        .await
        .is_err());
    let capabilities = ApplicationCapabilities::from_request_parts(&mut parts, &application)
        .await
        .unwrap();
    assert_eq!(
        capabilities.persisted_member_read,
        CapabilityAvailability::Available
    );
    assert_eq!(
        capabilities.live_stream,
        CapabilityAvailability::Unavailable
    );
    assert_eq!(
        capabilities.conversation_activation,
        CapabilityAvailability::Unavailable
    );
    assert_eq!(capabilities.user_input, CapabilityAvailability::Unavailable);
    assert_eq!(capabilities.telemetry, CapabilityAvailability::Unavailable);
}

#[tokio::test]
async fn shared_route_lookup_is_target_scoped() {
    let (application, target, other) = observation().await;
    let app = crate::api::create_router(application);
    for (id, expected) in [(target, StatusCode::OK), (other, StatusCode::NOT_FOUND)] {
        let response = app
            .clone()
            .oneshot(request(
                &format!("/api/conversations/{id}/route"),
                Some("127.0.0.1:1234"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn normal_routes_reject_live_operations_in_observation_composition() {
    let (application, target, _) = observation().await;
    let app = crate::api::create_router(application);
    for path in [
        format!("/api/conversations/{target}/stream"),
        "/api/models".to_owned(),
        "/api/deployment".to_owned(),
    ] {
        let response = app
            .clone()
            .oneshot(request(&path, Some("127.0.0.1:1234"), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
    }
}

async fn read_json(application: &ApplicationState, path: &str) -> serde_json::Value {
    let response = crate::api::create_router(application.clone())
        .oneshot(request(path, Some("127.0.0.1:1234"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn observation_target_discovery_preserves_ordinary_and_global_projection_contracts() {
    for global in [false, true] {
        let db = crate::db::Database::open_in_memory().await.unwrap();
        let ordinary = db
            .create_conversation(
                "persisted ordinary",
                "ordinary",
                "/fixture",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let selected = if global { &coordinator } else { &ordinary };
        let application = ApplicationState::Observation(
            ObservationApplication::for_target(db.clone(), &selected.id)
                .await
                .unwrap(),
        );
        let capabilities = read_json(&application, "/api/capabilities").await;
        assert_eq!(capabilities["persisted_member_read"], "available");
        for capability in [
            "live_stream",
            "conversation_activation",
            "user_input",
            "telemetry",
            "provider_catalog",
            "host_resource_observations",
        ] {
            assert_eq!(capabilities[capability], "unavailable");
        }
        let discovered = read_json(&application, "/api/observation/target").await;
        let metadata = &discovered["conversation"];
        assert_eq!(
            metadata["product_conversation_id"],
            selected.product_conversation_id.as_str()
        );
        let list = read_json(&application, "/api/product-conversations").await;
        if global {
            assert_eq!(discovered["kind"], "coordinator");
            assert_eq!(list["product_conversations"], serde_json::json!([]));
            assert!(metadata.get("lifecycle").is_none());
            assert!(metadata.get("presentation").is_none());
            let path = format!("/api/global/coordinator/history/{}", selected.id);
            let detail = read_json(&application, &path).await;
            let projected = super::super::product_conversations::read_coordinator_history(
                &db,
                selected.id.clone(),
                super::super::product_conversations::CoordinatorHistoryQuery {
                    before: None,
                    message_limit: None,
                    open_id: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(detail, serde_json::to_value(projected.0).unwrap());
            for field in [
                "product_conversation_id",
                "canonical_route",
                "canonical_root",
                "latest_transcript_row_id",
                "updated_at",
            ] {
                assert_eq!(metadata[field], detail[field], "{field}");
            }
            assert_eq!(
                metadata["canonical_root"]["title"],
                serde_json::json!(selected.title)
            );
        } else {
            assert_eq!(discovered["kind"], "ordinary");
            assert_eq!(list["product_conversations"].as_array().unwrap().len(), 1);
            assert_eq!(metadata, &list["product_conversations"][0]);
            let projected =
                super::super::product_conversations::read_scoped_product_conversation_list(
                    &db,
                    Some(&selected.product_conversation_id),
                )
                .await
                .unwrap();
            assert_eq!(list, serde_json::to_value(projected.0).unwrap());
            let path = format!(
                "/api/product-conversations/{}",
                selected.product_conversation_id
            );
            let detail = read_json(&application, &path).await;
            assert_eq!(
                detail["product_conversation_id"],
                metadata["product_conversation_id"]
            );
            assert_eq!(detail["canonical_route"], metadata["canonical_route"]);
        }
        let foreign_path = if global {
            format!(
                "/api/product-conversations/{}",
                ordinary.product_conversation_id
            )
        } else {
            format!("/api/global/coordinator/history/{}", coordinator.id)
        };
        let response = crate::api::create_router(application.clone())
            .oneshot(request(&foreign_path, Some("127.0.0.1:1234"), None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = crate::api::create_router(application)
            .oneshot(request(
                "/api/observation/target",
                Some("192.0.2.1:1234"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
