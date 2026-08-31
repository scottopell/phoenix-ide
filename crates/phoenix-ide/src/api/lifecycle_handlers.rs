#![allow(clippy::wildcard_enum_match_arm)]
//! Conversation lifecycle HTTP handlers: task approval, abandon, mark-merged.

use super::handlers::AppError;
use super::types::{
    CancelCloseBeforeRetirementRequest, ConfirmCloseLossRetirementRequest,
    ConfirmCloseStopWorkRequest, ConflictErrorResponse, ForkDismissResponse, ForkPromoteResponse,
    ForkProposalListResponse, ForkProposalSummary, ForkSpawnResponse, RequestChangesRequest,
    RetryCloseRetirementRequest, SuccessResponse, TaskApprovalRequest, TaskApprovalResponse,
    TaskFeedbackRequest,
};
use super::AppState;
#[cfg(test)]
use crate::db::Conversation;
use crate::runtime::fork_resolve::ForkResolveError;
use crate::state_machine::state::TaskApprovalOutcome;
use crate::state_machine::{ConvState, Event};
use phoenix_core::domain::close::TranscriptConversationId;

use axum::{
    extract::{Path, State},
    Json,
};

// ============================================================
// Terminal-action gate (REQ-BED-031)
// ============================================================

/// Reject terminal user actions when the conversation has an existing
/// continuation, as required by REQ-BED-031 and
/// `bedrock.allium::TerminalActionRequiresNoContinuation`.
///
/// `action` is a human-readable verb phrase (e.g. `"abandon"`,
/// `"mark as merged"`) that appears in the error message so the UI can
/// present a coherent reason.
///
/// Returns 409 Conflict with `error_type = "continuation_exists"` so
/// the frontend can dispatch on it (Phase 5) — e.g. offer to route to
/// the continuation instead of showing the raw error text.
#[cfg(test)]
fn reject_if_continued(conv: &Conversation, action: &str) -> Result<(), AppError> {
    if let Some(continuation_id) = conv.continued_in_conv_id.as_deref() {
        return Err(AppError::Conflict(Box::new(
            ConflictErrorResponse::new(
                format!(
                    "Cannot {action} a conversation that has been continued. \
                     The action belongs on the continuation conversation ({continuation_id})."
                ),
                "continuation_exists",
            )
            .with_continuation_id(continuation_id),
        )));
    }
    Ok(())
}

#[cfg(test)]
fn ensure_terminal_action_legal(conv: &Conversation, action: &str) -> Result<(), AppError> {
    reject_if_continued(conv, action)?;

    if !conv.state.allows_terminal_action() {
        return Err(AppError::BadRequest(format!(
            "Conversation must be idle, context-exhausted, or in a recoverable error state to {action}"
        )));
    }

    Ok(())
}

// ============================================================
// Task Approval (REQ-BED-028)
// ============================================================

async fn ensure_task_approval_authorized(
    state: &AppState,
    conversation: &crate::db::Conversation,
) -> Result<(), AppError> {
    let approval_authority =
        crate::resource_authority::resolve_resource_authority(state.runtime.db(), conversation)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
            .task_approval_authority();
    if approval_authority
        == crate::resource_authority::TaskApprovalAuthority::GitBackedActiveWorkScope
    {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "Task approval requires an active Git-backed WorkScope".to_string(),
        ))
    }
}

pub(crate) async fn approve_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<TaskApprovalRequest>>,
) -> Result<Json<TaskApprovalResponse>, AppError> {
    // 1. Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    ensure_task_approval_authorized(&state, &conv).await?;

    let handoff = body.map(|Json(req)| req.handoff).unwrap_or_default();
    // 3. Dispatch approval event to state machine
    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalDecided {
                outcome: TaskApprovalOutcome::Approved { handoff },
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(TaskApprovalResponse {
        success: true,
        first_task: None, // Set by executor via SSE if applicable
    }))
}

