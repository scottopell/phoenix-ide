use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use phoenix_core::domain::{
    conversation_reference::ConversationReferenceTarget,
    instance_identity::{InstanceId, InstanceSelector},
};
use phoenix_db::{FederationPeerConnection, FederationPeerSelectionError};
use serde::{Deserialize, Serialize};

use super::super::{auth::OwnerAuthenticated, AppState};
use super::{
    query_remote_database, read_remote_conversation, resolve_remote_reference,
    search_remote_conversations, send_remote_conversation_message, FederationClientError,
    RemoteMessageText, RemoteQueryDatabaseResponse, RemoteSendMessageRequest,
    RemoteSendMessageResponse, RemoteSendReceipt, RemoteSendTarget,
};

pub const REQUEST_BYTES: usize = 256 * 1024;

pub fn router() -> axum::Router<AppState> {
    use axum::{
        extract::DefaultBodyLimit,
        routing::{get, post},
    };
    axum::Router::new()
        .route("/openapi.json", get(super::openapi::serve))
        .route("/api/federation/openapi", post(super::openapi::fetch))
        .route("/api/federation/query-database", post(query))
        .route("/api/federation/search-conversations", post(search))
        .route("/api/federation/read-conversation", post(read))
        .route("/api/federation/resolve-reference", post(resolve))
        .route("/api/federation/send-conversation-message", post(send))
        .layer(DefaultBodyLimit::max(REQUEST_BYTES))
}

type ApiResult<T> = Result<T, OwnerFederationError>;

