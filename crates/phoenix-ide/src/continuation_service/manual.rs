use crate::api::handlers::{map_admission_db_error, map_ordinary_mutation_admission, AppError};
use crate::api::{ConflictErrorResponse, ContinueConversationResponse, ContinueConversationStatus};
use crate::continuation_service::ContinuationApplicationService;
use crate::db::{ContinueOutcome, DbError};
use crate::send_chat_service::{SendChatApplicationService, SendChatOutcome, SendChatRequest};
use crate::state_machine::ConvState;

use super::continuation_expansion_policy;

pub(crate) fn existing_continuation_status(
    dispatch_status: ContinueConversationStatus,
) -> ContinueConversationStatus {
    match dispatch_status {
        ContinueConversationStatus::Accepted | ContinueConversationStatus::AlreadyExists => {
            ContinueConversationStatus::AlreadyExists
        }
        ContinueConversationStatus::DispatchFailed => ContinueConversationStatus::DispatchFailed,
    }
}

pub(crate) fn continuation_message_id(
    message_id: String,
) -> Result<phoenix_workflow::ClientTurnKey, AppError> {
    phoenix_workflow::ClientTurnKey::new(message_id).ok_or_else(|| {
        AppError::BadRequest("Continuation message_id must not be empty.".to_string())
    })
}

pub(crate) fn automatic_retry_phase_for_turn_state(
    state: crate::send_chat_service::AutomaticRetryTurnState,
    reserved_phase: phoenix_core::domain::product_conversation::AutomaticContinuationPhase,
) -> Option<phoenix_core::domain::product_conversation::AutomaticContinuationPhase> {
    match state {
        crate::send_chat_service::AutomaticRetryTurnState::RearmedWithAdmission => None,
        crate::send_chat_service::AutomaticRetryTurnState::AlreadyAccepted => Some(
            phoenix_core::domain::product_conversation::AutomaticContinuationPhase::DispatchAccepted,
        ),
        crate::send_chat_service::AutomaticRetryTurnState::NoAcceptedTurn => Some(reserved_phase),
    }
}