pub(crate) async fn reject_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalDecided {
                outcome: TaskApprovalOutcome::Rejected,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

pub(crate) async fn task_feedback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TaskFeedbackRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalDecided {
                outcome: TaskApprovalOutcome::FeedbackProvided {
                    annotations: req.annotations,
                },
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Fork proposal resolution (REQ-PROJ-034 / 037)
// ============================================================

/// Map a typed fork-resolve failure to the matching HTTP status. A non-pending
/// proposal / terminal origin / branch collision is a 409; an unknown proposal
/// is a 404; everything else is a 500-class internal error.
fn fork_resolve_app_error(e: ForkResolveError) -> AppError {
    match e {
        ForkResolveError::NotFound(m) => AppError::NotFound(m),
        ForkResolveError::Conflict(m) => AppError::Conflict(Box::new(ConflictErrorResponse::new(
            m,
            "fork_proposal_conflict",
        ))),
        ForkResolveError::Internal(m) => AppError::Internal(m),
    }
}

/// Validate that `proposal_id` belongs to `conv_id`, returning 404 when the
/// proposal is unknown and 400 when it belongs to a different conversation.
async fn require_proposal_for_conversation(
    state: &AppState,
    conv_id: &str,
    proposal_id: &str,
) -> Result<(), AppError> {
    let proposal = state
        .db
        .get_fork_proposal(proposal_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("fork proposal {proposal_id}")))?;
    if proposal.origin_conversation_id != conv_id {
        return Err(AppError::BadRequest(format!(
            "fork proposal {proposal_id} does not belong to conversation {conv_id}"
        )));
    }
    Ok(())
}

/// `POST /api/conversations/:id/proposals/:proposal_id/approve` — spawn a Work
/// fork (REQ-PROJ-034).
pub(crate) async fn approve_fork_proposal(
    State(state): State<AppState>,
    Path((id, proposal_id)): Path<(String, String)>,
) -> Result<Json<ForkSpawnResponse>, AppError> {
    require_proposal_for_conversation(&state, &id, &proposal_id).await?;
    let fork_conversation_id = state
        .runtime
        .approve_fork_proposal(&proposal_id)
        .await
        .map_err(fork_resolve_app_error)?;
    Ok(Json(ForkSpawnResponse {
        fork_conversation_id,
    }))
}

/// `POST /api/conversations/:id/proposals/:proposal_id/dismiss` — record the
/// proposal as dismissed (REQ-PROJ-034). Idempotent.
pub(crate) async fn dismiss_fork_proposal(
    State(state): State<AppState>,
    Path((id, proposal_id)): Path<(String, String)>,
) -> Result<Json<ForkDismissResponse>, AppError> {
    require_proposal_for_conversation(&state, &id, &proposal_id).await?;
    let transitioned = state
        .runtime
        .dismiss_fork_proposal(&proposal_id)
        .await
        .map_err(fork_resolve_app_error)?;
    Ok(Json(ForkDismissResponse {
        success: true,
        no_op: !transitioned,
    }))
}

/// `POST /api/conversations/:id/proposals/:proposal_id/request-changes` —
/// promote the proposal to an Explore refinement (REQ-PROJ-037).
pub(crate) async fn request_changes_on_fork_proposal(
    State(state): State<AppState>,
    Path((id, proposal_id)): Path<(String, String)>,
    Json(req): Json<RequestChangesRequest>,
) -> Result<Json<ForkPromoteResponse>, AppError> {
    require_proposal_for_conversation(&state, &id, &proposal_id).await?;
    let refinement_conversation_id = state
        .runtime
        .request_changes_on_fork_proposal(&proposal_id, req.note)
        .await
        .map_err(fork_resolve_app_error)?;
    Ok(Json(ForkPromoteResponse {
        refinement_conversation_id,
    }))
}

/// `GET /api/conversations/:id/proposals` — list this conversation's fork
/// proposals so the UI can render / withdraw the Review affordance.
pub(crate) async fn list_fork_proposals(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ForkProposalListResponse>, AppError> {
    let rows = state
        .db
        .list_fork_proposals_for_origin(&id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let proposals = rows
        .into_iter()
        .map(|p| ForkProposalSummary {
            id: p.id,
            status: p.status.as_str().to_string(),
            title: p.title,
            priority: p.priority,
            task_file: p.task_file,
            body: p.body,
            fork_conversation_id: p.fork_conversation_id,
            refinement_conversation_id: p.refinement_conversation_id,
        })
        .collect();
    Ok(Json(ForkProposalListResponse { proposals }))
}

// ============================================================
// Task Abandon (REQ-PROJ-010)
// ============================================================

/// Abandon a Work or Branch conversation: delete worktree, optionally delete branch,
/// capture diff snapshot, transition to Terminal.
/// Single-phase endpoint -- the frontend confirms via a dialog before calling this.
#[allow(clippy::too_many_lines)]
/// Confirms destructive loss retirement against one exact persisted server
/// inspection. This contract deliberately accepts no client path or inventory.
pub(crate) async fn confirm_close_loss_retirement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ConfirmCloseLossRetirementRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    let attempt_id = phoenix_core::domain::close::CloseAttemptId::parse(request.attempt_id)
        .map_err(|error| AppError::TypedBadRequest {
            message: error.to_string(),
            error_type: "invalid_close_attempt".to_string(),
        })?;
    let snapshot = phoenix_core::domain::close::CloseRetirementSnapshot::parse(
        request.inspection_generation,
        request.inspection_fingerprint,
    )
    .map_err(|error| AppError::TypedBadRequest {
        message: error.to_string(),
        error_type: "invalid_close_snapshot".to_string(),
    })?;
    let attempt = state
        .db
        .get_close_obligation(attempt_id.as_str())
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let aggregate = state
        .db
        .get_ordinary_product_conversation(attempt.product_conversation_id())
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let active_transcript = aggregate
        .segments
        .last()
        .map(|segment| segment.transcript_row.conversation.id.as_str());
    if active_transcript != Some(id.as_str()) {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close confirmation is accepted only from the active aggregate transcript",
            "inactive_close_transcript",
        ))));
    }
    let fresh_snapshot = state
        .runtime
        .inspect_close_retirement_only(attempt_id.clone())
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error,
                "close_inspection_failed",
            )))
        })?;
    if fresh_snapshot == snapshot {
        state
            .db
            .confirm_close_loss_retirement(&attempt_id, &fresh_snapshot)
            .await
            .map_err(|error| {
                AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    error.to_string(),
                    "stale_close_inspection",
                )))
            })?;
    } else {
        let fresh_obligation = state
            .db
            .get_close_obligation(attempt_id.as_str())
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        if fresh_obligation.phase() != phoenix_core::domain::close::ClosePhase::RetirementRequested
        {
            return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "Close confirmation does not match fresh server inspection",
                "stale_close_inspection",
            ))));
        }
    }
    state
        .runtime
        .retire_close_runtime_resources(attempt_id)
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error,
                "close_retirement_needs_repair",
            )))
        })?;
    Ok(Json(SuccessResponse { success: true }))
}

