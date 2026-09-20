use super::handlers::AppError;
use super::AppState;
use axum::extract::{Path, State};
use axum::response::Response;

async fn shared_owner(state: &AppState, token: &str) -> Result<String, AppError> {
    state
        .db
        .get_share_token_by_token(token)
        .await
        .map_err(|error| {
            tracing::error!(%error, "failed to resolve SVG share authorization");
            AppError::Internal("Unable to read shared visualization".to_owned())
        })?
        .map(|(conversation_id, _)| conversation_id)
        .ok_or_else(|| AppError::NotFound("Share link not found or revoked".to_owned()))
}

pub(super) async fn image(
    State(state): State<AppState>,
    Path((token, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let owner = shared_owner(&state, &token).await?;
    super::svg_artifacts::image(State(state), Path((owner, artifact_id))).await
}

pub(super) async fn source(
    State(state): State<AppState>,
    Path((token, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let owner = shared_owner(&state, &token).await?;
    super::svg_artifacts::source(State(state), Path((owner, artifact_id))).await
}

pub(super) async fn download(
    State(state): State<AppState>,
    Path((token, artifact_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let owner = shared_owner(&state, &token).await?;
    super::svg_artifacts::download(State(state), Path((owner, artifact_id))).await
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    async fn get(router: &axum::Router, uri: &str) -> axum::response::Response {
        router
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn shared_svg_artifact_retrieval_is_token_scoped_and_revocable_without_password() {
        let mut state = super::super::handlers::hard_delete_cascade_tests::make_test_state().await;
        state.password = Some("private-instance".to_owned());
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"/>"#;
        let svg = crate::tools::present_svg::validation::validate(bytes).unwrap();
        let invocation =
            crate::tools::present_svg::validation::SvgInvocationId::new("agent", "call");
        for owner in ["shared-owner", "other-owner"] {
            state
                .db
                .create_conversation(owner, owner, "/tmp", true, None, None)
                .await
                .unwrap();
        }
        let artifact = state
            .db
            .publish_svg_artifact(
                "shared-owner",
                &invocation,
                &phoenix_svg::SvgPresentationMetadata::new("Chart", "Bars").unwrap(),
                &svg,
            )
            .await
            .established()
            .unwrap()
            .unwrap();
        let token = state.db.create_share_token("shared-owner").await.unwrap();
        let other_token = state.db.create_share_token("other-owner").await.unwrap();
        let router = crate::api::create_router(state.clone());
        for suffix in ["", "/source", "/download"] {
            let shared_uri = format!(
                "/api/share/{token}/svg-artifacts/{}{suffix}",
                artifact.artifact_id
            );
            let response = get(&router, &shared_uri).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("sandbox"));
            assert_eq!(
                response.headers()["cross-origin-resource-policy"],
                "same-origin"
            );
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            if suffix == "/source" {
                assert_eq!(
                    response.headers()[header::CONTENT_TYPE],
                    "text/plain; charset=utf-8"
                );
            } else {
                assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
                assert!(response.headers()[header::CONTENT_DISPOSITION]
                    .to_str()
                    .unwrap()
                    .starts_with("attachment"));
            }
            assert_eq!(
                to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
                bytes
            );
            for denied_token in ["invalid-token", &other_token] {
                let denied_uri = format!("/api/share/{denied_token}/svg-artifacts/{}{suffix}?conversation_id=shared-owner", artifact.artifact_id);
                let denied = get(&router, &denied_uri).await;
                assert_eq!(denied.status(), StatusCode::NOT_FOUND);
            }
            let protected_uri = format!(
                "/api/conversations/shared-owner/svg-artifacts/{}{suffix}",
                artifact.artifact_id
            );
            let protected = get(&router, &protected_uri).await;
            assert_eq!(protected.status(), StatusCode::UNAUTHORIZED);
        }
        state.db.delete_share_token("shared-owner").await.unwrap();
        for suffix in ["", "/source", "/download"] {
            let uri = format!(
                "/api/share/{token}/svg-artifacts/{}{suffix}",
                artifact.artifact_id
            );
            let response = get(&router, &uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        assert!(state
            .db
            .svg_artifact("shared-owner", &artifact.artifact_id)
            .await
            .unwrap()
            .is_some());
    }
}