pub(super) async fn dispatch_continuation_handoff(
    service: &ContinuationApplicationService,
    intent: crate::db::ContinuationDispatchIntent,
) -> (ContinueConversationStatus, Option<String>) {
    let parent_id = intent.parent_conversation_id.clone();
    let conversation_id = intent.successor_conversation_id.clone();
    let expansion_policy = continuation_expansion_policy(intent.opening_authority);
    let dispatch =
        SendChatApplicationService::new(service.runtime.db().clone(), service.runtime.clone())
            .send(SendChatRequest {
                conversation_id: conversation_id.clone(),
                origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
                text: intent.handoff,
                message_id: intent.message_id.as_str().to_string(),
                images: Vec::new(),
                files: Vec::new(),
                user_agent: intent.user_agent,
                expansion_policy,
            })
            .await;
    match dispatch {
        Ok(SendChatOutcome::Delivered) => {
            // Channel delivery is not durable acceptance. The outbox intent stays
            // pending until the message INSERT trigger consumes it, so a crash
            // before executor persistence remains replayable.
            (ContinueConversationStatus::Accepted, None)
        }
        Ok(SendChatOutcome::AlreadyPersisted) => {
            if let Err(error) = service
                .runtime
                .db()
                .delete_continuation_dispatch_intent(&parent_id)
                .await
            {
                tracing::warn!(
                    parent_id,
                    continuation_id = conversation_id,
                    error = %error,
                    "handoff is durable but continuation intent cleanup failed",
                );
            }
            (ContinueConversationStatus::Accepted, None)
        }
        Ok(SendChatOutcome::QueuedAsSteering) => {
            // The opening message is not persisted yet. Its INSERT trigger consumes
            // this intent and records the completed handoff atomically.
            (ContinueConversationStatus::Accepted, None)
        }
        Ok(SendChatOutcome::Rejected { message, .. }) => {
            (ContinueConversationStatus::DispatchFailed, Some(message))
        }
        Err(error) => {
            tracing::warn!(
                parent_id,
                continuation_id = conversation_id,
                error = %error,
                "continuation created but opening handoff dispatch failed",
            );
            (
                ContinueConversationStatus::DispatchFailed,
                Some(error.to_string()),
            )
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn continue_manual(
    service: &ContinuationApplicationService,
    parent_id: &str,
    req: crate::api::ContinueConversationRequest,
) -> Result<ContinueConversationResponse, AppError> {
    let id = parent_id;

    let parent = service
        .runtime
        .db()
        .get_conversation(id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    let _product_admission = service
        .runtime
        .lock_product_message_admission(parent.product_conversation_id.as_str())
        .await;

    let admission = service
        .runtime
        .mutation_admission(id)
        .await
        .map_err(map_admission_db_error)?;
    let _mutation_admission = admission.lock().await;
    let parent = service
        .runtime
        .db()
        .get_conversation(id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    if req.handoff.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Continuation handoff must not be empty.".to_string(),
        ));
    }
    let generated_context = if parent.continued_in_conv_id.is_some() {
        false
    } else {
        match &req.source {
            crate::api::ContinueHandoffSource::Manual => false,
            crate::api::ContinueHandoffSource::Candidate { candidate_id } => {
                let candidates = service
                    .runtime
                    .db()
                    .continuation_handoff_candidates(id)
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?;
                if !candidates.iter().any(|candidate| {
                    &candidate.id == candidate_id
                        && candidate.text == req.handoff
                        && candidate.origin.is_generated()
                        && candidate.origin.revision().is_current_for(&parent)
                }) {
                    return Err(AppError::BadRequest("Accepted candidate must exactly match a retained generated preview. Submit edits as a separate manual handoff.".to_string()));
                }
                true
            }
            crate::api::ContinueHandoffSource::Summary { message_id } => {
                let message = service
                    .runtime
                    .db()
                    .get_message_by_id_in_conversation(id, message_id)
                    .await
                    .map_err(|error| AppError::BadRequest(error.to_string()))?;
                if !matches!(&parent.state, ConvState::ContextExhausted { summary } if summary == &req.handoff)
                    || !matches!(message.content, crate::db::MessageContent::Continuation(content) if content.summary == req.handoff)
                {
                    return Err(AppError::BadRequest(
                        "Accepted generated summary must exactly match its transcript row"
                            .to_string(),
                    ));
                }
                true
            }
        }
    };
    let mut automatic_retry_phase = None;
    let _automatic_retry_authority = if req.retry_failed_automatic {
        Some(
            service
                .runtime
                .acquire_local_authority_pass()
                .map_err(|()| AppError::Internal("local authority is closed".to_string()))?,
        )
    } else {
        None
    };
    let failed_admission = if req.retry_failed_automatic {
        service
            .runtime
            .db()
            .automatic_continuation_admission(id)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
            .filter(|admission| {
                admission.phase
                    == phoenix_core::domain::product_conversation::AutomaticContinuationPhase::Failed
            })
    } else {
        None
    };
    let (message_id, handoff, user_agent, opening_authority) = if let Some(admission) =
        failed_admission
    {
        let summary = service
            .runtime
            .db()
            .get_message_by_id_in_conversation(id, &admission.summary_message_id)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
        let crate::db::MessageContent::Continuation(summary) = summary.content else {
            return Err(AppError::Internal(
                "automatic continuation summary has the wrong message type".to_string(),
            ));
        };
        let mut retry_phase = admission.resume_phase;
        if let Some(intent) = service
            .runtime
            .db()
            .continuation_dispatch_intent(id)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?
        {
            if retry_phase
                == phoenix_core::domain::product_conversation::AutomaticContinuationPhase::Admitted
            {
                retry_phase = phoenix_core::domain::product_conversation::AutomaticContinuationPhase::SuccessorReserved;
            }
            let (handoff, expansion_policy) = match intent.opening_authority {
                phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext => (
                    summary.summary.as_str(),
                    crate::send_chat_service::MessageExpansionPolicy::GeneratedPredecessorContext,
                ),
                phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::UserAuthorizedInstruction => (
                    intent.handoff.as_str(),
                    crate::send_chat_service::MessageExpansionPolicy::LiteralText,
                ),
            };
            let service = crate::send_chat_service::SendChatApplicationService::new(
                service.runtime.db().clone(),
                service.runtime.clone(),
            );
            let turn_state = service
                .rearm_exact_terminal_turn(
                    id,
                    &intent.successor_conversation_id,
                    &intent.message_id,
                    handoff,
                    intent.user_agent.clone(),
                    expansion_policy,
                )
                .await
                .map_err(|error| AppError::Internal(error.to_string()))?;
            automatic_retry_phase = automatic_retry_phase_for_turn_state(turn_state, retry_phase);
        } else {
            automatic_retry_phase = Some(retry_phase);
        }
        (
            admission.first_message_id,
            summary.summary,
            Some("phoenix-automatic-continuation".to_string()),
            phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext,
        )
    } else {
        (
            continuation_message_id(req.message_id.clone())?,
            req.handoff,
            req.user_agent,
            if generated_context {
                phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::GeneratedPredecessorContext
            } else {
                phoenix_core::domain::product_conversation::ContinuationOpeningAuthority::UserAuthorizedInstruction
            },
        )
    };

    if parent.continued_in_conv_id.is_none() {
        let admission = service
            .runtime
            .db()
            .message_target_admission(id)
            .await
            .map_err(map_admission_db_error)?;
        map_ordinary_mutation_admission(&admission, "continuation admission")?;
        let review = service
            .runtime
            .review_continuation_admission(id, handoff.clone(), opening_authority)
            .await
            .map_err(AppError::BadRequest)?;
        if !review.source.fits() {
            let message = match review.source.status {
                crate::runtime::HandoffCapacityStatus::RequiredOverhead => "Required instructions/tools and full output reservation leave no successor working room. Handoff editing and shortening cannot fix this overhead; explicitly change model or revise required instructions/tools.",
                crate::runtime::HandoffCapacityStatus::HandoffTooLarge { .. } => "The supplied handoff leaves insufficient successor working room. Review the capacity target, edit the handoff, request one shortening candidate, or explicitly change model.",
                crate::runtime::HandoffCapacityStatus::Ready { .. } => unreachable!("fit checked above"),
            };
            let mut conflict =
                ConflictErrorResponse::new(message, "continuation_capacity_conflict");
            conflict.capacity = Some(review.source);
            return Err(AppError::Conflict(Box::new(conflict)));
        }
    }

    let (outcome, intent) = service
        .runtime
        .db()
        .continue_conversation_with_intent(
            id,
            crate::db::NewContinuationDispatchIntent {
                message_id,
                handoff,
                user_agent,
                opening_authority,
            },
        )
        .await
        .map_err(|e| match e {
            DbError::ConversationNotFound(msg) => AppError::NotFound(msg),
            DbError::ContinuationPrecondition(msg) => AppError::Conflict(Box::new(
                ConflictErrorResponse::new(msg, "continuation_not_allowed"),
            )),
            DbError::CloseAdmissionFenced(_) => AppError::Conflict(Box::new(
                ConflictErrorResponse::new(
                    "continuation is not allowed while the ProductConversation has an active Close obligation",
                    "continuation_not_allowed",
                ),
            )),
            DbError::ProductConversationUnavailable(product_conversation_id) => {
                AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    format!(
                        "ProductConversation {product_conversation_id} is unavailable because it is in History"
                    ),
                    "continuation_not_allowed",
                )))
            }
            other @ (DbError::ReminderInvalid(_)
        | DbError::ReminderRequestConflict
        | DbError::CoordinatorWorkScope(_)
            | DbError::Sqlx(_)
            | DbError::ConversationTitleInvalid
            | DbError::CurrentCoordinatorRequired
            | DbError::ProductCreationIntentInvalid(_)
            | DbError::ConversationAlreadyExists(_)
            | DbError::MessageNotFound(_)
            | DbError::MessageConflict(_)
            | DbError::SlugExists(_)
            | DbError::TranscriptSnapshotChanged
            | DbError::Serialization(_)
            | DbError::CloseFoundationConflict(_)
            | DbError::SteeringQueueFull
            | DbError::ProductCreationClaimLost
            | DbError::CloseFoundationPrecondition(_)
            | DbError::CloseFoundationStaleLatest { .. }
            | DbError::CloseFoundationRepairRequired(_)
            | DbError::CloseFoundationNotFound(_)
            | DbError::SubAgentLifecycleConflict(_)
            | DbError::DirectTurnConflict(_)
            | DbError::FederationSendAuthorityInactive
            | DbError::FederationSendIdentityConflict
            | DbError::FederationPeerNotFound
            | DbError::FederationPeerAmbiguous
            | DbError::FederationSteeringDuplicate
            | DbError::ForkProposalConflict(_)
            | DbError::GitRepositoryWorkScopeProjectConflict { .. }
            | DbError::DormantGitRepositoryCatchupPermitTargetMismatch
            | DbError::DormantGitRepositoryCatchupStaleOperation
            | DbError::DormantGitRepositoryCatchupBlockedByReadinessClaim
            | DbError::DormantGitRepositoryReadinessCatchupInProgress
            | DbError::DormantGitRepositoryReadinessReceiptTargetMismatch
            | DbError::DormantGitRepositoryReadinessReceiptOperationMismatch) => {
                AppError::Internal(other.to_string())
            }
        })?;

    if let Some(retry_phase) = automatic_retry_phase {
        service
            .runtime
            .db()
            .retry_failed_automatic_continuation(id, retry_phase)
            .await
            .map_err(|error| AppError::Internal(error.to_string()))?;
    }

    match outcome {
        ContinueOutcome::Created(new_conv) => {
            tracing::info!(
                parent_id = %id,
                continuation_id = %new_conv.id,
                mode = new_conv.conv_mode.label(),
                "continuation created",
            );
            // Spawn runtime for the new conversation so SSE subscribers can
            // immediately find it. Fire-and-forget: the DB transaction is
            // the load-bearing side; a spawn failure does not roll the
            // conversation back (the handler's contract is the DB write
            // succeeded, not that the runtime is up).
            let conv_id = new_conv.id.clone();
            let runtime = service.runtime.clone();
            tokio::spawn(async move {
                if let Err(e) = runtime.get_or_create(&conv_id).await {
                    tracing::warn!(
                        conv_id = %conv_id,
                        error = %e,
                        "failed to spawn runtime for continuation (SSE subscribers will retry)",
                    );
                }
            });

            let conversation_id = new_conv.id;
            let slug = new_conv.slug;
            let Some(intent) = intent else {
                return Err(AppError::Internal(
                    "continuation created without a dispatch intent".to_string(),
                ));
            };
            let (status, error) = dispatch_continuation_handoff(service, intent).await;

            Ok(ContinueConversationResponse {
                conversation_id,
                slug,
                status,
                error,
            })
        }
        ContinueOutcome::AlreadyContinued(existing) => {
            tracing::info!(
                parent_id = %id,
                existing_continuation = %existing.id,
                "continuation already existed; returning existing id idempotently",
            );
            if let Some(intent) = intent {
                let (dispatch_status, error) = dispatch_continuation_handoff(service, intent).await;
                // The persisted earlier intent won the race. Report the existing
                // successor so a losing editor keeps its draft.
                let status = existing_continuation_status(dispatch_status);
                return Ok(ContinueConversationResponse {
                    conversation_id: existing.id,
                    slug: existing.slug,
                    status,
                    error,
                });
            }
            Ok(ContinueConversationResponse {
                conversation_id: existing.id,
                slug: existing.slug,
                status: ContinueConversationStatus::AlreadyExists,
                error: None,
            })
        }
        ContinueOutcome::ParentNotContextExhausted { state_variant } => {
            tracing::debug!(
                parent_id = %id,
                state = state_variant,
                "continuation rejected: parent is not context-exhausted",
            );
            Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                format!(
                    "Conversation is not in context-exhausted state (current: {state_variant}); \
                     only context-exhausted conversations can be continued."
                ),
                "parent_not_context_exhausted",
            ))))
        }
    }
}
