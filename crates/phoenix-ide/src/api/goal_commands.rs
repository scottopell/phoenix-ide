use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use phoenix_core::domain::{
    conversation_goal::{GoalDisposition, GoalHistoryEntry, GoalId, GoalText, GoalVersion},
    product_conversation::ProductConversationId,
};
use phoenix_db::conversation_goals::{
    user_commands::{GoalUserChange, GoalUserReceipt, GoalUserResult},
    GoalDbError,
};
use serde::Deserialize;

use super::{auth::OwnerAuthenticated, handlers::AppError, types::ConflictErrorResponse, AppState};
use service::GoalCommandService;

mod service;
#[cfg(test)]
mod tests;

pub(super) fn routes() -> Router<AppState> {
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
            post(state),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    request_id: uuid::Uuid,
    expected_version: GoalVersion,
    objective: GoalText,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateRequest {
    request_id: uuid::Uuid,
    expected_version: GoalVersion,
    goal_id: GoalId,
    disposition: GoalDisposition,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    after: Option<GoalVersion>,
}

fn owner(reference: String) -> Result<ProductConversationId, AppError> {
    ProductConversationId::parse(reference).map_err(|error| AppError::BadRequest(error.to_string()))
}

async fn current(
    State(app): State<AppState>,
    Path(reference): Path<String>,
    _owner: OwnerAuthenticated,
) -> Result<Json<Option<GoalHistoryEntry>>, AppError> {
    GoalCommandService::new(app.runtime)
        .current(&owner(reference)?)
        .await
        .map(Json)
}

async fn history(
    State(app): State<AppState>,
    Path(reference): Path<String>,
    _owner: OwnerAuthenticated,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<GoalHistoryEntry>>, AppError> {
    GoalCommandService::new(app.runtime)
        .history(
            &owner(reference)?,
            query.after.unwrap_or(GoalVersion::INITIAL),
        )
        .await
        .map(Json)
}

async fn create(
    State(app): State<AppState>,
    Path(reference): Path<String>,
    authenticated: OwnerAuthenticated,
    Json(request): Json<CreateRequest>,
) -> Result<Response, AppError> {
    GoalCommandService::new(app.runtime)
        .command(
            owner(reference)?,
            request.request_id,
            request.expected_version,
            GoalUserChange::Create {
                objective: request.objective,
            },
            authenticated,
        )
        .await
        .map(receipt_response)
}

async fn state(
    State(app): State<AppState>,
    Path(reference): Path<String>,
    authenticated: OwnerAuthenticated,
    Json(request): Json<StateRequest>,
) -> Result<Response, AppError> {
    GoalCommandService::new(app.runtime)
        .command(
            owner(reference)?,
            request.request_id,
            request.expected_version,
            GoalUserChange::State {
                goal_id: request.goal_id,
                disposition: request.disposition,
            },
            authenticated,
        )
        .await
        .map(receipt_response)
}

fn receipt_response(receipt: GoalUserReceipt) -> Response {
    let status = match &receipt.result {
        GoalUserResult::Accepted { .. } => StatusCode::OK,
        GoalUserResult::Rejected { .. } => StatusCode::CONFLICT,
    };
    (status, Json(receipt)).into_response()
}

fn goal_error(error: GoalDbError) -> AppError {
    match error {
        GoalDbError::Admission(error) => super::product_conversations::db_to_app(error),
        error @ (GoalDbError::RetryConflict | GoalDbError::TargetChanged | GoalDbError::Busy) => {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error.to_string(),
                "goal_command_conflict",
            )))
        }
        error @ GoalDbError::Domain(_) => AppError::BadRequest(error.to_string()),
        error @ (GoalDbError::BindingConflict
        | GoalDbError::Control(_)
        | GoalDbError::DuplicateGoalId
        | GoalDbError::Sqlx(_)
        | GoalDbError::Integrity(_)) => AppError::Internal(error.to_string()),
    }
}
