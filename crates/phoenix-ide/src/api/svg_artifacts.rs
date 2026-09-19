use super::handlers::AppError;
use super::AppState;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use phoenix_db::{Database, SvgArtifact};

#[derive(Clone, Copy)]
enum Representation {
    Image,
    Source,
    Download,
}

async fn owned_artifact(
    db: &Database,
    conversation_id: &str,
    artifact_id: &str,
) -> Result<SvgArtifact, AppError> {
    db.svg_artifact(conversation_id, artifact_id)
        .await
        .map_err(|error| {
            tracing::error!(%error, "failed to read SVG artifact");
            AppError::Internal("Unable to read visualization".to_owned())
        })?
        .ok_or_else(|| AppError::NotFound("Visualization is unavailable".to_owned()))
}

fn artifact_response(artifact: SvgArtifact, representation: Representation) -> Response {
    let mut response = artifact.bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(match representation {
            Representation::Source => "text/plain; charset=utf-8",
            Representation::Image | Representation::Download => "image/svg+xml",
        }),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "sandbox; default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(match representation {
            Representation::Source => "inline",
            Representation::Image | Representation::Download => {
                "attachment; filename=visualization.svg"
            }
        }),
    );
    response
}

pub(super) async fn image(
    State(state): State<AppState>,
    Path((conversation_id, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    Ok(artifact_response(
        owned_artifact(&state.db, &conversation_id, &artifact_id).await?,
        Representation::Image,
    ))
}

pub(super) async fn source(
    State(state): State<AppState>,
    Path((conversation_id, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    Ok(artifact_response(
        owned_artifact(&state.db, &conversation_id, &artifact_id).await?,
        Representation::Source,
    ))
}

pub(super) async fn download(
    State(state): State<AppState>,
    Path((conversation_id, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    Ok(artifact_response(
        owned_artifact(&state.db, &conversation_id, &artifact_id).await?,
        Representation::Download,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn authenticated_state() -> AppState {
        let db = Database::open_in_memory().await.unwrap();
        let llm_registry = Arc::new(phoenix_llm::ModelRegistry::new_empty());
        let platform = crate::platform::PlatformCapability::None {
            details: "test".into(),
        };
        let mcp_manager = Arc::new(crate::tools::mcp::McpClientManager::new());
        let runtime = Arc::new(crate::runtime::RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform.clone(),
            mcp_manager.clone(),
            None,
        ));
        let terminals = runtime.terminals.clone();
        let message_retriever: Arc<dyn crate::db::MessageRetriever> = Arc::new(db.fts_retriever());
        let chain_qa = crate::chain_qa::ChainQa::new(
            db.clone(),
            llm_registry.clone(),
            message_retriever.clone(),
        );
        let sessions = crate::api::auth::SessionStore::new(db.clone(), "svg-test-password".into());
        AppState {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            terminals,
            chain_qa,
            message_retriever,
            sessions,
            credential_helper: None,
            password: Some("svg-test-password".into()),
            login_throttle: crate::api::auth::LoginThrottle::new(),
            codex_login: crate::api::codex_login::CodexLoginManager::new(),
            deployment: Arc::new(crate::api::deployment::DeploymentConfig::for_tests()),
            runtime_env: Arc::new(phoenix_core::runtime_env::PhoenixRuntimeEnvironment::detect()),
            suggest_token: String::new(),
            discovery: crate::discovery::start(crate::discovery::DiscoveryConfig {
                enabled: false,
                ..crate::discovery::DiscoveryConfig::from_env()
            }),
            resource_monitor: crate::api::resource_monitor::ResourceMonitor::new(),
        }
    }

    #[tokio::test]
    async fn preview_source_and_download_require_authentication_and_exact_owner() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let state = authenticated_state().await;
        state
            .db
            .create_conversation("owner", "owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let artifact = state
            .db
            .publish_svg_artifact(
                "owner",
                "call",
                "Title",
                "Description",
                100.0,
                50.0,
                b"<svg/>",
            )
            .await
            .unwrap();
        let router = crate::api::create_router(state);
        for suffix in ["", "/source", "/download"] {
            let uri = format!(
                "/api/conversations/owner/svg-artifacts/{}{suffix}",
                artifact.artifact_id
            );
            let unauthenticated = router
                .clone()
                .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
            let authorized = router
                .clone()
                .oneshot(
                    Request::get(&uri)
                        .header(header::AUTHORIZATION, "Bearer svg-test-password")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(authorized.status(), StatusCode::OK);
            let wrong_owner = uri.replace("/owner/", "/other/");
            let unauthorized_owner = router
                .clone()
                .oneshot(
                    Request::get(wrong_owner)
                        .header(header::AUTHORIZATION, "Bearer svg-test-password")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(unauthorized_owner.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn all_representations_use_owned_snapshots_and_safe_headers() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("owner", "owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let artifact = db
            .publish_svg_artifact(
                "owner",
                "call",
                "Title",
                "Description",
                100.0,
                50.0,
                b"<svg/>",
            )
            .await
            .unwrap();
        assert!(matches!(
            owned_artifact(&db, "other", &artifact.artifact_id).await,
            Err(AppError::NotFound(_))
        ));
        for representation in [
            Representation::Image,
            Representation::Source,
            Representation::Download,
        ] {
            let response = artifact_response(
                owned_artifact(&db, "owner", &artifact.artifact_id)
                    .await
                    .unwrap(),
                representation,
            );
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert_eq!(
                response.headers()["cross-origin-resource-policy"],
                "same-origin"
            );
            assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .starts_with("sandbox; default-src 'none'"));
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            match representation {
                Representation::Source => assert_eq!(
                    response.headers()[header::CONTENT_TYPE],
                    "text/plain; charset=utf-8"
                ),
                Representation::Image | Representation::Download => assert_eq!(
                    response.headers()[header::CONTENT_DISPOSITION],
                    "attachment; filename=visualization.svg"
                ),
            }
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 100)
                    .await
                    .unwrap()
                    .as_ref(),
                b"<svg/>"
            );
        }
    }
}