pub(crate) async fn confirm_close_stop_work(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ConfirmCloseStopWorkRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let transcript = state
        .db
        .get_conversation(&id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let aggregate = state
        .db
        .get_ordinary_product_conversation(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    if aggregate
        .segments
        .last()
        .map(|segment| segment.transcript_row.conversation.id.as_str())
        != Some(id.as_str())
    {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Stop-work confirmation is accepted only from the active aggregate transcript",
            "inactive_close_transcript",
        ))));
    }
    let obligation = state
        .db
        .get_active_close_obligation_for_product(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
        .ok_or_else(|| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "No active Close attempt",
                "close_attempt_not_active",
            )))
        })?;
    if obligation.attempt_id().as_str() != request.attempt_id {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close attempt changed; refresh before confirming stop-work",
            "stale_close_attempt",
        ))));
    }
    state
        .db
        .begin_close_active_work_settlement(obligation.attempt_id().as_str())
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error.to_string(),
                "close_stop_work_confirmation_failed",
            )))
        })?;
    state
        .runtime
        .resume_pending_close_settlements()
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error,
                "close_settlement_in_progress",
            )))
        })?;
    let refreshed = state
        .db
        .get_close_obligation(obligation.attempt_id().as_str())
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    match refreshed.phase() {
        phoenix_core::domain::close::ClosePhase::SettlingActiveWork
        | phoenix_core::domain::close::ClosePhase::CancelRequestedDuringSettlement => {
            Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "Close settlement remains in progress",
                "close_settlement_in_progress",
            ))))
        }
        phoenix_core::domain::close::ClosePhase::AwaitingLossConfirmation => {
            Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "Close requires confirmation of the freshly inspected loss inventory",
                "close_loss_confirmation_required",
            ))))
        }
        _ => Ok(Json(SuccessResponse { success: true })),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn cancel_close_before_retirement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CancelCloseBeforeRetirementRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _guard = admission.lock().await;
    let transcript = state
        .db
        .get_conversation(&id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let aggregate = state
        .db
        .get_ordinary_product_conversation(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    if aggregate
        .segments
        .last()
        .map(|segment| segment.transcript_row.conversation.id.as_str())
        != Some(id.as_str())
    {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close cancellation is accepted only from the active aggregate transcript",
            "inactive_close_transcript",
        ))));
    }
    let obligation = state
        .db
        .get_active_close_obligation_for_product(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
        .ok_or_else(|| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                "No active Close attempt",
                "close_attempt_not_active",
            )))
        })?;
    if obligation.attempt_id().as_str() != request.attempt_id {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close attempt changed; refresh before cancelling",
            "stale_close_attempt",
        ))));
    }
    state
        .runtime
        .cancel_close_before_retirement(obligation.attempt_id())
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error.clone(),
                "close_cancel_failed",
            )))
        })?;
    Ok(Json(SuccessResponse { success: true }))
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn retry_close_retirement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RetryCloseRetirementRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    let transcript = state
        .db
        .get_conversation(&id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let aggregate = state
        .db
        .get_ordinary_product_conversation(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let active_transcript = aggregate
        .segments
        .last()
        .map(|segment| segment.transcript_row.conversation.id.as_str());
    if active_transcript != Some(id.as_str()) {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close retry is accepted only from the active aggregate transcript",
            "inactive_close_transcript",
        ))));
    }
    let attempt = state
        .db
        .get_active_close_obligation_for_product(&transcript.product_conversation_id)
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error.to_string(),
                "close_retry_unavailable",
            )))
        })?;
    let attempt = attempt.ok_or_else(|| {
        AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "No active Close attempt is available for retry",
            "close_retry_unavailable",
        )))
    })?;
    if attempt.attempt_id().as_str() != request.attempt_id {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close attempt changed; refresh before retrying",
            "stale_close_attempt",
        ))));
    }
    let retried = state
        .db
        .retry_close_retirement(attempt.attempt_id())
        .await
        .map_err(|error| {
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                error.to_string(),
                "close_retry_unavailable",
            )))
        })?;
    let retry_result = if retried.phase()
        == phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection
    {
        state
            .runtime
            .inspect_close_retirement(retried.attempt_id().clone())
            .await
            .map(|_| ())
    } else {
        state
            .runtime
            .retire_close_runtime_resources(retried.attempt_id().clone())
            .await
    };
    if let Err(error) = retry_result {
        let authoritative = state
            .db
            .get_close_obligation(retried.attempt_id().as_str())
            .await
            .map_err(|reload_error| AppError::Internal(reload_error.to_string()))?;
        if authoritative.phase() == phoenix_core::domain::close::ClosePhase::RetirementRequested {
            let scope = state
                .db
                .list_close_attempt_scopes(retried.attempt_id().as_str())
                .await
                .map_err(|route_error| AppError::Internal(route_error.to_string()))?
                .into_iter()
                .next()
                .ok_or_else(|| AppError::Internal("Close retry has no captured scope".to_string()))?
                .scope;
            state
                .runtime
                .route_close_attempt_to_repair::<()>(
                    retried.attempt_id(),
                    &scope,
                    phoenix_core::domain::close::RetirementFailureReason::ManualRepairRequired,
                    error.clone(),
                )
                .await
                .expect_err("repair routing returns the persisted repair detail");
        }
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            error,
            "close_retirement_needs_repair",
        ))));
    }
    let authoritative = state
        .db
        .get_close_obligation(retried.attempt_id().as_str())
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    if authoritative.phase() == phoenix_core::domain::close::ClosePhase::AwaitingLossConfirmation {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close requires confirmation of the freshly inspected loss inventory",
            "close_loss_confirmation_required",
        ))));
    }
    Ok(Json(SuccessResponse { success: true }))
}

