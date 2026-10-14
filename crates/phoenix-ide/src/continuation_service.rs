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

fn continuation_expansion_policy(
    authority: ContinuationOpeningAuthority,
) -> MessageExpansionPolicy {
    match authority {
        ContinuationOpeningAuthority::GeneratedPredecessorContext => {
            MessageExpansionPolicy::GeneratedPredecessorContext
        }
        ContinuationOpeningAuthority::UserAuthorizedInstruction => {
            MessageExpansionPolicy::LiteralText
        }
    }
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

#[derive(Debug, Clone)]
pub(crate) struct OwnerContinuationRequest {
    pub destination_product_conversation_id:
        phoenix_core::domain::product_conversation::ProductConversationId,
    pub predecessor_conversation_id: String,
    pub expected_work_scope_id: Option<phoenix_core::work_scope::WorkScopeId>,
    pub message_id: phoenix_workflow::ClientTurnKey,
    pub handoff: String,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerContinuationIntentAdmission {
    Created,
    ExactRetry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContinuationDispatchStatus {
    Accepted,
    AlreadySettled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerContinuationResult {
    pub successor_conversation_id: String,
    pub successor_slug: Option<String>,
    pub intent_admission: OwnerContinuationIntentAdmission,
    pub dispatch: ContinuationDispatchStatus,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OwnerContinuationError {
    #[error("continuation handoff must not be empty")]
    EmptyHandoff,
    #[error("the persisted continuation intent differs from this request")]
    IntentConflict,
    #[error("continuation created without a dispatch intent")]
    MissingIntent,
    #[error("continuation predecessor is not context-exhausted: {0}")]
    PredecessorNotContextExhausted(String),
    #[error(transparent)]
    Database(#[from] phoenix_db::DbError),
}

fn settled_owner_intent_matches(
    receipt: &phoenix_db::CompletedContinuationHandoff,
    successor_id: &str,
    requested: &NewContinuationDispatchIntent,
) -> bool {
    let direct_message_id = requested.message_id.as_str();
    let prefixed_message_id = format!("{successor_id}:{direct_message_id}");
    receipt.successor_conversation_id == successor_id
        && (receipt.accepted_successor_message_id == direct_message_id
            || receipt.accepted_successor_message_id == prefixed_message_id)
        && receipt.opening_authority == requested.opening_authority
        && receipt.handoff == requested.handoff
        && receipt.user_agent == requested.user_agent
}

fn owner_intent_matches(
    persisted: &crate::db::ContinuationDispatchIntent,
    requested: &NewContinuationDispatchIntent,
) -> bool {
    persisted.message_id == requested.message_id
        && persisted.handoff == requested.handoff
        && persisted.user_agent == requested.user_agent
        && persisted.opening_authority == requested.opening_authority
}

impl ContinuationApplicationService {
    #[must_use]
    pub(crate) fn new(runtime: Arc<RuntimeManager>) -> Self {
        Self { runtime }
    }

    pub(crate) async fn continue_owner(
        &self,
        request: OwnerContinuationRequest,
    ) -> Result<OwnerContinuationResult, OwnerContinuationError> {
        if request.handoff.trim().is_empty() {
            return Err(OwnerContinuationError::EmptyHandoff);
        }
        let _product_admission = self
            .runtime
            .lock_product_message_admission(request.destination_product_conversation_id.as_str())
            .await;
        let requested_intent = NewContinuationDispatchIntent::user_authorized(
            request.message_id,
            request.handoff,
            request.user_agent,
        );
        let (outcome, persisted_intent) = self
            .runtime
            .db()
            .continue_conversation_with_intent_checked(
                &request.predecessor_conversation_id,
                requested_intent.clone(),
                phoenix_db::ContinuationExpectation::new(
                    request.destination_product_conversation_id,
                    request.expected_work_scope_id,
                ),
            )
            .await?;

        match outcome {
            ContinueOutcome::Created(successor) => {
                let intent = persisted_intent.ok_or(OwnerContinuationError::MissingIntent)?;
                if !owner_intent_matches(&intent, &requested_intent) {
                    return Err(OwnerContinuationError::IntentConflict);
                }
                self.spawn_successor_runtime(successor.id.clone());
                let dispatch = self.dispatch_intent(intent).await;
                Ok(OwnerContinuationResult {
                    successor_conversation_id: successor.id,
                    successor_slug: successor.slug,
                    intent_admission: OwnerContinuationIntentAdmission::Created,
                    dispatch,
                })
            }
            ContinueOutcome::AlreadyContinued(successor) => {
                let dispatch = if let Some(intent) = persisted_intent {
                    if !owner_intent_matches(&intent, &requested_intent) {
                        return Err(OwnerContinuationError::IntentConflict);
                    }
                    self.dispatch_intent(intent).await
                } else {
                    self.require_settled_intent_matches(
                        &request.predecessor_conversation_id,
                        &successor.id,
                        &requested_intent,
                    )
                    .await?;
                    ContinuationDispatchStatus::AlreadySettled
                };
                Ok(OwnerContinuationResult {
                    successor_conversation_id: successor.id,
                    successor_slug: successor.slug,
                    intent_admission: OwnerContinuationIntentAdmission::ExactRetry,
                    dispatch,
                })
            }
            ContinueOutcome::ParentNotContextExhausted { state_variant } => Err(
                OwnerContinuationError::PredecessorNotContextExhausted(state_variant.to_string()),
            ),
        }
    }

    fn spawn_successor_runtime(&self, conversation_id: String) {
        let runtime = Arc::clone(&self.runtime);
        tokio::spawn(async move {
            if let Err(error) = runtime.get_or_create(&conversation_id).await {
                tracing::warn!(
                    conv_id = %conversation_id,
                    %error,
                    "failed to spawn runtime for continuation",
                );
            }
        });
    }

    pub(crate) async fn dispatch_intent(
        &self,
        intent: crate::db::ContinuationDispatchIntent,
    ) -> ContinuationDispatchStatus {
        let parent_id = intent.parent_conversation_id.clone();
        let successor_id = intent.successor_conversation_id.clone();
        let outcome =
            SendChatApplicationService::new(self.runtime.db().clone(), Arc::clone(&self.runtime))
                .send(SendChatRequest {
                    conversation_id: successor_id.clone(),
                    origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
                    text: intent.handoff,
                    message_id: intent.message_id.as_str().to_string(),
                    images: Vec::new(),
                    files: Vec::new(),
                    user_agent: intent.user_agent,
                    expansion_policy: continuation_expansion_policy(intent.opening_authority),
                })
                .await;
        match outcome {
            Ok(SendChatOutcome::Delivered | SendChatOutcome::QueuedAsSteering) => {
                ContinuationDispatchStatus::Accepted
            }
            Ok(SendChatOutcome::AlreadyPersisted) => {
                if let Err(error) = self
                    .runtime
                    .db()
                    .delete_continuation_dispatch_intent(&parent_id)
                    .await
                {
                    tracing::warn!(
                        %parent_id,
                        continuation_id = %successor_id,
                        %error,
                        "handoff is durable but continuation intent cleanup failed",
                    );
                }
                ContinuationDispatchStatus::Accepted
            }
            Ok(SendChatOutcome::Rejected { message, .. }) => {
                ContinuationDispatchStatus::Failed(message)
            }
            Err(error) => {
                tracing::warn!(
                    %parent_id,
                    continuation_id = %successor_id,
                    %error,
                    "continuation exists but opening handoff dispatch failed",
                );
                ContinuationDispatchStatus::Failed(error.to_string())
            }
        }
    }

    async fn require_settled_intent_matches(
        &self,
        predecessor_id: &str,
        successor_id: &str,
        requested: &NewContinuationDispatchIntent,
    ) -> Result<(), OwnerContinuationError> {
        let receipt = self
            .runtime
            .db()
            .completed_continuation_handoff(predecessor_id)
            .await?
            .ok_or(OwnerContinuationError::IntentConflict)?;
        if settled_owner_intent_matches(&receipt, successor_id, requested) {
            Ok(())
        } else {
            Err(OwnerContinuationError::IntentConflict)
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn drive_admission(
        &self,
        admission: &AutomaticContinuationAdmission,
    ) -> Result<bool, String> {
        if admission.opening_authority != ContinuationOpeningAuthority::GeneratedPredecessorContext
        {
            return Err("automatic admission lacks generated-context authority".to_string());
        }
        if self
            .runtime
            .db()
            .has_completed_continuation_handoff(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
            && self
                .runtime
                .db()
                .reconcile_completed_automatic_continuation(admission)
                .await
                .map_err(|error| error.to_string())?
                .is_some()
        {
            return Ok(true);
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
            let _product_admission = self
                .runtime
                .lock_product_message_admission(admission.product_conversation_id.as_str())
                .await;
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

        if plan.transfer_ownership && automatic_intent {
            self.advance_current(
                &admission.predecessor_conversation_id,
                AutomaticContinuationPhase::OwnershipTransferred,
            )
            .await?;
        }

        if !automatic_intent {
            self.runtime
                .get_or_create(&successor.id)
                .await
                .map_err(|error| error.clone())?;
            let outcome =
                SendChatApplicationService::new(self.runtime.db().clone(), self.runtime.clone())
                    .send(SendChatRequest {
                        origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
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
                SendChatOutcome::AlreadyPersisted | SendChatOutcome::Delivered => return Ok(true),
                SendChatOutcome::QueuedAsSteering => return Ok(false),
                SendChatOutcome::Rejected { message, .. } => return Err(message),
            }
        }

        if plan.dispatch_opening {
            self.runtime
                .get_or_create(&successor.id)
                .await
                .map_err(|error| error.clone())?;
            let expansion_policy = continuation_expansion_policy(intent.opening_authority);
            let outcome =
                SendChatApplicationService::new(self.runtime.db().clone(), self.runtime.clone())
                    .send(SendChatRequest {
                        origin: phoenix_core::domain::db_schema::InputOrigin::SystemGenerated,
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
        Ok(current.phase != AutomaticContinuationPhase::DispatchAccepted)
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

    async fn reconcile_durable_phase(
        &self,
        admission: &AutomaticContinuationAdmission,
    ) -> Result<AutomaticContinuationPhase, String> {
        if self
            .runtime
            .db()
            .reconcile_completed_automatic_continuation(admission)
            .await
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(self.current_admission(admission).await?.phase);
        }
        let intent = self
            .runtime
            .db()
            .continuation_dispatch_intent(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        if intent.is_some() {
            let current = self.current_admission(admission).await?;
            if current.phase == AutomaticContinuationPhase::Admitted {
                self.advance_current(
                    &admission.predecessor_conversation_id,
                    AutomaticContinuationPhase::SuccessorReserved,
                )
                .await?;
                return Ok(AutomaticContinuationPhase::SuccessorReserved);
            }
            if current.phase == AutomaticContinuationPhase::SuccessorReserved {
                self.advance_current(
                    &admission.predecessor_conversation_id,
                    AutomaticContinuationPhase::OwnershipTransferred,
                )
                .await?;
                return Ok(AutomaticContinuationPhase::OwnershipTransferred);
            }
            return Ok(current.phase);
        }
        Ok(self.current_admission(admission).await?.phase)
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

pub(crate) async fn drain_automatic_continuations(runtime: Arc<RuntimeManager>) -> bool {
    let Ok(_authority) = runtime.acquire_local_authority_pass() else {
        return false;
    };
    let admissions = match runtime
        .db()
        .pending_automatic_continuation_admissions()
        .await
    {
        Ok(admissions) => admissions,
        Err(error) => {
            warn!(%error, "automatic continuation admission discovery is unclassified");
            runtime.signal_fatal_local_authority("automatic_continuation_discovery_classification");
            return false;
        }
    };
    let service = ContinuationApplicationService::new(runtime.clone());
    for admission in admissions {
        match service.drive_admission(&admission).await {
            Ok(true) => {
                debug!(predecessor = %admission.predecessor_conversation_id, "automatic continuation progressed");
            }
            Ok(false) => {
                let error = "automatic continuation replay made no durable progress";
                warn!(predecessor = %admission.predecessor_conversation_id, %error);
                if let Err(record_error) = runtime
                    .db()
                    .reconcile_or_record_automatic_continuation_no_progress(&admission, error)
                    .await
                {
                    warn!(%record_error, "automatic continuation no-progress outcome is unclassified");
                    runtime.signal_fatal_local_authority(
                        "automatic_continuation_no_progress_classification",
                    );
                    return false;
                }
            }
            Err(error) => {
                let current = service.reconcile_durable_phase(&admission).await;
                match current {
                    Ok(phase) if phase != admission.phase => {
                        warn!(
                            predecessor = %admission.predecessor_conversation_id,
                            phase = phase.as_str(),
                            %error,
                            "automatic continuation advanced before a later step failed"
                        );
                    }
                    Ok(_) => {
                        warn!(predecessor = %admission.predecessor_conversation_id, %error, "automatic continuation made no durable progress");
                        if let Err(record_error) = runtime
                            .db()
                            .reconcile_or_record_automatic_continuation_no_progress(
                                &admission, &error,
                            )
                            .await
                        {
                            warn!(%record_error, "automatic continuation no-progress outcome is unclassified");
                            runtime.signal_fatal_local_authority(
                                "automatic_continuation_no_progress_classification",
                            );
                            return false;
                        }
                    }
                    Err(record_error) => {
                        warn!(%record_error, "automatic continuation reservation outcome is unclassified");
                        runtime.signal_fatal_local_authority(
                            "automatic_continuation_reservation_classification",
                        );
                        return false;
                    }
                }
            }
        }
    }
    true
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
    fn owner_retry_requires_the_exact_persisted_intent() {
        let persisted = crate::db::ContinuationDispatchIntent {
            parent_conversation_id: "parent".to_string(),
            successor_conversation_id: "successor".to_string(),
            message_id: phoenix_workflow::ClientTurnKey::try_from("message").unwrap(),
            handoff: "exact handoff".to_string(),
            user_agent: Some("agent".to_string()),
            opening_authority: ContinuationOpeningAuthority::UserAuthorizedInstruction,
        };
        let exact = NewContinuationDispatchIntent::user_authorized(
            phoenix_workflow::ClientTurnKey::try_from("message").unwrap(),
            "exact handoff".to_string(),
            Some("agent".to_string()),
        );
        assert!(owner_intent_matches(&persisted, &exact));

        let changed = NewContinuationDispatchIntent::user_authorized(
            phoenix_workflow::ClientTurnKey::try_from("message").unwrap(),
            "changed handoff".to_string(),
            Some("agent".to_string()),
        );
        assert!(!owner_intent_matches(&persisted, &changed));
    }

    #[test]
    fn settled_owner_retry_rejects_changed_metadata_or_authority() {
        let receipt = phoenix_db::CompletedContinuationHandoff {
            successor_conversation_id: "successor".to_string(),
            accepted_successor_message_id: "successor:message".to_string(),
            handoff: "exact handoff".to_string(),
            user_agent: Some("agent".to_string()),
            opening_authority: ContinuationOpeningAuthority::UserAuthorizedInstruction,
        };
        let exact = NewContinuationDispatchIntent::user_authorized(
            phoenix_workflow::ClientTurnKey::try_from("message").unwrap(),
            "exact handoff".to_string(),
            Some("agent".to_string()),
        );
        assert!(settled_owner_intent_matches(&receipt, "successor", &exact));

        let changed_agent = NewContinuationDispatchIntent::user_authorized(
            phoenix_workflow::ClientTurnKey::try_from("message").unwrap(),
            "exact handoff".to_string(),
            Some("other-agent".to_string()),
        );
        assert!(!settled_owner_intent_matches(
            &receipt,
            "successor",
            &changed_agent
        ));

        let changed_authority = NewContinuationDispatchIntent::generated_predecessor_context(
            phoenix_workflow::ClientTurnKey::try_from("message").unwrap(),
            "exact handoff".to_string(),
        );
        assert!(!settled_owner_intent_matches(
            &receipt,
            "successor",
            &changed_authority
        ));
    }

    #[test]
    fn manual_continuation_recovery_preserves_literal_payload_semantics() {
        assert_eq!(
            continuation_expansion_policy(ContinuationOpeningAuthority::UserAuthorizedInstruction),
            MessageExpansionPolicy::LiteralText
        );
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
