use super::*;
use axum::{body::Body, http::Request};
use phoenix_core::domain::instance_identity::{
    FederationCredentialVerifier, OwnerPassword, PeerBearerCredential, PeerTlsTrust,
};
use tower::ServiceExt as _;

#[test]
#[allow(clippy::too_many_lines)]
fn generated_reference_has_exact_supported_paths_and_resolvable_derived_schemas() {
    fn resolve_refs(value: &Value, document: &Value) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref") {
                    let path = reference.as_str().unwrap().strip_prefix('#').unwrap();
                    assert!(document.pointer(path).is_some(), "unresolved {path}");
                }
                for value in object.values() {
                    resolve_refs(value, document);
                }
            }
            Value::Array(array) => {
                for value in array {
                    resolve_refs(value, document);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    let id = InstanceId::new();
    let reference = serde_json::to_value(generate(id)).unwrap();
    assert_eq!(reference["info"]["title"], "Federation API reference");
    assert_eq!(reference["x-phoenix-instance-id"], json!(id));
    let mut expected = vec![
        "/openapi.json".to_string(),
        "/api/federation/openapi".into(),
        "/api/federation/peers".into(),
        "/api/federation/peers/{instance}/notes".into(),
        "/api/federation/peer/send-authored-conversation-message".into(),
    ];
    for operation in [
        "query-database",
        "search-conversations",
        "read-conversation",
        "resolve-reference",
        "send-conversation-message",
    ] {
        for prefix in ["/api/federation/", "/api/federation/peer/"] {
            let path = format!("{prefix}{operation}");
            let expected_security = if prefix.contains("/peer/") {
                Access::Peer.security()
            } else {
                Access::Owner.security()
            };
            assert_eq!(
                reference["paths"][&path]["post"]["security"],
                expected_security
            );
            expected.push(path);
        }
    }
    for suffix in ["create", "replay", "authorize", "unsubscribe"] {
        expected.push(format!("/api/federation/peer/subscriptions/{suffix}"));
    }
    for suffix in [
        "",
        "/create",
        "/sync-once",
        "/unsubscribe",
        "/notifications",
    ] {
        expected.push(format!("/api/federation/subscriptions{suffix}"));
    }
    expected.sort();
    assert_eq!(
        reference["paths"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        expected
    );
    let schemas = &reference["components"]["schemas"];
    for name in [
        "OwnerQueryRequest",
        "OwnerSearchRequest",
        "OwnerReadRequest",
        "OwnerResolveRequest",
        "OwnerSendRequest",
        "FetchOpenApiRequest",
        "OwnerSubscriptionRequest",
    ] {
        assert!(
            schemas[name]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("instance")),
            "{name}"
        );
        assert_eq!(schemas[name]["additionalProperties"], false);
    }
    assert_eq!(
        schemas["OwnerReadRequest"]["properties"]["cursor"]["default"],
        0
    );
    assert!(!schemas["OwnerSendRequest"]["properties"]
        .as_object()
        .unwrap()
        .contains_key("sender"));
    assert_eq!(
        schemas["RemoteSendMessageRequest"]["additionalProperties"],
        false
    );
    assert!(!schemas["RemoteSendMessageRequest"]["properties"]
        .as_object()
        .unwrap()
        .contains_key("sender"));
    assert_eq!(
        schemas["RemoteAuthoredSendMessageRequest"]["additionalProperties"],
        false
    );
    assert!(schemas["RemoteAuthoredSendMessageRequest"]["required"]
        .as_array()
        .unwrap()
        .contains(&json!("sender")));
    let attestation = &schemas["RemoteSenderAttestation"];
    assert!(!attestation.to_string().contains("remote_api"));
    for sender in attestation["oneOf"].as_array().unwrap() {
        assert_eq!(sender["additionalProperties"], false);
        assert!(sender["required"]
            .as_array()
            .unwrap()
            .contains(&json!("source_call")));
        for field in ["instance_id", "caller_instance_id", "source_instance_id"] {
            assert!(!sender["properties"]
                .as_object()
                .unwrap()
                .contains_key(field));
        }
    }
    let authored =
        &reference["paths"]["/api/federation/peer/send-authored-conversation-message"]["post"];
    assert_eq!(authored["security"], Access::Peer.security());
    assert_eq!(
        authored["tags"],
        json!(["Server-to-server authored operations"])
    );
    let boundary = authored["description"].as_str().unwrap();
    for statement in [
        "trusts that source server",
        "not a direct user/API authorship surface",
        "same directional bearer",
    ] {
        assert!(boundary.contains(statement));
    }
    for name in [
        "RemoteSendReceipt",
        "ConversationReferenceTarget",
        "SubscriptionCursor",
        "SubscriptionDelivery",
        "SubscriptionResult",
        "PeerNotesDocument",
        "FederationPeerSummary",
        "RemoteReadErrorResponse",
        "DestinationApiReference",
    ] {
        assert!(!schemas[name].is_null(), "missing derived {name}");
    }
    for owner in ["QualifiedSearchSender", "ResolveGlobalReferenceResponse"] {
        let source_call = &schemas[owner]["properties"]["source_call"];
        assert!(source_call
            .to_string()
            .contains("#/components/schemas/SourceToolCall"));
    }
    for field in ["message_id", "tool_use_id"] {
        assert_eq!(
            schemas["SourceToolCall"]["properties"][field]["type"],
            "string"
        );
        assert!(schemas["SourceToolCall"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!(field)));
    }
    assert_eq!(schemas["SubscriptionCursor"]["minimum"], 0.0);
    assert_eq!(
        schemas["PeerNotesDocument"]["properties"]["notes"]["type"],
        "string"
    );
    let summary = schemas["FederationPeerSummary"]["properties"]
        .as_object()
        .unwrap();
    assert_eq!(summary.len(), 4);
    for secret in [
        "bearer_credential",
        "tls_trust",
        "certificate_pem",
        "verifier",
    ] {
        assert!(!summary.contains_key(secret));
    }
    resolve_refs(&reference, &reference);
    assert!(serde_json::to_vec(&reference).unwrap().len() < DOCUMENT_BYTES);
}

#[test]
fn decoder_binds_destination_preserves_extensions_and_bounds_size() {
    let id = InstanceId::new();
    let mut value = serde_json::to_value(generate(id)).unwrap();
    value["servers"] = json!([{"url":"https://destination.example"}]);
    value["info"]["x-extra"] = json!("retained");
    let bytes = serde_json::to_vec(&value).unwrap();
    let fetched = decode(&bytes, id).unwrap();
    assert_eq!(fetched.destination_instance_id, id);
    assert_eq!(serde_json::to_value(fetched.document).unwrap(), value);
    assert!(matches!(
        decode(&bytes, InstanceId::new()),
        Err(FederationClientError::DestinationMismatch)
    ));
    value
        .as_object_mut()
        .unwrap()
        .remove("x-phoenix-instance-id");
    assert!(decode(&serde_json::to_vec(&value).unwrap(), id).is_err());
    assert!(decode(b"not json", id).is_err());
    assert!(matches!(
        decode(&vec![b' '; DOCUMENT_BYTES + 1], id),
        Err(FederationClientError::ResponseTooLarge(DOCUMENT_BYTES))
    ));
    let mut bytes = vec![0; DOCUMENT_BYTES - 1];
    append_bounded_response_chunk(&mut bytes, &[0], DOCUMENT_BYTES).unwrap();
    assert!(append_bounded_response_chunk(&mut bytes, &[0], DOCUMENT_BYTES).is_err());
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    auth: Option<&str>,
    value: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        request = request.header("authorization", format!("Bearer {auth}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), DOCUMENT_BYTES)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn prefix_like_owner_password_and_owner_session_still_authorize_reference() {
    let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let password = "phx_peer_owner-password".to_string();
    state.password = Some(OwnerPassword::parse(password.clone()).unwrap());
    let app = crate::api::handlers::create_router(state);
    assert_eq!(
        call(&app, "GET", "/openapi.json", Some(&password), Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .extension(axum::extract::ConnectInfo(
                    "127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .header("content-type", "application/json")
                .body(Body::from(json!({"password":password}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .header("cookie", cookie)
                .header(
                    "authorization",
                    format!("Bearer phx_peer_{}", "z".repeat(43)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn auth_is_document_only_and_fetch_requires_explicit_saved_instance() {
    let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    state.password = Some(OwnerPassword::parse("reference-owner-test".into()).unwrap());
    let id = state.db.instance_id().await.unwrap();
    let credential = PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
    let caller = InstanceId::new();
    state
        .db
        .replace_federation_enrollment(
            caller,
            "reference caller",
            &FederationCredentialVerifier::from_bearer(credential.expose().as_bytes()),
        )
        .await
        .unwrap();
    let app = crate::api::handlers::create_router(state.clone());
    assert_eq!(
        call(&app, "GET", "/openapi.json", None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for auth in ["reference-owner-test", credential.expose()] {
        let (status, reference) = call(&app, "GET", "/openapi.json", Some(auth), Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reference["x-phoenix-instance-id"], json!(id));
        assert!(!reference.to_string().contains(credential.expose()));
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/federation/openapi",
            Some(credential.expose()),
            json!({"instance":"any"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for value in [
        json!({}),
        json!({"instance":null}),
        json!({"instance":" "}),
        json!({"instance":id,"url":"https://untrusted.example"}),
    ] {
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/federation/openapi",
                Some("reference-owner-test"),
                value
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for instance in [id.to_string(), "unknown".into()] {
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/federation/openapi",
                Some("reference-owner-test"),
                json!({"instance":instance})
            )
            .await,
            (StatusCode::NOT_FOUND, json!({"code":"peer_not_found"}))
        );
    }
    for endpoint in [
        "https://peer-one.invalid:8443",
        "https://peer-two.invalid:9443",
    ] {
        state
            .db
            .save_federation_peer_connection(
                InstanceId::new(),
                "duplicate",
                &endpoint.parse().unwrap(),
                &credential,
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/federation/openapi",
            Some("reference-owner-test"),
            json!({"instance":"duplicate"})
        )
        .await,
        (StatusCode::CONFLICT, json!({"code":"ambiguous_instance"}))
    );
    // Revocation removes the documentation-only peer admission too.
    state.db.revoke_federation_enrollment(caller).await.unwrap();
    assert_eq!(
        call(
            &app,
            "GET",
            "/openapi.json",
            Some(credential.expose()),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn unconfigured_owner_policy_does_not_become_invalid_peer_fallback() {
    let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    state.password = None;
    let app = crate::api::handlers::create_router(state);
    assert_eq!(
        call(&app, "GET", "/openapi.json", None, Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let invalid_peer = format!("phx_peer_{}", "z".repeat(43));
    assert_eq!(
        call(
            &app,
            "GET",
            "/openapi.json",
            Some(&invalid_peer),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
