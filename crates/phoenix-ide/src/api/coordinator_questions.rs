//! REQ-AUQ-010..012: the user's side of non-blocking Coordinator questions.
use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    Json,
};
use phoenix_core::domain::sm_state::{QuestionAnnotation, UserQuestion};
use phoenix_db::{CoordinatorQuestionReplyKind, CoordinatorQuestionSettlement};
use serde::{Deserialize, Serialize};

use super::{handlers::AppError, types::ConflictErrorResponse, AppState};

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

#[derive(Debug, Serialize)]
pub(super) struct CoordinatorQuestionReplyResponse {
    success: bool,
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
) -> Result<Json<CoordinatorQuestionReplyResponse>, AppError> {
    match settlement {
        CoordinatorQuestionSettlement::Settled => {
            Ok(Json(CoordinatorQuestionReplyResponse { success: true }))
        }
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

fn deliver_soon(state: &AppState) {
    let runtime = state.runtime.clone();
    tokio::spawn(async move {
        crate::coordinator_question_delivery::deliver_pass(&runtime).await;
    });
}

pub(super) async fn answer(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(request): Json<AnswerCoordinatorQuestionRequest>,
) -> Result<Json<CoordinatorQuestionReplyResponse>, AppError> {
    let Some(question) = state
        .db
        .open_coordinator_question(&request_id)
        .await
        .map_err(internal)?
    else {
        let status = state
            .db
            .coordinator_question_status(&request_id)
            .await
            .map_err(internal)?;
        return settlement_result(
            status.map_or(CoordinatorQuestionSettlement::NotFound, |status| {
                CoordinatorQuestionSettlement::AlreadySettled { status }
            }),
        );
    };
    let answered_all = question.questions.iter().all(|q| {
        request
            .answers
            .get(&q.question)
            .is_some_and(|answer| !answer.trim().is_empty())
    });
    if !answered_all {
        return Err(AppError::BadRequest(
            "Answer every question before submitting.".into(),
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
        .reply_to_coordinator_question(
            &request_id,
            CoordinatorQuestionReplyKind::Answered,
            &reply,
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
) -> Result<Json<CoordinatorQuestionReplyResponse>, AppError> {
    let settlement = state
        .db
        .reply_to_coordinator_question(
            &request_id,
            CoordinatorQuestionReplyKind::Dismissed,
            &crate::coordinator_question_delivery::dismissal_reply(&request_id),
            chrono::Utc::now().timestamp_micros(),
        )
        .await
        .map_err(internal)?;
    let result = settlement_result(settlement)?;
    deliver_soon(&state);
    Ok(result)
}
