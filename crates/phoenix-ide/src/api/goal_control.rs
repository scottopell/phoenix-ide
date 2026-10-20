use std::sync::Arc;

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use phoenix_core::domain::conversation_goal::{GoalHistoryEntry, GoalVersion};
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::conversation_goals::control::{ControlError, ControlOutcome, ControlReceipt};
use phoenix_db::conversation_goals::{
    user_commands::{GoalUserReceipt, GoalUserResult},
    GoalDbError,
};
use serde::Deserialize;

use super::auth::OwnerAuthenticated;
use crate::goal_service::user_commands::{
    GoalCommandService, GoalCommandServiceError, GoalCreateRequest, GoalStateRequest,
};
use crate::runtime::goal_control::{
    GoalControlRequest, GoalControlService, GoalControlServiceError,
};
use crate::runtime::RuntimeManager;

type ApiError = (StatusCode, Json<serde_json::Value>);

struct GoalServices {
    commands: GoalCommandService,
    controls: GoalControlService,
}

type InstalledServices = Option<Extension<Arc<GoalServices>>>;

pub(super) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            "/api/product-conversations/:reference/goal",
            get(current).post(create),
        )
        .route(
            "/api/product-conversations/:reference/goal/history",
            get(history),
        )
        .route(
            "/api/product-conversations/:reference/goal/state",
            post(set_state),
        )
        .route(
            "/api/product-conversations/:reference/goal-control",
            post(apply),
        )
}

#[allow(dead_code)]
pub(crate) fn private_install<S: Clone + Send + Sync + 'static>(
    runtime: Arc<RuntimeManager>,
) -> Result<Router<S>, GoalCommandServiceError> {
    let controls = GoalControlService::from_installed_database(runtime.db().clone())
        .map_err(|_| GoalCommandServiceError::Disabled)?;
    let commands = GoalCommandService::from_installed_runtime(runtime)?;
    Ok(routes().layer(Extension(Arc::new(GoalServices { commands, controls }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    after: GoalVersion,
}

async fn current(
    authenticated: OwnerAuthenticated,
    Path(reference): Path<String>,
    services: InstalledServices,
) -> Result<Json<Option<GoalHistoryEntry>>, ApiError> {
    let services = installed(services)?;
    let owner = canonical_owner(reference)?;
    services
        .commands
        .current(authenticated, owner)
        .await
        .map(Json)
        .map_err(|error| command_error(&error))
}

async fn history(
    authenticated: OwnerAuthenticated,
    Path(reference): Path<String>,
    services: InstalledServices,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<GoalHistoryEntry>>, ApiError> {
    let services = installed(services)?;
    let owner = canonical_owner(reference)?;
    services
        .commands
        .history(authenticated, owner, query.after)
        .await
        .map(Json)
        .map_err(|error| command_error(&error))
}

async fn create(
    authenticated: OwnerAuthenticated,
    Path(reference): Path<String>,
    services: InstalledServices,
    Json(request): Json<GoalCreateRequest>,
) -> Result<(StatusCode, Json<GoalUserReceipt>), ApiError> {
    let services = installed(services)?;
    let owner = canonical_owner(reference)?;
    let receipt = services
        .commands
        .create(authenticated, owner, request)
        .await
        .map_err(|error| command_error(&error))?;
    Ok(user_receipt(receipt))
}

async fn set_state(
    authenticated: OwnerAuthenticated,
    Path(reference): Path<String>,
    services: InstalledServices,
    Json(request): Json<GoalStateRequest>,
) -> Result<(StatusCode, Json<GoalUserReceipt>), ApiError> {
    let services = installed(services)?;
    let owner = canonical_owner(reference)?;
    let receipt = services
        .commands
        .state(authenticated, owner, request)
        .await
        .map_err(|error| command_error(&error))?;
    Ok(user_receipt(receipt))
}

fn installed(services: InstalledServices) -> Result<Arc<GoalServices>, ApiError> {
    services.map(|Extension(services)| services).ok_or_else(|| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "goal services are not installed",
        )
    })
}

fn canonical_owner(reference: String) -> Result<ProductConversationId, ApiError> {
    ProductConversationId::parse(reference)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid owner identity"))
}

fn user_receipt(receipt: GoalUserReceipt) -> (StatusCode, Json<GoalUserReceipt>) {
    let status = match &receipt.result {
        GoalUserResult::Accepted { .. } => StatusCode::OK,
        GoalUserResult::Rejected { .. } => StatusCode::CONFLICT,
    };
    (status, Json(receipt))
}

fn command_error(error: &GoalCommandServiceError) -> ApiError {
    match error {
        GoalCommandServiceError::Disabled => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "goal services are not installed",
        ),
        GoalCommandServiceError::Goal(GoalDbError::Domain(_)) => {
            api_error(StatusCode::BAD_REQUEST, "invalid goal command")
        }
        GoalCommandServiceError::Storage(phoenix_db::DbError::ConversationNotFound(_))
        | GoalCommandServiceError::Goal(GoalDbError::Admission(
            phoenix_db::DbError::ConversationNotFound(_),
        )) => api_error(StatusCode::NOT_FOUND, "ordinary owner unavailable"),
        GoalCommandServiceError::Goal(
            GoalDbError::Busy | GoalDbError::TargetChanged | GoalDbError::RetryConflict,
        )
        | GoalCommandServiceError::Storage(phoenix_db::DbError::CloseAdmissionFenced(_))
        | GoalCommandServiceError::Storage(phoenix_db::DbError::ProductConversationUnavailable(
            _,
        ))
        | GoalCommandServiceError::Goal(GoalDbError::Admission(
            phoenix_db::DbError::CloseAdmissionFenced(_),
        )) => api_error(
            StatusCode::CONFLICT,
            "goal command admission changed or is fenced",
        ),
        error => {
            tracing::error!(%error, "goal command or factual read failed");
            api_error(StatusCode::INTERNAL_SERVER_ERROR, "goal persistence failed")
        }
    }
}

async fn apply(
    authenticated: OwnerAuthenticated,
    Path(owner): Path<String>,
    services: InstalledServices,
    Json(request): Json<GoalControlRequest>,
) -> Result<(StatusCode, Json<ControlReceipt>), ApiError> {
    let services = installed(services)?;
    let owner = canonical_owner(owner)?;
    let receipt = services
        .controls
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
mod install_tests;

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
