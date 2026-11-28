//! Close orchestration shared by application entry points.

use crate::api::handlers::AppError;
use crate::api::{AppState, ConflictErrorResponse};
use phoenix_core::domain::close::{
    CloseAttemptId, CloseObligation, ClosePhase, TranscriptConversationId,
};
use phoenix_db::GenericCloseAdmission;

pub(crate) async fn require_close_settlement_without_retained_custody(
    state: &AppState,
    attempt_id: &phoenix_core::domain::close::CloseAttemptId,
) -> Result<(), AppError> {
    state
        .db
        .require_no_retained_legacy_creation_for_retirement(attempt_id)
        .await
        .map_err(|error| {
            let crate::db::DbError::CloseFoundationPrecondition(detail) = error else {
                return AppError::Internal(error.to_string());
            };
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                detail,
                "close_settlement_in_progress",
            )))
        })
}

fn close_settlement_conflict(detail: String) -> AppError {
    AppError::Conflict(Box::new(ConflictErrorResponse::new(
        detail,
        "close_settlement_in_progress",
    )))
}

async fn project_close_worker_failure(
    state: &AppState,
    attempt_id: &CloseAttemptId,
    detail: String,
) -> AppError {
    use phoenix_core::domain::close::CloseCompletionOutcome;

    let current = match state.db.get_close_obligation(attempt_id.as_str()).await {
        Ok(current) => current,
        Err(error) => return AppError::Internal(error.to_string()),
    };
    let error_type = match (current.phase(), current.close_outcome()) {
        (_, Some(CloseCompletionOutcome::CloseIncomplete)) => "close_incomplete",
        (ClosePhase::NeedsRepair | ClosePhase::RetirementRequested, _) => {
            "close_retirement_needs_repair"
        }
        (ClosePhase::AwaitingRetirementInspection, _) => "close_inspection_failed",
        _ => "close_settlement_in_progress",
    };
    AppError::Conflict(Box::new(ConflictErrorResponse::new(detail, error_type)))
}

async fn observe_close_settlement(
    state: &AppState,
    attempt_id: &CloseAttemptId,
) -> Result<CloseObligation, AppError> {
    let advanced = state
        .db
        .get_close_obligation(attempt_id.as_str())
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    if advanced.phase() == ClosePhase::SettlingActiveWork {
        require_close_settlement_without_retained_custody(state, advanced.attempt_id()).await?;
    }
    if matches!(
        advanced.phase(),
        ClosePhase::SettlingActiveWork | ClosePhase::CancelRequestedDuringSettlement
    ) {
        return Err(close_settlement_conflict(
            "Close settlement remains in progress".into(),
        ));
    }
    Ok(advanced)
}

#[allow(clippy::too_many_lines, clippy::single_match_else)]
pub(crate) async fn run_legacy_close_compat(
    state: &AppState,
    id: &str,
    action: &str,
) -> Result<(), AppError> {
    use phoenix_core::domain::close::CloseCompletionOutcome;

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
            let admission = state
                .db
                .begin_generic_close_settlement(
                    &transcript.product_conversation_id,
                    &expected_latest_transcript,
                    &attempt_id,
                )
                .await
                .map_err(|error| {
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
                })?;
            match admission {
                GenericCloseAdmission::InsertedSettling(settlement) => {
                    let attempt_id = settlement.obligation().attempt_id().clone();
                    let worker = state
                        .runtime
                        .start_generic_close_settlement(settlement)
                        .await
                        .map_err(close_settlement_conflict)?;
                    let result = worker.await.unwrap_or_else(|error| {
                        Err(format!("Close settlement worker failed: {error}"))
                    });
                    if let Err(error) = result {
                        return Err(project_close_worker_failure(state, &attempt_id, error).await);
                    }
                    observe_close_settlement(state, &attempt_id).await?
                }
                GenericCloseAdmission::InsertedAwaitingStopWork(obligation)
                | GenericCloseAdmission::Existing(obligation) => obligation,
            }
        }
    };
    loop {
        obligation = match obligation.phase() {
            ClosePhase::AwaitingBlockerResolution => state
                .db
                .prepare_close_initial_settlement(obligation.attempt_id().as_str())
                .await
                .map_err(|error| {
                    AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        error.to_string(),
                        "close_start_failed",
                    )))
                })?,
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
                if let Err(error) = state
                    .runtime
                    .resume_close_settlements(Some(obligation.attempt_id()))
                    .await
                {
                    return Err(
                        project_close_worker_failure(state, obligation.attempt_id(), error).await,
                    );
                }
                observe_close_settlement(state, obligation.attempt_id()).await?
            }
            ClosePhase::AwaitingRetirementInspection | ClosePhase::RetirementRequested => {
                let result = state
                    .runtime
                    .continue_legacy_close(obligation.attempt_id().clone())
                    .await;
                if let Err(error) = result {
                    return Err(
                        project_close_worker_failure(state, obligation.attempt_id(), error).await,
                    );
                }
                let current = state
                    .db
                    .get_close_obligation(obligation.attempt_id().as_str())
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?;
                if matches!(
                    current.phase(),
                    ClosePhase::AwaitingRetirementInspection | ClosePhase::RetirementRequested
                ) {
                    let error_type = if current.phase() == ClosePhase::RetirementRequested {
                        "close_retirement_needs_repair"
                    } else {
                        "close_inspection_failed"
                    };
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        "Close continuation has not reached a durable outcome",
                        error_type,
                    ))));
                }
                current
            }
            ClosePhase::AwaitingLossConfirmation => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Close requires confirmation of the exact persisted loss inventory",
                    "close_loss_confirmation_required",
                ))));
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