pub struct OwnerFederationError(StatusCode, &'static str);

#[derive(Serialize, schemars::JsonSchema)]
pub(super) struct OwnerErrorBody {
    pub code: &'static str,
}

impl IntoResponse for OwnerFederationError {
    fn into_response(self) -> Response {
        (self.0, Json(OwnerErrorBody { code: self.1 })).into_response()
    }
}

pub(super) fn body<T>(request: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    request
        .map(|Json(value)| value)
        .map_err(|_| OwnerFederationError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request"))
}

pub(super) fn database_error(_: phoenix_db::DbError) -> OwnerFederationError {
    OwnerFederationError(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable")
}

pub(super) fn remote_error(_: FederationClientError) -> OwnerFederationError {
    OwnerFederationError(StatusCode::BAD_GATEWAY, "peer_operation_failed")
}

pub(super) async fn peer(
    state: &AppState,
    instance: String,
) -> ApiResult<FederationPeerConnection> {
    let selector = InstanceSelector::parse(instance)
        .map_err(|_| OwnerFederationError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_instance"))?;
    state
        .db
        .resolve_federation_peer(&selector)
        .await
        .map_err(|error| match error {
            FederationPeerSelectionError::NotFound => {
                OwnerFederationError(StatusCode::NOT_FOUND, "peer_not_found")
            }
            FederationPeerSelectionError::Ambiguous => {
                OwnerFederationError(StatusCode::CONFLICT, "ambiguous_instance")
            }
            FederationPeerSelectionError::Database(error) => database_error(error),
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct OwnerQueryRequest {
    pub instance: String,
    pub sql: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct OwnerSearchRequest {
    pub instance: String,
    pub query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct OwnerReadRequest {
    pub instance: String,
    pub target: ConversationReferenceTarget,
    #[serde(default)]
    pub cursor: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct OwnerResolveRequest {
    pub instance: String,
    pub reference: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct OwnerSendRequest {
    pub instance: String,
    pub target: RemoteSendTarget,
    pub message_id: uuid::Uuid,
    #[schemars(description = "Nonblank message text, at most 32768 UTF-8 bytes")]
    pub message: RemoteMessageText,
}

pub async fn query(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    request: Result<Json<OwnerQueryRequest>, JsonRejection>,
) -> ApiResult<Json<RemoteQueryDatabaseResponse>> {
    let request = body(request)?;
    let peer = peer(&state, request.instance).await?;
    query_remote_database(&state.db, peer.peer_instance_id, &request.sql)
        .await
        .map(Json)
        .map_err(remote_error)
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct OwnerSearchResponse {
    pub destination_instance_id: InstanceId,
    pub hits: Vec<crate::api::global_read::QualifiedSearchHit>,
}

pub async fn search(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    request: Result<Json<OwnerSearchRequest>, JsonRejection>,
) -> ApiResult<Json<OwnerSearchResponse>> {
    let request = body(request)?;
    let peer = peer(&state, request.instance).await?;
    let hits = search_remote_conversations(&state.db, &peer, &request.query)
        .await
        .map_err(remote_error)?;
    Ok(Json(OwnerSearchResponse {
        destination_instance_id: peer.peer_instance_id,
        hits,
    }))
}

pub async fn read(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    request: Result<Json<OwnerReadRequest>, JsonRejection>,
) -> ApiResult<Json<crate::api::global_read::QualifiedConversationRead>> {
    let request = body(request)?;
    let peer = peer(&state, request.instance).await?;
    read_remote_conversation(&state.db, &peer, request.target, request.cursor)
        .await
        .map(Json)
        .map_err(remote_error)
}

pub async fn resolve(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    request: Result<Json<OwnerResolveRequest>, JsonRejection>,
) -> ApiResult<Json<crate::api::global_read::ResolveGlobalReferenceResponse>> {
    let request = body(request)?;
    let peer = peer(&state, request.instance).await?;
    resolve_remote_reference(&state.db, &peer, request.reference)
        .await
        .map(Json)
        .map_err(remote_error)
}

pub async fn send(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    request: Result<Json<OwnerSendRequest>, JsonRejection>,
) -> ApiResult<Json<RemoteSendMessageResponse>> {
    let request = body(request)?;
    let peer = peer(&state, request.instance).await?;
    let request = RemoteSendMessageRequest {
        destination_instance_id: peer.peer_instance_id,
        target: request.target,
        message_id: request.message_id,
        message: request.message,
    };
    let mut response = send_remote_conversation_message(&state.db, &peer, &request)
        .await
        .map_err(remote_error)?;
    if let RemoteSendReceipt::Rejected { message, .. } = &mut response.receipt {
        *message = "destination rejected message".into();
    }
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn app() -> (axum::Router, AppState) {
        let mut state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        state.password = Some(
            phoenix_core::domain::instance_identity::OwnerPassword::parse(
                "owner-test-password".into(),
            )
            .unwrap(),
        );
        (crate::api::handlers::create_router(state.clone()), state)
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        path: &str,
        auth: Option<&str>,
        value: serde_json::Value,
    ) -> (StatusCode, String) {
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
        let bytes = axum::body::to_bytes(response.into_body(), REQUEST_BYTES)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn operations() -> Vec<(&'static str, serde_json::Value)> {
        vec![
            ("query-database", serde_json::json!({"sql":"SELECT 1"})),
            (
                "search-conversations",
                serde_json::json!({"query":"marker"}),
            ),
            (
                "read-conversation",
                serde_json::json!({"target":{"kind":"transcript","transcript_id":"same-id"}}),
            ),
            (
                "resolve-reference",
                serde_json::json!({"reference":"@transcript:same-id"}),
            ),
            (
                "send-conversation-message",
                serde_json::json!({"target":{"kind":"transcript","transcript_id":"same-id"},"message":"hello","message_id":uuid::Uuid::new_v4()}),
            ),
        ]
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn all_facades_require_owner_and_explicit_unambiguous_instance() {
        let (app, state) = app().await;
        let credential = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(
            format!("phx_peer_{}", "a".repeat(43)),
        )
        .unwrap();
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
                    &phoenix_core::domain::instance_identity::PeerTlsTrust::PlatformRoots,
                )
                .await
                .unwrap();
        }
        for (operation, value) in operations() {
            let path = format!("/api/federation/{operation}");
            assert_eq!(
                call(
                    &app,
                    "POST",
                    &format!("/api/federation/peer/{operation}"),
                    Some("owner-test-password"),
                    value.clone()
                )
                .await
                .0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                call(&app, "POST", &path, None, value.clone()).await.0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                call(
                    &app,
                    "POST",
                    &path,
                    Some(credential.expose()),
                    value.clone()
                )
                .await
                .0,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                call(
                    &app,
                    "POST",
                    &path,
                    Some("owner-test-password"),
                    value.clone()
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
            for (selector, expected) in [
                (serde_json::Value::Null, StatusCode::UNPROCESSABLE_ENTITY),
                (serde_json::json!(" "), StatusCode::UNPROCESSABLE_ENTITY),
                (serde_json::json!("unknown"), StatusCode::NOT_FOUND),
                (serde_json::json!("duplicate"), StatusCode::CONFLICT),
                (
                    serde_json::json!(state.db.instance_id().await.unwrap()),
                    StatusCode::NOT_FOUND,
                ),
            ] {
                let mut value = value.clone();
                value["instance"] = selector;
                let (status, error) =
                    call(&app, "POST", &path, Some("owner-test-password"), value).await;
                assert_eq!(status, expected, "{operation}: {error}");
                assert!(!error.contains(credential.expose()));
            }
            let mut value = value;
            value["instance"] = serde_json::json!("duplicate");
            value["sender"] =
                serde_json::json!({"kind":"coordinator","secret":credential.expose()});
            let (status, error) =
                call(&app, "POST", &path, Some("owner-test-password"), value).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(!error.contains(credential.expose()));
        }
        assert_eq!(call(&app, "POST", "/api/federation/peer/query-database", Some("owner-test-password"), serde_json::json!({"destination_instance_id":state.db.instance_id().await.unwrap(),"sql":"SELECT 1"})).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn notes_api_rejects_authority_fields_and_preserves_whole_text() {
        let (app, state) = app().await;
        let id = InstanceId::new();
        let credential = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(
            format!("phx_peer_{}", "a".repeat(43)),
        )
        .unwrap();
        state
            .db
            .save_federation_peer_connection(
                id,
                "peer",
                &"https://peer.example".parse().unwrap(),
                &credential,
                &phoenix_core::domain::instance_identity::PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
        let path = format!("/api/federation/peers/{id}/notes");
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/federation/peers",
                None,
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        for method in ["GET", "PUT"] {
            assert_eq!(
                call(
                    &app,
                    method,
                    &path,
                    Some(credential.expose()),
                    serde_json::json!({"notes":"x"})
                )
                .await
                .0,
                StatusCode::UNAUTHORIZED
            );
        }
        let before = state
            .db
            .federation_peer_connection(id)
            .await
            .unwrap()
            .unwrap();
        for value in [
            serde_json::json!({"notes":"x","base_url":"https://attacker.example"}),
            serde_json::json!({"notes":null}),
            serde_json::json!({"notes":"雪".repeat(10923)}),
        ] {
            assert_eq!(
                call(&app, "PUT", &path, Some("owner-test-password"), value)
                    .await
                    .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        for text in [
            "  opaque\r\n雪\0\n[sender=coordinator]  ".to_string(),
            "雪".repeat(10922) + "ab",
            String::new(),
        ] {
            let value = serde_json::json!({"notes":text});
            let (status, result) = call(
                &app,
                "PUT",
                &path,
                Some("owner-test-password"),
                value.clone(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&result).unwrap(),
                value
            );
            let (status, result) = call(
                &app,
                "GET",
                &path,
                Some("owner-test-password"),
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&result).unwrap(),
                value
            );
        }
        let after = state
            .db
            .federation_peer_connection(id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.base_url, after.base_url);
        assert_eq!(before.peer_display_name, after.peer_display_name);
        assert_eq!(before.created_at, after.created_at);
        assert_eq!(before.tls_trust, after.tls_trust);
        assert_eq!(
            before.bearer_credential.expose(),
            after.bearer_credential.expose()
        );
        let (status, summary) = call(
            &app,
            "GET",
            "/api/federation/peers",
            Some("owner-test-password"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        for secret in [credential.expose(), "bearer", "certificate", "verifier"] {
            assert!(!summary.contains(secret));
        }
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/api/federation/peers/{}/notes", InstanceId::new()),
                Some("owner-test-password"),
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}