#[allow(clippy::too_many_lines, clippy::single_match_else)]
async fn run_legacy_close_compat(state: &AppState, id: &str, action: &str) -> Result<(), AppError> {
    use phoenix_core::domain::close::{CloseAttemptId, ClosePhase};

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
            let attempt_id = CloseAttemptId::parse(uuid::Uuid::new_v4().to_string())
                .expect("UUID is a valid Close attempt id");
            state
                .db
                .begin_close_foundation(
                    &transcript.product_conversation_id,
                    &expected_latest_transcript,
                    attempt_id.as_str(),
                )
                .await
                .map_err(|error| {
                    AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        error.to_string(),
                        "close_start_failed",
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
                    .resume_pending_close_settlements()
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
                    let error_type = if current.phase() == ClosePhase::NeedsRepair {
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
                state
                    .runtime
                    .retire_close_runtime_resources(obligation.attempt_id().clone())
                    .await
                    .map_err(|error| {
                        AppError::Conflict(Box::new(ConflictErrorResponse::new(
                            error,
                            "close_retirement_needs_repair",
                        )))
                    })?;
                return Ok(());
            }
            ClosePhase::NeedsRepair => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Close retirement requires repair before it can continue",
                    "close_retirement_needs_repair",
                ))));
            }
            ClosePhase::Completed => return Ok(()),
        };
    }
}

