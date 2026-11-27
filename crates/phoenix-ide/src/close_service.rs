//! Close orchestration shared by application entry points.

use crate::api::handlers::AppError;
use crate::api::{AppState, ConflictErrorResponse};
use phoenix_core::domain::close::TranscriptConversationId;

pub(crate) async fn require_close_settlement_without_retained_custody(
    state: &AppState,
    attempt_id: &phoenix_core::domain::close::CloseAttemptId,
) -> Result<(), AppError> {
    state
        .db
        .require_no_retained_legacy_creation_for_retirement(attempt_id)
        .await
        .map_err(|error| match error {
            crate::db::DbError::CloseFoundationPrecondition(detail) => AppError::Conflict(
                Box::new(ConflictErrorResponse::new(detail, "close_settlement_in_progress")),
            ),
            error => AppError::Internal(error.to_string()),
        })
}

#[allow(clippy::too_many_lines, clippy::single_match_else)]
pub(crate) async fn run_legacy_close_compat(
    state: &AppState,
    id: &str,
    action: &str,
    require_idle_latest: bool,
) -> Result<(), AppError> {
    use phoenix_core::domain::close::{CloseAttemptId, CloseCompletionOutcome, ClosePhase};

    let transcript = state
        .db
        .get_conversation(id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    if action != "archive"
        && action != "archive chain"
        && transcript.conv_mode.worktree_path().is_none()
    {
        return Err(AppError::BadRequest(format!(
            "Conversation must own an allocated worktree to {action}"
        )));
    }
    let aggregate = state
        .db
        .get_ordinary_product_conversation(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let active_transcript = aggregate
        .segments
        .last()
        .map(|segment| segment.transcript_row.conversation.id.as_str());
    if active_transcript != Some(id) {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close is accepted only from the active aggregate transcript",
            "inactive_close_transcript",
        ))));
    }
    let expected_latest_transcript = TranscriptConversationId::parse(id.to_string())
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let mut obligation = match state
        .db
        .get_active_close_obligation_for_product(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
    {
        Some(obligation) => obligation,
        None => {
            if matches!(
                state
                    .db
                    .product_conversation_admission(&transcript.id)
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?,
                phoenix_db::ProductConversationAdmission::History(_)
            ) {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "ProductConversation is already in History",
                    "close_already_history",
                ))));
            }
            let attempt_id = CloseAttemptId::parse(uuid::Uuid::new_v4().to_string())
                .expect("UUID is a valid Close attempt id");
            let begin = if require_idle_latest {
                state
                    .db
                    .begin_direct_close_foundation(
                        &transcript.product_conversation_id,
                        &expected_latest_transcript,
                        attempt_id.as_str(),
                    )
                    .await
            } else {
                state
                    .db
                    .begin_close_foundation(
                        &transcript.product_conversation_id,
                        &expected_latest_transcript,
                        attempt_id.as_str(),
                    )
                    .await
            };
            begin.map_err(|error| {
                let error_type =
                    if matches!(error, crate::db::DbError::CloseFoundationStaleLatest { .. }) {
                        "stale_latest_close_transcript"
                    } else {
                        "close_start_failed"
                    };
                AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    error.to_string(),
                    error_type,
                )))
            })?
        }
    };
    loop {
        obligation = match obligation.phase() {
            ClosePhase::AwaitingBlockerResolution => {
                if state
                    .db
                    .close_attempt_latest_was_busy(obligation.attempt_id().as_str())
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?
                {
                    let awaiting_confirmation = state
                        .db
                        .confirm_close_stop_work(obligation.attempt_id().as_str())
                        .await
                        .map_err(|error| {
                            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                                error.to_string(),
                                "close_start_failed",
                            )))
                        })?;
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        format!(
                            "Close attempt {} requires explicit stop-work confirmation",
                            awaiting_confirmation.attempt_id()
                        ),
                        "close_stop_work_confirmation_required",
                    ))));
                }
                state
                    .db
                    .begin_close_idle_settlement(obligation.attempt_id().as_str())
                    .await
                    .map_err(|error| {
                        AppError::Conflict(Box::new(ConflictErrorResponse::new(
                            error.to_string(),
                            "close_start_failed",
                        )))
                    })?
            }
            ClosePhase::AwaitingStopWorkConfirmation => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    format!(
                        "Close attempt {} still requires explicit stop-work confirmation",
                        obligation.attempt_id()
                    ),
                    "close_stop_work_confirmation_required",
                ))));
            }
            ClosePhase::SettlingActiveWork | ClosePhase::CancelRequestedDuringSettlement => {
                state
                    .runtime
                    .resume_close_settlements(Some(obligation.attempt_id()))
                    .await
                    .map_err(|error| {
                        AppError::Conflict(Box::new(ConflictErrorResponse::new(
                            error,
                            "close_settlement_in_progress",
                        )))
                    })?;
                let advanced = state
                    .db
                    .get_close_obligation(obligation.attempt_id().as_str())
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?;
                if advanced.phase() == ClosePhase::SettlingActiveWork {
                    require_close_settlement_without_retained_custody(state, advanced.attempt_id())
                        .await?;
                }
                if matches!(
                    advanced.phase(),
                    ClosePhase::SettlingActiveWork | ClosePhase::CancelRequestedDuringSettlement
                ) {
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        "Close settlement remains in progress",
                        "close_settlement_in_progress",
                    ))));
                }
                advanced
            }
            ClosePhase::AwaitingRetirementInspection => {
                if let Err(error) = state
                    .runtime
                    .inspect_close_retirement(obligation.attempt_id().clone())
                    .await
                {
                    let current = state
                        .db
                        .get_close_obligation(obligation.attempt_id().as_str())
                        .await
                        .map_err(|db_error| AppError::Internal(db_error.to_string()))?;
                    let error_type = if current.close_outcome()
                        == Some(CloseCompletionOutcome::CloseIncomplete)
                    {
                        "close_incomplete"
                    } else if current.phase() == ClosePhase::NeedsRepair {
                        "close_retirement_needs_repair"
                    } else {
                        "close_inspection_failed"
                    };
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        error, error_type,
                    ))));
                }
                state
                    .db
                    .get_close_obligation(obligation.attempt_id().as_str())
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?
            }
            ClosePhase::AwaitingLossConfirmation => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Close requires confirmation of the exact persisted loss inventory",
                    "close_loss_confirmation_required",
                ))));
            }
            ClosePhase::RetirementRequested => {
                if let Err(error) = state
                    .runtime
                    .retire_close_runtime_resources(obligation.attempt_id().clone())
                    .await
                {
                    let current = state
                        .db
                        .get_close_obligation(obligation.attempt_id().as_str())
                        .await
                        .map_err(|db_error| AppError::Internal(db_error.to_string()))?;
                    let error_type = if current.close_outcome()
                        == Some(CloseCompletionOutcome::CloseIncomplete)
                    {
                        "close_incomplete"
                    } else {
                        "close_retirement_needs_repair"
                    };
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        error, error_type,
                    ))));
                }
                return Ok(());
            }
            ClosePhase::NeedsRepair => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Close retirement requires repair before it can continue",
                    "close_retirement_needs_repair",
                ))));
            }
            ClosePhase::Completed => {
                return match obligation.close_outcome() {
                    Some(
                        CloseCompletionOutcome::Archived
                        | CloseCompletionOutcome::ArchivedCleanupAttention,
                    ) => Ok(()),
                    Some(CloseCompletionOutcome::CloseIncomplete) => {
                        Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                            "Close could not confirm that conversation and process shutdown completed",
                            "close_incomplete",
                        ))))
                    }
                    Some(CloseCompletionOutcome::Cancelled) => {
                        Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                            "Close was cancelled before archival completed",
                            "close_cancelled",
                        ))))
                    }
                    None => Err(AppError::Internal(
                        "completed Close attempt has no terminal outcome".to_string(),
                    )),
                };
            }
        };
    }
}
