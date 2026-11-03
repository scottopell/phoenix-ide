//! REQ-REM-006/008: owner-scoped management and model-independent visible facts.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use phoenix_core::domain::reminder::{
    Reminder, ReminderId, ReminderNotification, ReminderRevision, ReminderTimestampUs,
};
use phoenix_db::{DbError, DueReminderCursor, ReminderChange, RescheduleReminderInput};
use phoenix_tools::reminders::ReminderWhen;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{handlers::AppError, AppState};

#[allow(clippy::wildcard_enum_match_arm)]
fn db_error(error: DbError) -> AppError {
    match error {
        DbError::ConversationNotFound(_) => AppError::NotFound("Conversation not found".into()),
        DbError::ReminderInvalid(message) => AppError::BadRequest(message),
        DbError::ProductConversationUnavailable(id) => {
            AppError::Conflict(Box::new(super::types::ConflictErrorResponse::new(
                format!("ProductConversation {id} is read-only in History"),
                "product_conversation_not_open",
            )))
        }
        DbError::CloseAdmissionFenced(_) => {
            AppError::Conflict(Box::new(super::types::ConflictErrorResponse::new(
                "Conversation cannot accept reminder changes while Close is in progress",
                "close_admission_fenced",
            )))
        }
        error => AppError::Internal(error.to_string()),
    }
}

async fn transcript(state: &AppState, id: &str) -> Result<String, AppError> {
    match state.db.get_conversation(id).await {
        Ok(conversation) => Ok(conversation.id),
        Err(DbError::ConversationNotFound(_)) => state
            .db
            .get_conversation_by_slug(id)
            .await
            .map(|c| c.id)
            .map_err(db_error),
        Err(error) => Err(db_error(error)),
    }
}

