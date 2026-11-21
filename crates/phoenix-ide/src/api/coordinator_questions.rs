//! REQ-AUQ-010..012: the user's side of non-blocking Coordinator questions.
use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    Json,
};
use phoenix_core::domain::sm_state::{QuestionAnnotation, UserQuestion};
use phoenix_db::{CoordinatorQuestionClose, CoordinatorQuestionSettlement};
use serde::{Deserialize, Serialize};

use super::{
    handlers::AppError,
    types::{ConflictErrorResponse, SuccessResponse},
    AppState,
};

/// An open question from the Coordinator, as the client renders it.
#[derive(Debug, Serialize)]
pub(super) struct CoordinatorQuestionView {
    request_id: String,
    coordinator_conversation_id: String,
    /// RFC 3339.
    asked_at: String,
    questions: Vec<UserQuestion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnswerCoordinatorQuestionRequest {
    answers: HashMap<String, String>,
    #[serde(default)]
    annotations: Option<HashMap<String, QuestionAnnotation>>,
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(error.to_string())
}

pub(super) async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<CoordinatorQuestionView>>, AppError> {
    let open = state
        .db
        .open_coordinator_questions()
        .await
        .map_err(internal)?;
    Ok(Json(
        open.into_iter()
            .map(|question| CoordinatorQuestionView {
                request_id: question.request_id,
                coordinator_conversation_id: question.coordinator_conversation_id,
                asked_at: chrono::DateTime::<chrono::Utc>::from_timestamp_micros(
                    question.asked_at_us,
                )
                .unwrap_or_default()
                .to_rfc3339(),
                questions: question.questions,
            })
            .collect(),
    ))
}

fn settlement_result(
    settlement: CoordinatorQuestionSettlement,
) -> Result<Json<SuccessResponse>, AppError> {
    match settlement {
        CoordinatorQuestionSettlement::Settled => Ok(Json(SuccessResponse { success: true })),
        CoordinatorQuestionSettlement::NotFound => {
            Err(AppError::NotFound("Question not found".into()))
        }
        CoordinatorQuestionSettlement::AlreadySettled { status } => {
            Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!("This question is no longer open ({status})."),
                "stale_question_request",
            ))))
        }
    }
}

/// The direct-turn worker's input discovery delivers stored replies.
fn deliver_soon(state: &AppState) {
    state.runtime.kick_direct_turn_worker();
}

pub(super) async fn answer(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(request): Json<AnswerCoordinatorQuestionRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    let question = match state
        .db
        .open_coordinator_question(&request_id)
        .await
        .map_err(internal)?
    {
        Ok(question) => question,
        Err(closed) => return settlement_result(closed),
    };
    if !phoenix_core::domain::sm_state::answers_cover_questions(
        &question.questions,
        &request.answers,
        request.annotations.as_ref(),
    ) {
        return Err(AppError::BadRequest(
            "Answer every question exactly once, keyed by its full text, before submitting.".into(),
        ));
    }
    let reply = crate::coordinator_question_delivery::answer_reply(
        &request_id,
        &question.questions,
        &request.answers,
        request.annotations.as_ref(),
    );
    let settlement = state
        .db
        .settle_coordinator_question(
            &request_id,
            CoordinatorQuestionClose::Answered { reply: &reply },
            chrono::Utc::now().timestamp_micros(),
        )
        .await
        .map_err(internal)?;
    let result = settlement_result(settlement)?;
    deliver_soon(&state);
    Ok(result)
}

pub(super) async fn dismiss(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let reply = crate::coordinator_question_delivery::dismissal_reply(&request_id);
    let settlement = state
        .db
        .settle_coordinator_question(
            &request_id,
            CoordinatorQuestionClose::Dismissed { reply: &reply },
            chrono::Utc::now().timestamp_micros(),
        )
        .await
        .map_err(internal)?;
    let result = settlement_result(settlement)?;
    deliver_soon(&state);
    Ok(result)
}
