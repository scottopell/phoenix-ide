use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use phoenix_core::domain::instance_identity::{OwnerPassword, PeerBaseUrl, PeerBearerCredential};
use tower::ServiceExt;

#[tokio::test]
async fn owner_router_preserves_safe_peer_array_string_notes_and_public_reference() {
    let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    state.password = Some(OwnerPassword::parse("owner-test-password".into()).unwrap());
    let instance = state.db.instance_id().await.unwrap();
    let peer_id = InstanceId::new();
    let secret = PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
    state
        .db
        .save_federation_peer_connection(
            peer_id,
            "alias",
            &"https://peer.example:8031".parse::<PeerBaseUrl>().unwrap(),
            &secret,
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
    let app = crate::api::handlers::create_router(state.clone());
    for path in [
        "/api/federation/peers".to_string(),
        format!("/api/federation/peers/{peer_id}/notes"),
    ] {
        for credential in [None, Some(secret.expose())] {
            let mut request = Request::builder().uri(&path);
            if let Some(credential) = credential {
                request = request.header("authorization", format!("Bearer {credential}"));
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/federation/peers")
                .header("authorization", "Bearer owner-test-password")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), MAX_OPENAPI_BYTES)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(secret.expose()));
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0].as_object().unwrap().len(), 4);
    assert_eq!(value[0]["peer_instance_id"], peer_id.to_string());
    assert_eq!(value[0]["peer_display_name"], "alias");
    assert!(value[0]["base_url"]
        .as_str()
        .unwrap()
        .starts_with("https://peer.example:8031"));
    assert!(!value[0]["created_at"].as_str().unwrap().is_empty());
    assert_owner_notes_and_public_reference(app, &state, peer_id, instance).await;
}

async fn assert_owner_notes_and_public_reference(
    app: axum::Router,
    state: &AppState,
    peer_id: InstanceId,
    instance: InstanceId,
) {
    let path = format!("/api/federation/peers/{peer_id}/notes");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("authorization", "Bearer owner-test-password")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let initial: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap()).unwrap();
    assert_eq!(initial, json!({"notes": ""}));
    for notes in [String::new(), "雪".repeat(10922) + "ab"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(&path)
                    .header("authorization", "Bearer owner-test-password")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"notes": notes}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(response, json!({"notes": notes}));
        assert_eq!(
            state.db.federation_peer_notes(peer_id).await.unwrap(),
            Some(notes)
        );
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&path)
                .header("authorization", "Bearer owner-test-password")
                .header("content-type", "application/json")
                .body(Body::from(json!({"notes": "雪".repeat(10923)}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), MAX_OPENAPI_BYTES)
        .await
        .unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    validate_openapi(&document, instance).unwrap();
    assert!(document["components"]["securitySchemes"]["OwnerAuthenticated"].is_object());
    assert!(document["components"]["securitySchemes"]["PeerBearer"].is_object());
    assert!(validate_openapi(&document, peer_id).is_err());
    let mut bound = vec![0; MAX_OPENAPI_BYTES - 1];
    append_bounded_response_chunk(&mut bound, b"x", MAX_OPENAPI_BYTES).unwrap();
    assert!(matches!(
        append_bounded_response_chunk(&mut bound, b"x", MAX_OPENAPI_BYTES),
        Err(FederationClientError::ResponseTooLarge(65536))
    ));
}
