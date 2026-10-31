use crate::api::handlers::AppError;
use crate::api::product_conversations::db_to_app;
use crate::api::{
    AutomaticContinuationAdmissionPhaseView, AutomaticContinuationAdmissionView,
    AutomaticContinuationAggregateView, AutomaticContinuationFailureView,
    AutomaticContinuationView,
};
use crate::db::Database;
use phoenix_core::domain::db_schema::MessageContent;
use phoenix_core::domain::product_conversation::{
    AutoContinueOnContextExhaustion, AutomaticContinuationPhase, ProductConversationId,
};

pub(crate) mod native;

#[derive(Debug)]
pub(crate) enum Target {
    Ordinary(ProductConversationId),
    Coordinator,
}

#[derive(Debug)]
enum ResolvedTarget {
    Ordinary(ProductConversationId),
    Coordinator(ProductConversationId),
}

impl ResolvedTarget {
    fn product_conversation_id(&self) -> &ProductConversationId {
        match self {
            Self::Ordinary(id) | Self::Coordinator(id) => id,
        }
    }
}

async fn resolve(db: &Database, target: Target) -> Result<ResolvedTarget, AppError> {
    match target {
        Target::Ordinary(product_conversation_id) => {
            db.get_ordinary_product_conversation(&product_conversation_id)
                .await
                .map_err(db_to_app)?;
            Ok(ResolvedTarget::Ordinary(product_conversation_id))
        }
        Target::Coordinator => {
            let coordinator_id = db
                .coordinator_conversation_id()
                .await
                .map_err(db_to_app)?
                .ok_or_else(|| {
                    AppError::NotFound("Coordinator has not been created".to_string())
                })?;
            let coordinator = db
                .get_conversation(&coordinator_id)
                .await
                .map_err(db_to_app)?;
            Ok(ResolvedTarget::Coordinator(
                coordinator.product_conversation_id,
            ))
        }
    }
}

pub(crate) async fn read(
    db: &Database,
    target: Target,
) -> Result<AutomaticContinuationView, AppError> {
    let target = resolve(db, target).await?;
    automatic_continuation_view(db, target).await
}

pub(crate) async fn set(
    db: &Database,
    target: Target,
    preference: AutoContinueOnContextExhaustion,
) -> Result<AutomaticContinuationView, AppError> {
    let target = resolve(db, target).await?;
    db.set_auto_continue_on_context_exhaustion(target.product_conversation_id(), preference)
        .await
        .map_err(db_to_app)?;
    automatic_continuation_view(db, target).await
}

async fn automatic_continuation_view(
    db: &Database,
    target: ResolvedTarget,
) -> Result<AutomaticContinuationView, AppError> {
    let product_conversation_id = target.product_conversation_id();
    let preference = db
        .auto_continue_on_context_exhaustion(product_conversation_id)
        .await
        .map_err(db_to_app)?;
    let admission = db
        .latest_automatic_continuation_admission(product_conversation_id)
        .await
        .map_err(db_to_app)?;
    let admission = if let Some(admission) = admission {
        let actionable_failure = if let Some(message) = admission.last_error {
            let summary = db
                .get_message_by_id_in_conversation(
                    &admission.predecessor_conversation_id,
                    &admission.summary_message_id,
                )
                .await
                .map_err(db_to_app)?;
            let MessageContent::Continuation(summary) = summary.content else {
                return Err(AppError::Internal(
                    "automatic continuation summary has the wrong message type".to_string(),
                ));
            };
            let persisted_winner = db
                .continuation_dispatch_intent(&admission.predecessor_conversation_id)
                .await
                .map_err(db_to_app)?;
            let (first_message_id, accepted_handoff, opening_authority) = persisted_winner
                .map(|intent| {
                    (
                        intent.message_id.as_str().to_string(),
                        intent.handoff,
                        intent.opening_authority,
                    )
                })
                .unwrap_or_else(|| {
                    (
                        admission.first_message_id.as_str().to_string(),
                        summary.summary,
                        admission.opening_authority,
                    )
                });
            Some(AutomaticContinuationFailureView {
                message,
                first_message_id,
                accepted_handoff,
                opening_authority,
            })
        } else {
            None
        };
        Some(AutomaticContinuationAdmissionView {
            predecessor_transcript_row_id: admission.predecessor_conversation_id,
            phase: admission_phase_view(admission.phase),
            no_progress_attempts: admission.no_progress_attempts,
            actionable_failure,
        })
    } else {
        None
    };
    let id = product_conversation_id.to_string();
    Ok(AutomaticContinuationView {
        aggregate: match target {
            ResolvedTarget::Coordinator(_) => AutomaticContinuationAggregateView::Coordinator {
                product_conversation_id: id,
            },
            ResolvedTarget::Ordinary(_) => AutomaticContinuationAggregateView::Ordinary {
                product_conversation_id: id,
            },
        },
        auto_continue_on_context_exhaustion: preference.is_enabled(),
        admission,
    })
}

fn admission_phase_view(
    phase: AutomaticContinuationPhase,
) -> AutomaticContinuationAdmissionPhaseView {
    match phase {
        AutomaticContinuationPhase::Admitted => AutomaticContinuationAdmissionPhaseView::Admitted,
        AutomaticContinuationPhase::SuccessorReserved => {
            AutomaticContinuationAdmissionPhaseView::SuccessorReserved
        }
        AutomaticContinuationPhase::OwnershipTransferred => {
            AutomaticContinuationAdmissionPhaseView::OwnershipTransferred
        }
        AutomaticContinuationPhase::DispatchAccepted => {
            AutomaticContinuationAdmissionPhaseView::DispatchAccepted
        }
        AutomaticContinuationPhase::MessageSettled => {
            AutomaticContinuationAdmissionPhaseView::MessageSettled
        }
        AutomaticContinuationPhase::Superseded => {
            AutomaticContinuationAdmissionPhaseView::Superseded
        }
        AutomaticContinuationPhase::Failed => AutomaticContinuationAdmissionPhaseView::Failed,
    }
}
