use super::*;
use crate::api::federation::openapi::{
    fetch_peer, generate, DestinationApiReference, DOCUMENT_BYTES,
};
use axum::{
    body::{Body, Bytes},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

const OWNER: &str = "isolated-openapi-owner";
fn protected_router(mut state: crate::api::AppState) -> axum::Router {
    state.password =
        Some(phoenix_core::domain::instance_identity::OwnerPassword::parse(OWNER.into()).unwrap());
    create_router(state)
}
async fn owner_fetch(caller: &TestInstance, destination: InstanceId) -> reqwest::Response {
    caller
        .client
        .post(caller.endpoint("api/federation/openapi"))
        .bearer_auth(OWNER)
        .json(&json!({"instance":destination}))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn saved_peer_reference_uses_verified_tls_peer_auth_and_exact_destination() {
    let caller = TestInstance::start_with_router(true, protected_router).await;
    let destination = TestInstance::start_with_router(true, protected_router).await;
    let mut peer = caller.connect_to(&destination).await;
    assert_eq!(
        destination
            .client
            .get(destination.endpoint("openapi.json"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let fetched = owner_fetch(&caller, peer.peer_instance_id).await;
    assert_eq!(fetched.status(), StatusCode::OK);
    let fetched: DestinationApiReference = fetched.json().await.unwrap();
    assert_eq!(fetched.destination_instance_id, peer.peer_instance_id);
    assert_eq!(fetched.document.instance_id, peer.peer_instance_id);
    let direct_schema = &fetched.document.components.schemas["RemoteSendMessageRequest"];
    assert_eq!(direct_schema["additionalProperties"], false);
    assert!(!direct_schema["properties"]
        .as_object()
        .unwrap()
        .contains_key("sender"));
    let authored =
        &fetched.document.paths["/api/federation/peer/send-authored-conversation-message"]["post"];
    assert_eq!(authored["security"], json!([{"PeerBearer":[]}]));
    assert_eq!(
        authored["tags"],
        json!(["Server-to-server authored operations"])
    );
    assert!(authored["description"]
        .as_str()
        .unwrap()
        .contains("trusts that source server"));
    assert_ne!(
        fetched.document.instance_id,
        caller.state.db.instance_id().await.unwrap()
    );
    assert!(!serde_json::to_string(&fetched)
        .unwrap()
        .contains(peer.bearer_credential.expose()));
    peer.tls_trust = PeerTlsTrust::PlatformRoots;
    assert!(
        fetch_peer(&peer).await.is_err(),
        "private CA must not be implicitly trusted"
    );
    caller
        .state
        .db
        .save_federation_peer_connection(
            peer.peer_instance_id,
            "bad trust",
            &peer.base_url,
            &peer.bearer_credential,
            &PeerTlsTrust::PlatformRoots,
        )
        .await
        .unwrap();
    let response = owner_fetch(&caller, peer.peer_instance_id).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({"code":"peer_operation_failed"})
    );
    caller
        .state
        .db
        .save_federation_peer_connection(
            peer.peer_instance_id,
            "restored trust",
            &peer.base_url,
            &peer.bearer_credential,
            &destination.trust,
        )
        .await
        .unwrap();
    destination
        .state
        .db
        .revoke_federation_enrollment(caller.state.db.instance_id().await.unwrap())
        .await
        .unwrap();
    let response = owner_fetch(&caller, peer.peer_instance_id).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({"code":"peer_operation_failed"})
    );
    destination.stop().await;
    let response = owner_fetch(&caller, peer.peer_instance_id).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({"code":"peer_operation_failed"})
    );
    caller.stop().await;
}

#[tokio::test]
async fn redirect_does_not_reach_target_or_forward_credentials() {
    let caller = TestInstance::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    for status in [
        StatusCode::MOVED_PERMANENTLY,
        StatusCode::FOUND,
        StatusCode::SEE_OTHER,
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        let observed = requests.clone();
        let source =
            TestInstance::start_with_router(true, move |_| {
                Router::new()
                .route("/openapi.json", get(move || async move {
                    (status, [(header::LOCATION, "/redirect-target")])
                }))
                .route("/redirect-target", get(move || {
                    let observed = observed.clone();
                    async move {
                        observed.fetch_add(1, Ordering::SeqCst);
                        StatusCode::OK
                    }
                }))
            })
            .await;
        let peer = caller.connect_to(&source).await;
        assert!(fetch_peer(&peer).await.is_err());
        assert_eq!(requests.load(Ordering::SeqCst), 0);
        // Positive control: the same trusted origin and Location are reachable if followed.
        let PeerTlsTrust::PrivateCa { certificate_pem } = &source.trust else {
            panic!("fixture must use a private CA");
        };
        let following = reqwest::Client::builder()
            .tls_certs_only([
                reqwest::Certificate::from_pem(certificate_pem.expose().as_bytes()).unwrap(),
            ])
            .build()
            .unwrap();
        assert_eq!(
            following
                .get(source.endpoint("openapi.json"))
                .bearer_auth(peer.bearer_credential.expose())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        requests.store(0, Ordering::SeqCst);
        source.stop().await;
    }
    caller.stop().await;
}

#[tokio::test]
async fn remote_oversize_documents_fail_with_and_without_content_length() {
    let caller = TestInstance::start_with_router(true, protected_router).await;
    for chunked in [false, true] {
        let destination = TestInstance::start_with_router(true, move |_| {
            Router::new().route(
                "/openapi.json",
                get(move || async move {
                    if chunked {
                        let chunks = futures::stream::iter([
                            Ok::<_, std::convert::Infallible>(Bytes::from(vec![
                                b' ';
                                DOCUMENT_BYTES
                            ])),
                            Ok(Bytes::from_static(b" ")),
                        ]);
                        Body::from_stream(chunks).into_response()
                    } else {
                        vec![b' '; DOCUMENT_BYTES + 1].into_response()
                    }
                }),
            )
        })
        .await;
        let peer = caller.connect_to(&destination).await;
        assert!(matches!(
            fetch_peer(&peer).await,
            Err(crate::api::federation::FederationClientError::ResponseTooLarge(DOCUMENT_BYTES))
        ));
        let response = owner_fetch(&caller, peer.peer_instance_id).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            json!({"code":"peer_operation_failed"})
        );
        destination.stop().await;
    }
    caller.stop().await;
}

#[tokio::test]
async fn wrong_destination_and_credential_echo_are_not_exposed_or_replaced_locally() {
    let caller = TestInstance::start_with_router(true, protected_router).await;
    let destination = TestInstance::start_with_router(true, |_| {
        Router::new().route(
            "/openapi.json",
            get(|| async { axum::Json(generate(InstanceId::new())) }),
        )
    })
    .await;
    let peer = caller.connect_to(&destination).await;
    assert!(matches!(
        fetch_peer(&peer).await,
        Err(crate::api::federation::FederationClientError::DestinationMismatch)
    ));
    let response = owner_fetch(&caller, peer.peer_instance_id).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({"code":"peer_operation_failed"})
    );
    destination.stop().await;
    let destination = TestInstance::start_with_router(true, |state| {
        Router::new().route(
            "/openapi.json",
            get(move |headers: axum::http::HeaderMap| {
                let state = state.clone();
                async move {
                    let mut document = generate(state.db.instance_id().await.unwrap());
                    document.info.description = headers
                        .get(header::AUTHORIZATION)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .into();
                    axum::Json(document)
                }
            }),
        )
    })
    .await;
    let peer = caller.connect_to(&destination).await;
    let response = owner_fetch(&caller, peer.peer_instance_id).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({"code":"peer_operation_failed"})
    );
    destination.stop().await;
    caller.stop().await;
}
