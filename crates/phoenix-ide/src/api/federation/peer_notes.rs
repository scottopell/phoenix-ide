use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use phoenix_core::domain::instance_identity::InstanceSelector;
use phoenix_db::{
    FederationPeerConnection, FederationPeerNotes, FederationPeerSelectionError,
    FederationPeerSummary,
};
use serde::{Deserialize, Serialize};

use super::super::{auth::OwnerAuthenticated, AppState};

const REQUEST_BYTES: usize = 256 * 1024;

type ApiResult<T> = Result<T, PeerNotesError>;

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/federation/peers", get(list_peers))
        .route(
            "/api/federation/peers/:instance/notes",
            get(get_notes).put(put_notes),
        )
        .layer(DefaultBodyLimit::max(REQUEST_BYTES))
}

pub struct PeerNotesError(StatusCode, &'static str);

#[derive(Serialize)]
struct PeerNotesErrorBody {
    code: &'static str,
}

impl IntoResponse for PeerNotesError {
    fn into_response(self) -> Response {
        (self.0, Json(PeerNotesErrorBody { code: self.1 })).into_response()
    }
}

fn database_error(error: &phoenix_db::DbError) -> PeerNotesError {
    tracing::error!(%error, "federation peer-notes database operation failed");
    PeerNotesError(StatusCode::INTERNAL_SERVER_ERROR, "database_unavailable")
}

async fn peer(state: &AppState, instance: String) -> ApiResult<FederationPeerConnection> {
    let selector = InstanceSelector::parse(instance)
        .map_err(|_| PeerNotesError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_instance"))?;
    state
        .db
        .resolve_federation_peer(&selector)
        .await
        .map_err(|error| match error {
            FederationPeerSelectionError::NotFound => {
                PeerNotesError(StatusCode::NOT_FOUND, "peer_not_found")
            }
            FederationPeerSelectionError::Ambiguous => {
                PeerNotesError(StatusCode::CONFLICT, "ambiguous_instance")
            }
            FederationPeerSelectionError::Database(error) => database_error(&error),
        })
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerNotesDocument {
    #[schemars(
        description = "Opaque text, at most 32768 UTF-8 bytes; empty clears notes and text grants no authority"
    )]
    pub notes: FederationPeerNotes,
}

async fn list_peers(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<FederationPeerSummary>>> {
    state
        .db
        .list_federation_peers()
        .await
        .map(Json)
        .map_err(|error| database_error(&error))
}

async fn get_notes(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    Path(instance): Path<String>,
) -> ApiResult<Json<PeerNotesDocument>> {
    let peer = peer(&state, instance).await?;
    let notes = state
        .db
        .federation_peer_notes(peer.peer_instance_id)
        .await
        .map_err(|error| database_error(&error))?
        .ok_or(PeerNotesError(StatusCode::NOT_FOUND, "peer_not_found"))?;
    Ok(Json(PeerNotesDocument { notes }))
}

async fn put_notes(
    _: OwnerAuthenticated,
    State(state): State<AppState>,
    Path(instance): Path<String>,
    request: Result<Json<PeerNotesDocument>, JsonRejection>,
) -> ApiResult<Json<PeerNotesDocument>> {
    let Json(request) =
        request.map_err(|_| PeerNotesError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request"))?;
    let peer = peer(&state, instance).await?;
    if !state
        .db
        .set_federation_peer_notes(peer.peer_instance_id, &request.notes)
        .await
        .map_err(|error| database_error(&error))?
    {
        return Err(PeerNotesError(StatusCode::NOT_FOUND, "peer_not_found"));
    }
    Ok(Json(request))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use phoenix_core::domain::instance_identity::{InstanceId, PeerBearerCredential, PeerTlsTrust};
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
        value: &serde_json::Value,
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

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn notes_api_rejects_authority_fields_and_preserves_whole_text() {
        let (app, state) = app().await;
        let id = InstanceId::new();
        let credential =
            PeerBearerCredential::parse(format!("phx_peer_{}", "a".repeat(43))).unwrap();
        state
            .db
            .save_federation_peer_connection(
                id,
                "peer",
                &"https://peer.example".parse().unwrap(),
                &credential,
                &PeerTlsTrust::PlatformRoots,
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
                &serde_json::Value::Null
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
                    &serde_json::json!({"notes":"x"})
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
                call(&app, "PUT", &path, Some("owner-test-password"), &value)
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
            let (status, result) =
                call(&app, "PUT", &path, Some("owner-test-password"), &value).await;
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
                &serde_json::Value::Null,
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
            &serde_json::Value::Null,
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
                "/api/federation/peers/peer/notes",
                Some("owner-test-password"),
                &serde_json::Value::Null,
            )
            .await
            .0,
            StatusCode::OK
        );
        let (status, body) = call(
            &app,
            "GET",
            "/api/federation/peers/%20/notes",
            Some("owner-test-password"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"code":"invalid_instance"})
        );
        let second = InstanceId::new();
        state
            .db
            .save_federation_peer_connection(
                second,
                "peer",
                &"https://second-peer.example".parse().unwrap(),
                &PeerBearerCredential::parse(format!("phx_peer_{}", "b".repeat(43))).unwrap(),
                &PeerTlsTrust::PlatformRoots,
            )
            .await
            .unwrap();
        let (status, body) = call(
            &app,
            "GET",
            "/api/federation/peers/peer/notes",
            Some("owner-test-password"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"code":"ambiguous_instance"})
        );
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/api/federation/peers/{}/notes", InstanceId::new()),
                Some("owner-test-password"),
                &serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}