pub(super) async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Reminder>>, AppError> {
    let id = transcript(&state, &id).await?;
    Ok(Json(
        state.db.list_reminders(&id, 50).await.map_err(db_error)?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RevisionRequest {
    expected_revision: ReminderRevision,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RescheduleRequest {
    expected_revision: ReminderRevision,
    when: ReminderWhen,
}

fn change_response(change: ReminderChange) -> Response {
    let status = match &change {
        ReminderChange::Updated { .. } => StatusCode::OK,
        ReminderChange::NotFound => StatusCode::NOT_FOUND,
        ReminderChange::RevisionConflict { .. }
        | ReminderChange::AlreadyDue { .. }
        | ReminderChange::AlreadyCancelled { .. } => StatusCode::CONFLICT,
    };
    (status, Json(change)).into_response()
}

pub(super) async fn cancel(
    State(state): State<AppState>,
    Path((id, reminder_id)): Path<(String, ReminderId)>,
    Json(request): Json<RevisionRequest>,
) -> Result<Response, AppError> {
    let id = transcript(&state, &id).await?;
    let now = ReminderTimestampUs::parse(Utc::now().timestamp_micros())
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(change_response(
        crate::reminder_service::cancel(
            &state.runtime,
            &id,
            &reminder_id,
            request.expected_revision,
            now,
        )
        .await
        .map_err(db_error)?,
    ))
}

pub(super) async fn reschedule(
    State(state): State<AppState>,
    Path((id, reminder_id)): Path<(String, ReminderId)>,
    Json(request): Json<RescheduleRequest>,
) -> Result<Response, AppError> {
    let id = transcript(&state, &id).await?;
    let now = Utc::now();
    let (due_at_us, timezone) = request.when.resolve(now).map_err(AppError::BadRequest)?;
    let input = RescheduleReminderInput {
        source_transcript_id: id,
        reminder_id,
        expected_revision: request.expected_revision,
        due_at_us,
        timezone,
        now_us: ReminderTimestampUs::parse(now.timestamp_micros())
            .map_err(|e| AppError::BadRequest(e.to_string()))?,
    };
    Ok(change_response(
        crate::reminder_service::reschedule(&state.runtime, &input)
            .await
            .map_err(db_error)?,
    ))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FeedQuery {
    after_due_at_us: Option<ReminderTimestampUs>,
    after_id: Option<ReminderId>,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderFeedCursor {
    pub due_at_us: ReminderTimestampUs,
    pub reminder_id: ReminderId,
}

#[derive(Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ReminderFeedResponse {
    pub items: Vec<ReminderNotification>,
    pub next: Option<ReminderFeedCursor>,
}

pub(super) async fn due_feed(
    State(state): State<AppState>,
    Query(query): Query<FeedQuery>,
) -> Result<Json<ReminderFeedResponse>, AppError> {
    let after = match (query.after_due_at_us, query.after_id) {
        (None, None) => None,
        (Some(due_at_us), Some(reminder_id)) => Some(DueReminderCursor {
            due_at_us,
            reminder_id,
        }),
        _ => return Err(AppError::BadRequest("Supply both cursor fields".into())),
    };
    let since = ReminderTimestampUs::parse(
        (Utc::now() - chrono::Duration::days(1))
            .timestamp_micros()
            .max(0),
    )
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let items = state
        .db
        .reminder_notification_feed(since, after.as_ref(), 100)
        .await
        .map_err(db_error)?;
    let next = if items.len() == 100 {
        items.last().map(|last| ReminderFeedCursor {
            due_at_us: last.reminder.due_at_us,
            reminder_id: last.reminder.id.clone(),
        })
    } else {
        None
    };
    Ok(Json(ReminderFeedResponse { items, next }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::reminder::{ReminderText, ReminderTimezone};

    #[tokio::test]
    async fn management_is_owner_scoped_and_reports_conflicts_without_erasing_due_facts() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        for id in ["rem-owner", "other-owner"] {
            state
                .db
                .create_conversation(id, id, "/tmp", true, None, None)
                .await
                .unwrap();
        }
        let now = Utc::now().timestamp_micros();
        let reminder = state
            .db
            .create_reminder(&phoenix_db::CreateReminderInput {
                source_transcript_id: "rem-owner".into(),
                source_message_id: "assistant".into(),
                source_tool_use_id: "call".into(),
                request_fingerprint: "a".repeat(64),
                text: ReminderText::parse("Check Web Inspector").unwrap(),
                timezone: ReminderTimezone::parse("UTC").unwrap(),
                due_at_us: ReminderTimestampUs::parse(now + 60_000_000).unwrap(),
                now_us: ReminderTimestampUs::parse(now).unwrap(),
            })
            .await
            .unwrap();
        let result = cancel(
            State(state.clone()),
            Path(("other-owner".into(), reminder.id.clone())),
            Json(RevisionRequest {
                expected_revision: reminder.revision,
            }),
        )
        .await
        .unwrap();
        assert_eq!(result.status(), StatusCode::NOT_FOUND);
        let outdated = cancel(
            State(state.clone()),
            Path(("rem-owner".into(), reminder.id.clone())),
            Json(RevisionRequest {
                expected_revision: ReminderRevision::parse(2).unwrap(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(outdated.status(), StatusCode::CONFLICT);
        state
            .db
            .promote_due_reminders(ReminderTimestampUs::parse(now + 61_000_000).unwrap(), 10)
            .await
            .unwrap();
        let due = cancel(
            State(state.clone()),
            Path(("rem-owner".into(), reminder.id)),
            Json(RevisionRequest {
                expected_revision: reminder.revision,
            }),
        )
        .await
        .unwrap();
        assert_eq!(due.status(), StatusCode::CONFLICT);
        let Json(rows) = list(State(state.clone()), Path("rem-owner".into()))
            .await
            .unwrap();
        assert!(matches!(
            rows[0].state,
            phoenix_core::domain::reminder::ReminderState::Due { .. }
        ));
        let Json(feed) = due_feed(State(state), Query(FeedQuery::default()))
            .await
            .unwrap();
        assert_eq!(feed.items[0].target.id, "rem-owner");
        assert_eq!(feed.items[0].reminder, rows[0]);
    }

    #[tokio::test]
    async fn reschedule_uses_shared_time_validation_and_observed_revision() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        state
            .db
            .create_conversation("resched-owner", "resched-owner", "/tmp", true, None, None)
            .await
            .unwrap();
        let now = Utc::now().timestamp_micros();
        let reminder = state
            .db
            .create_reminder(&phoenix_db::CreateReminderInput {
                source_transcript_id: "resched-owner".into(),
                source_message_id: "assistant".into(),
                source_tool_use_id: "call".into(),
                request_fingerprint: "a".repeat(64),
                text: ReminderText::parse("Inspect Safari").unwrap(),
                timezone: ReminderTimezone::parse("UTC").unwrap(),
                due_at_us: ReminderTimestampUs::parse(now + 60_000_000).unwrap(),
                now_us: ReminderTimestampUs::parse(now).unwrap(),
            })
            .await
            .unwrap();
        let invalid = reschedule(
            State(state.clone()),
            Path(("resched-owner".into(), reminder.id.clone())),
            Json(RescheduleRequest {
                expected_revision: reminder.revision,
                when: ReminderWhen::At {
                    local_datetime: "2030-11-03T01:30:00".into(),
                    timezone: ReminderTimezone::parse("America/New_York").unwrap(),
                    utc_offset: None,
                },
            }),
        )
        .await;
        assert!(
            matches!(invalid, Err(AppError::BadRequest(message)) if message.contains("occurs twice"))
        );
        let updated = reschedule(
            State(state.clone()),
            Path(("resched-owner".into(), reminder.id.clone())),
            Json(RescheduleRequest {
                expected_revision: reminder.revision,
                when: ReminderWhen::After { seconds: 3600 },
            }),
        )
        .await
        .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        assert_eq!(
            state
                .db
                .get_reminder("resched-owner", &reminder.id)
                .await
                .unwrap()
                .unwrap()
                .revision
                .get(),
            2
        );
        let conflict = reschedule(
            State(state.clone()),
            Path(("resched-owner".into(), reminder.id)),
            Json(RescheduleRequest {
                expected_revision: reminder.revision,
                when: ReminderWhen::After { seconds: 7200 },
            }),
        )
        .await
        .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn management_statuses_are_explicit() {
        assert_eq!(
            change_response(ReminderChange::NotFound).status(),
            StatusCode::NOT_FOUND
        );
        assert!(matches!(
            db_error(DbError::ReminderInvalid("invalid time".into())),
            AppError::BadRequest(_)
        ));
    }
}
