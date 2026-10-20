use axum::{extract::Path, http::StatusCode, Json};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::control::{ControlError, ControlOutcome, ControlReceipt};

use super::auth::OwnerAuthenticated;
use crate::runtime::goal_control::{
    GoalControlRequest, GoalControlService, GoalControlServiceError,
};

type ApiError = (StatusCode, Json<serde_json::Value>);

pub(super) async fn apply(
    authenticated: OwnerAuthenticated,
    Path(owner): Path<String>,
    Json(request): Json<GoalControlRequest>,
) -> Result<(StatusCode, Json<ControlReceipt>), ApiError> {
    let owner = ProductConversationId::parse(owner)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid owner identity"))?;
    let receipt = GoalControlService::default()
        .apply(authenticated, owner, request)
        .await
        .map_err(|error| classify_error(&error))?;
    let status = match &receipt.result {
        ControlOutcome::Accepted { .. } => StatusCode::OK,
        ControlOutcome::RejectedVersion { .. } => StatusCode::CONFLICT,
    };
    Ok((status, Json(receipt)))
}

fn api_error(status: StatusCode, message: &str) -> ApiError {
    (status, Json(serde_json::json!({"error": message})))
}

fn classify_error(error: &GoalControlServiceError) -> ApiError {
    match error {
        GoalControlServiceError::Disabled => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "goal control is not installed",
        ),
        GoalControlServiceError::Control(ControlError::OwnerUnavailable) => {
            api_error(StatusCode::NOT_FOUND, "ordinary owner unavailable")
        }
        GoalControlServiceError::Control(ControlError::RequestConflict) => {
            api_error(StatusCode::CONFLICT, "request identity has different input")
        }
        GoalControlServiceError::Control(ControlError::VersionRange) => {
            api_error(StatusCode::BAD_REQUEST, "control version out of range")
        }
        GoalControlServiceError::Control(ControlError::AdmissionStopped) => {
            api_error(StatusCode::CONFLICT, "control is stopped or uninitialized")
        }
        GoalControlServiceError::Control(ControlError::Storage(_) | ControlError::Integrity(_))
        | GoalControlServiceError::Storage(_)
        | GoalControlServiceError::HostIdentity(_)
        | GoalControlServiceError::Clock => {
            tracing::error!(%error, "goal control request failed");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "goal control persistence failed",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::post, Extension, Router};
    use tower::ServiceExt;

    fn request(extra_principal: bool) -> Request<Body> {
        let mut body = serde_json::json!({"request_id":uuid::Uuid::new_v4(),"command":"stop","expected_control_version":0});
        if extra_principal {
            body["principal_id"] = serde_json::json!(uuid::Uuid::new_v4());
        }
        Request::post("/control/owner")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn conversation_goal_control_http_requires_owner_and_remains_structurally_disabled() {
        let router = Router::new().route("/control/:owner", post(apply));
        let response = router.clone().oneshot(request(false)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let router = router.layer(Extension(OwnerAuthenticated));
        let response = router.clone().oneshot(request(false)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let response = router.oneshot(request(true)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