pub(crate) async fn close_legacy_compat(
    state: &AppState,
    id: &str,
    action: &str,
) -> Result<(), AppError> {
    run_legacy_close_compat(state, id, action).await
}

pub(crate) async fn abandon_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _admission = admission.lock().await;
    run_legacy_close_compat(&state, &id, "abandon").await?;
    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Mark as Merged (REQ-PROJ-026)
// ============================================================

/// Mark a Work or Branch conversation as merged: delete worktree, optionally delete branch,
/// transition to Terminal. The user has already merged/PR'd the branch externally.
#[allow(clippy::too_many_lines)]
pub(crate) async fn mark_merged(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    let admission = state.runtime.conversation_admission(&id).await;
    let _admission = admission.lock().await;
    run_legacy_close_compat(&state, &id, "mark as merged").await?;
    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    //! REQ-BED-031 gate tests for the abandon and mark-as-merged handlers.
    //!
    //! The gate logic lives in [`reject_if_continued`], which both
    //! `abandon_task` and `mark_merged` invoke immediately after reading
    //! the conversation and before any worktree/branch destruction. The
    //! "state did NOT change on reject" property in REQ-BED-031 is
    //! therefore enforced structurally: the handler returns `Err` via
    //! `?` before reaching the `run_git` or `send_event` calls. These
    //! tests cover the gate itself; integration coverage of the full
    //! handler flow would require a full `AppState` harness that does
    //! not currently exist in the repo (Phase 2 tested `Database::
    //! continue_conversation` at the DB layer for the same reason).
    use super::*;
    use crate::db::{ConvMode, Conversation, NonEmptyString};
    use crate::state_machine::state::ConvState;
    use chrono::{TimeZone, Utc};

    fn fixture(id: &str, continued_in_conv_id: Option<String>) -> Conversation {
        let ts = Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap();
        Conversation {
            attached_work_scope_id: Some(
                crate::work_scope::WorkScopeId::parse("test-work").unwrap(),
            ),
            runtime_role: crate::work_scope::RuntimeRole::User,
            id: id.to_string(),
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::parse(id)
                    .unwrap(),
            slug: Some(format!("slug-{id}")),
            title: Some(format!("Title {id}")),
            cwd: "/tmp/work".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: ts,
            created_at: ts,
            updated_at: ts,
            archived: false,
            transcript_generation: 1,
            model: None,
            effort: None,
            service_tier: phoenix_core::domain::llm_types::ServiceTier::Standard,
            project_id: Some("proj-1".to_string()),
            conv_mode: ConvMode::Work {
                branch_name: NonEmptyString::new("task-24696-gate").unwrap(),
                worktree_path: NonEmptyString::new("/tmp/wt/gate").unwrap(),
                base_branch: NonEmptyString::new("main").unwrap(),
                task_id: NonEmptyString::new("TK24696").unwrap(),
                task_title: NonEmptyString::new("Gate test").unwrap(),
            },
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id,
            chain_name: None,
            llm_language: crate::llm_language::LlmLanguage::default(),
            spawned_from_conversation_id: None,
        }
    }

    #[tokio::test]
    async fn product_conversation_without_project_approves_from_allocated_work_scope() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let id = "product-work-scope-approval";
        state
            .db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .expect("conversation");
        state
            .db
            .update_conversation_mode_and_cwd(
                id,
                &ConvMode::Work {
                    branch_name: NonEmptyString::new("task-approval").unwrap(),
                    worktree_path: NonEmptyString::new("/tmp/task-approval").unwrap(),
                    base_branch: NonEmptyString::new("main").unwrap(),
                    task_id: NonEmptyString::new("12345").unwrap(),
                    task_title: NonEmptyString::new("approval").unwrap(),
                },
                "/tmp/task-approval",
            )
            .await
            .expect("allocate worktree environment");
        let conversation = state.db.get_conversation(id).await.expect("conversation");
        assert!(conversation.project_id.is_none());

        ensure_task_approval_authorized(&state, &conversation)
            .await
            .expect("active allocated WorkScope authorizes approval");
    }

    #[tokio::test]
    async fn approve_task_rejects_direct_unowned_cwd() {
        let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
        let id = "direct-unowned-approval";
        state
            .db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .expect("conversation");
        let conversation = state.db.get_conversation(id).await.expect("conversation");
        let error = ensure_task_approval_authorized(&state, &conversation)
            .await
            .expect_err("unowned cwd rejects approval");
        assert!(matches!(
            error,
            AppError::BadRequest(message)
                if message == "Task approval requires an active Git-backed WorkScope"
        ));
    }

    #[test]
    fn abandon_gate_passes_when_no_continuation() {
        let conv = fixture("parent-a", None);
        assert!(reject_if_continued(&conv, "abandon").is_ok());
    }

    /// Blocked: `continued_in_conv_id = Some(...)` — gate returns 409
    /// with `error_type = "continuation_exists"` and a message naming
    /// the continuation id. Structurally prevents the handler from
    /// reaching worktree/branch destruction (REQ-BED-031).
    #[test]
    fn abandon_gate_rejects_when_continuation_exists() {
        let conv = fixture("parent-a", Some("child-conv-id".to_string()));
        let err = reject_if_continued(&conv, "abandon")
            .expect_err("gate must reject when continued_in_conv_id is set");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "continuation_exists");
                assert_eq!(
                    detail.continuation_id.as_deref(),
                    Some("child-conv-id"),
                    "typed continuation_id must be populated so FE doesn't regex-parse the message",
                );
                assert!(
                    detail.error.contains("Cannot abandon"),
                    "error must name the action: {}",
                    detail.error
                );
                assert!(
                    detail.error.contains("child-conv-id"),
                    "error must include the continuation id for FE routing: {}",
                    detail.error
                );
            }
            _ => panic!("expected AppError::Conflict, got a different variant"),
        }
    }

    // ---- mark-as-merged gate ------------------------------------------

    /// Unblocked: `continued_in_conv_id = None` — gate passes, handler
    /// proceeds with existing mark-merged logic.
    #[test]
    fn mark_merged_gate_passes_when_no_continuation() {
        let conv = fixture("parent-m", None);
        assert!(reject_if_continued(&conv, "mark as merged").is_ok());
    }

    /// Blocked: `continued_in_conv_id = Some(...)` — gate returns 409
    /// with `error_type = "continuation_exists"` and a message naming
    /// the continuation id. Structurally prevents the handler from
    /// reaching worktree/branch destruction (REQ-BED-031).
    #[test]
    fn mark_merged_gate_rejects_when_continuation_exists() {
        let conv = fixture("parent-m", Some("child-conv-id".to_string()));
        let err = reject_if_continued(&conv, "mark as merged")
            .expect_err("gate must reject when continued_in_conv_id is set");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "continuation_exists");
                assert_eq!(
                    detail.continuation_id.as_deref(),
                    Some("child-conv-id"),
                    "typed continuation_id must be populated so FE doesn't regex-parse the message",
                );
                assert!(
                    detail.error.contains("Cannot mark as merged"),
                    "error must name the action: {}",
                    detail.error
                );
                assert!(
                    detail.error.contains("child-conv-id"),
                    "error must include the continuation id for FE routing: {}",
                    detail.error
                );
            }
            _ => panic!("expected AppError::Conflict, got a different variant"),
        }
    }

    #[test]
    fn terminal_action_gate_accepts_exactly_disposable_settled_states() {
        let legal_states = [
            ConvState::Idle,
            ConvState::Error {
                message: "usage window".into(),
                error_kind: crate::db::ErrorKind::UsageLimitReached,
                resets_at: None,
            },
            ConvState::RecoverableContinuationFailure {
                failure: crate::state_machine::state::RecoverableContinuationFailure {
                    request: crate::state_machine::state::ContinuationSummaryRequest {
                        operation_id: "op-1".into(),
                        rejected_tool_calls: vec![],
                        attempt: 2,
                    },
                    error_kind: crate::db::ErrorKind::ServerError,
                    message: "summary failed".into(),
                },
            },
            ConvState::ContextExhausted {
                summary: "continue from here".into(),
            },
        ];

        for state in legal_states {
            let mut conv = fixture("terminal-legal", None);
            conv.state = state;
            assert!(
                ensure_terminal_action_legal(&conv, "abandon a task").is_ok(),
                "expected state {} to permit terminal action",
                conv.state.variant_name()
            );
        }
    }

    #[test]
    fn terminal_action_gate_rejects_non_disposable_or_continued_states() {
        let illegal_states = [
            ConvState::LlmRequesting { attempt: 1 },
            ConvState::AwaitingContinuation {
                request: crate::state_machine::state::ContinuationSummaryRequest {
                    operation_id: "op-2".into(),
                    rejected_tool_calls: vec![],
                    attempt: 1,
                },
            },
            ConvState::HandedOff {
                successor_conv_id: "next".into(),
            },
        ];

        for state in illegal_states {
            let mut conv = fixture("terminal-illegal", None);
            conv.state = state;
            let err = ensure_terminal_action_legal(&conv, "mark as merged")
                .expect_err("non-disposable state must reject terminal action");
            match err {
                AppError::BadRequest(message) => assert!(
                    message.contains("Conversation must be idle, context-exhausted, or in a recoverable error state"),
                    "unexpected message: {message}"
                ),
                other => panic!("expected bad request, got {other:?}"),
            }
        }

        let mut conv = fixture("terminal-continued", Some("child-conv-id".to_string()));
        conv.state = ConvState::ContextExhausted {
            summary: "continued elsewhere".into(),
        };
        let err = ensure_terminal_action_legal(&conv, "mark as merged").expect_err(
            "continued conversation must reject terminal action even from ContextExhausted",
        );
        assert!(matches!(err, AppError::Conflict(_)));
    }
}
