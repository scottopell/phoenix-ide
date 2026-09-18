use std::sync::Arc;

use phoenix_core::domain::db_schema::MessageContent;
use phoenix_core::domain::product_conversation::{
    AutomaticContinuationPhase, ContinuationOpeningAuthority,
};
use phoenix_db::{AutomaticContinuationAdmission, ContinueOutcome, NewContinuationDispatchIntent};
use tracing::{debug, warn};

use crate::runtime::RuntimeManager;
use crate::send_chat_service::{
    MessageExpansionPolicy, SendChatApplicationService, SendChatOutcome, SendChatRequest,
};

fn winning_intent_is_automatic(
    admission: &AutomaticContinuationAdmission,
    intent: &crate::db::ContinuationDispatchIntent,
) -> bool {
    intent.message_id == admission.first_message_id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecoveryPlan {
    reserve_successor: bool,
    transfer_ownership: bool,
    dispatch_opening: bool,
}

fn recovery_plan(phase: AutomaticContinuationPhase) -> Option<RecoveryPlan> {
    Some(match phase {
        AutomaticContinuationPhase::Admitted => RecoveryPlan {
            reserve_successor: true,
            transfer_ownership: true,
            dispatch_opening: true,
        },
        AutomaticContinuationPhase::SuccessorReserved => RecoveryPlan {
            reserve_successor: false,
            transfer_ownership: true,
            dispatch_opening: true,
        },
        AutomaticContinuationPhase::OwnershipTransferred
        | AutomaticContinuationPhase::DispatchAccepted => RecoveryPlan {
            reserve_successor: false,
            transfer_ownership: false,
            dispatch_opening: true,
        },
        AutomaticContinuationPhase::MessageSettled
        | AutomaticContinuationPhase::Superseded
        | AutomaticContinuationPhase::Failed => {
            return None;
        }
    })
}

#[derive(Clone)]
pub(crate) struct ContinuationApplicationService {
    runtime: Arc<RuntimeManager>,
}

impl ContinuationApplicationService {
    #[must_use]
    pub(crate) fn new(runtime: Arc<RuntimeManager>) -> Self {
        Self { runtime }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn drive_admission(
        &self,
        admission: &AutomaticContinuationAdmission,
    ) -> Result<(), String> {
        if admission.opening_authority != ContinuationOpeningAuthority::GeneratedPredecessorContext
        {
            return Err("automatic admission lacks generated-context authority".to_string());
        }
        if self
            .runtime
            .db()
            .has_settled_automatic_continuation(admission)
            .await
            .map_err(|error| error.to_string())?
        {
            return self
                .advance_until(admission, AutomaticContinuationPhase::MessageSettled)
                .await;
        }

        if self
            .runtime
            .db()
            .has_completed_continuation_handoff(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
        {
            self.runtime
                .db()
                .supersede_automatic_continuation(&admission.predecessor_conversation_id)
                .await
                .map_err(|error| error.to_string())?;
            return Ok(());
        }

        let current = self.current_admission(admission).await?;
        let plan = recovery_plan(current.phase)
            .ok_or_else(|| "automatic admission is not progressable".to_string())?;
        if plan.reserve_successor {
            let summary = self
                .runtime
                .db()
                .get_message_by_id_in_conversation(
                    &admission.predecessor_conversation_id,
                    &admission.summary_message_id,
                )
                .await
                .map_err(|error| error.to_string())?;
            let MessageContent::Continuation(summary_content) = summary.content else {
                return Err("automatic continuation summary has the wrong message type".to_string());
            };
            match self
                .runtime
                .db()
                .continue_conversation_with_intent(
                    &admission.predecessor_conversation_id,
                    NewContinuationDispatchIntent::generated_predecessor_context(
                        admission.first_message_id.clone(),
                        summary_content.summary,
                    ),
                )
                .await
                .map_err(|error| error.to_string())?
                .0
            {
                ContinueOutcome::Created(_) | ContinueOutcome::AlreadyContinued(_) => {}
                ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                    return Err(format!("predecessor state changed to {state_variant}"));
                }
            }
            self.advance_current(
                &admission.predecessor_conversation_id,
                AutomaticContinuationPhase::SuccessorReserved,
            )
            .await?;
        }

        let intent = self
            .runtime
            .db()
            .continuation_dispatch_intent(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "automatic continuation dispatch intent is missing".to_string())?;
        let automatic_intent = winning_intent_is_automatic(admission, &intent);
        let successor = self
            .runtime
            .db()
            .get_conversation(&intent.successor_conversation_id)
            .await
            .map_err(|error| error.to_string())?;

        if plan.transfer_ownership {
            let parent = self
                .runtime
                .db()
                .get_conversation(&admission.predecessor_conversation_id)
                .await
                .map_err(|error| error.to_string())?;
            if parent.attached_work_scope_id == successor.attached_work_scope_id {
                crate::runtime::wake::transfer_active_for_continuation(
                    &self.runtime,
                    &admission.predecessor_conversation_id,
                    &successor.id,
                    phoenix_workflow::Timestamp(
                        u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default(),
                    ),
                )
                .await
                .map_err(|error| error.to_string())?;
            }
            if automatic_intent {
                self.advance_current(
                    &admission.predecessor_conversation_id,
                    AutomaticContinuationPhase::OwnershipTransferred,
                )
                .await?;
            }
        }

        if !automatic_intent {
            self.runtime
                .get_or_create(&successor.id)
                .await
                .map_err(|error| error.clone())?;
            let outcome =
                SendChatApplicationService::new(self.runtime.db().clone(), self.runtime.clone())
                    .send(SendChatRequest {
                        conversation_id: successor.id,
                        text: intent.handoff,
                        message_id: intent.message_id.as_str().to_string(),
                        images: Vec::new(),
                        files: Vec::new(),
                        user_agent: intent.user_agent,
                        expansion_policy: MessageExpansionPolicy::LiteralText,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
            match outcome {
                SendChatOutcome::Delivered
                | SendChatOutcome::AlreadyPersisted
                | SendChatOutcome::QueuedAsSteering => {
                    self.runtime
                        .db()
                        .supersede_automatic_continuation(&admission.predecessor_conversation_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Ok(());
                }
                SendChatOutcome::Rejected { message, .. } => return Err(message),
            }
        }

        if plan.dispatch_opening {
            self.runtime
                .get_or_create(&successor.id)
                .await
                .map_err(|error| error.clone())?;
            let expansion_policy = match intent.opening_authority {
                ContinuationOpeningAuthority::GeneratedPredecessorContext => {
                    MessageExpansionPolicy::GeneratedPredecessorContext
                }
                ContinuationOpeningAuthority::UserAuthorizedInstruction => {
                    MessageExpansionPolicy::ExpandReferences
                }
            };
            let outcome =
                SendChatApplicationService::new(self.runtime.db().clone(), self.runtime.clone())
                    .send(SendChatRequest {
                        conversation_id: successor.id,
                        text: intent.handoff,
                        message_id: intent.message_id.as_str().to_string(),
                        images: Vec::new(),
                        files: Vec::new(),
                        user_agent: intent.user_agent,
                        expansion_policy,
                    })
                    .await
                    .map_err(|error| error.to_string())?;
            match outcome {
                SendChatOutcome::Delivered
                | SendChatOutcome::AlreadyPersisted
                | SendChatOutcome::QueuedAsSteering => {
                    if current.phase != AutomaticContinuationPhase::DispatchAccepted {
                        self.advance_current(
                            &admission.predecessor_conversation_id,
                            AutomaticContinuationPhase::DispatchAccepted,
                        )
                        .await?;
                    }
                }
                SendChatOutcome::Rejected { message, .. } => return Err(message),
            }
        }
        Ok(())
    }

    async fn current_admission(
        &self,
        admission: &AutomaticContinuationAdmission,
    ) -> Result<AutomaticContinuationAdmission, String> {
        self.runtime
            .db()
            .automatic_continuation_admission(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "automatic continuation admission is missing".to_string())
    }

    async fn advance_until(
        &self,
        admission: &AutomaticContinuationAdmission,
        target: AutomaticContinuationPhase,
    ) -> Result<(), String> {
        let current = self
            .runtime
            .db()
            .automatic_continuation_admission(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "automatic continuation admission is missing".to_string())?;
        let phases = [
            AutomaticContinuationPhase::Admitted,
            AutomaticContinuationPhase::SuccessorReserved,
            AutomaticContinuationPhase::OwnershipTransferred,
            AutomaticContinuationPhase::DispatchAccepted,
            AutomaticContinuationPhase::MessageSettled,
        ];
        let current_index = phases
            .iter()
            .position(|phase| *phase == current.phase)
            .ok_or_else(|| "automatic admission is not progressable".to_string())?;
        let target_index = phases
            .iter()
            .position(|phase| *phase == target)
            .ok_or_else(|| "automatic target phase is not progressable".to_string())?;
        for phase in phases.iter().take(target_index + 1).skip(current_index + 1) {
            self.advance_current(&admission.predecessor_conversation_id, *phase)
                .await?;
        }
        Ok(())
    }

    async fn advance_current(
        &self,
        predecessor_id: &str,
        target: AutomaticContinuationPhase,
    ) -> Result<(), String> {
        self.runtime
            .db()
            .advance_automatic_continuation(predecessor_id, target)
            .await
            .map_err(|error| error.to_string())
    }
}

pub(crate) async fn drain_automatic_continuations(runtime: Arc<RuntimeManager>) {
    let admissions = match runtime
        .db()
        .pending_automatic_continuation_admissions()
        .await
    {
        Ok(admissions) => admissions,
        Err(error) => {
            warn!(%error, "failed to discover automatic continuation admissions");
            return;
        }
    };
    let service = ContinuationApplicationService::new(runtime.clone());
    for admission in admissions {
        match service.drive_admission(&admission).await {
            Ok(()) => {
                debug!(predecessor = %admission.predecessor_conversation_id, "automatic continuation progressed");
            }
            Err(error) => {
                let current = runtime
                    .db()
                    .automatic_continuation_admission(&admission.predecessor_conversation_id)
                    .await;
                match current {
                    Ok(Some(current)) if current.phase != admission.phase => {
                        warn!(
                            predecessor = %admission.predecessor_conversation_id,
                            phase = current.phase.as_str(),
                            %error,
                            "automatic continuation advanced before a later step failed"
                        );
                    }
                    Ok(Some(_)) => {
                        warn!(predecessor = %admission.predecessor_conversation_id, %error, "automatic continuation made no durable progress");
                        if let Err(record_error) = runtime
                            .db()
                            .reconcile_or_record_automatic_continuation_no_progress(
                                &admission, &error,
                            )
                            .await
                        {
                            warn!(%record_error, "failed to persist automatic continuation failure");
                        }
                    }
                    Ok(None) => warn!(
                        predecessor = %admission.predecessor_conversation_id,
                        %error,
                        "automatic continuation admission disappeared after failure"
                    ),
                    Err(record_error) => {
                        warn!(%record_error, "failed to inspect automatic continuation progress");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_manual_loser_with_automatic_message_identity_does_not_supersede() {
        let admission = AutomaticContinuationAdmission {
            predecessor_conversation_id: "parent".to_string(),
            product_conversation_id:
                phoenix_core::domain::product_conversation::ProductConversationId::new(),
            summary_message_id: "summary".to_string(),
            operation_id: "operation".to_string(),
            first_message_id: phoenix_workflow::ClientTurnKey::try_from("automatic-opening")
                .unwrap(),
            opening_authority: ContinuationOpeningAuthority::GeneratedPredecessorContext,
            phase: AutomaticContinuationPhase::Admitted,
            resume_phase: AutomaticContinuationPhase::Admitted,
            no_progress_attempts: 0,
            last_error: None,
            admitted_at_unix_micros: 1,
            updated_at_unix_micros: 1,
        };
        let intent = crate::db::ContinuationDispatchIntent {
            parent_conversation_id: "parent".to_string(),
            successor_conversation_id: "successor".to_string(),
            message_id: admission.first_message_id.clone(),
            handoff: "summary".to_string(),
            user_agent: None,
            opening_authority: ContinuationOpeningAuthority::UserAuthorizedInstruction,
        };

        assert!(winning_intent_is_automatic(&admission, &intent));
    }

    #[test]
    fn recovery_plan_resumes_after_each_durable_phase_without_repeating_it() {
        assert_eq!(
            recovery_plan(AutomaticContinuationPhase::Admitted),
            Some(RecoveryPlan {
                reserve_successor: true,
                transfer_ownership: true,
                dispatch_opening: true,
            })
        );
        assert_eq!(
            recovery_plan(AutomaticContinuationPhase::SuccessorReserved),
            Some(RecoveryPlan {
                reserve_successor: false,
                transfer_ownership: true,
                dispatch_opening: true,
            })
        );
        assert_eq!(
            recovery_plan(AutomaticContinuationPhase::OwnershipTransferred),
            Some(RecoveryPlan {
                reserve_successor: false,
                transfer_ownership: false,
                dispatch_opening: true,
            })
        );
        assert_eq!(
            recovery_plan(AutomaticContinuationPhase::DispatchAccepted),
            Some(RecoveryPlan {
                reserve_successor: false,
                transfer_ownership: false,
                dispatch_opening: true,
            })
        );
        assert_eq!(
            recovery_plan(AutomaticContinuationPhase::MessageSettled),
            None
        );
        assert_eq!(recovery_plan(AutomaticContinuationPhase::Superseded), None);
        assert_eq!(recovery_plan(AutomaticContinuationPhase::Failed), None);
    }
}
