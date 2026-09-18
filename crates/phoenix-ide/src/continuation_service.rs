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

#[derive(Clone)]
pub(crate) struct ContinuationApplicationService {
    runtime: Arc<RuntimeManager>,
}

impl ContinuationApplicationService {
    #[must_use]
    pub(crate) fn new(runtime: Arc<RuntimeManager>) -> Self {
        Self { runtime }
    }

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
            .has_completed_continuation_handoff(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
        {
            return self
                .advance_until(admission, AutomaticContinuationPhase::MessageSettled)
                .await;
        }
        let summary = self
            .runtime
            .db()
            .get_messages(&admission.predecessor_conversation_id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|message| message.message_id == admission.summary_message_id)
            .ok_or_else(|| "automatic continuation summary is missing".to_string())?;
        let MessageContent::Continuation(summary_content) = summary.content else {
            return Err("automatic continuation summary has the wrong message type".to_string());
        };
        let (outcome, intent) = self
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
            .map_err(|error| error.to_string())?;
        let successor = match outcome {
            ContinueOutcome::Created(conversation)
            | ContinueOutcome::AlreadyContinued(conversation) => conversation,
            ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                return Err(format!("predecessor state changed to {state_variant}"));
            }
        };
        self.advance_until(admission, AutomaticContinuationPhase::SuccessorReserved)
            .await?;
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
        self.advance_current(
            &admission.predecessor_conversation_id,
            AutomaticContinuationPhase::OwnershipTransferred,
        )
        .await?;
        let intent = match intent {
            Some(intent) => intent,
            None => self
                .runtime
                .db()
                .continuation_dispatch_intent(&admission.predecessor_conversation_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "automatic continuation dispatch intent is missing".to_string())?,
        };
        self.runtime
            .get_or_create(&successor.id)
            .await
            .map_err(|error| error.to_string())?;
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
                self.advance_current(
                    &admission.predecessor_conversation_id,
                    AutomaticContinuationPhase::DispatchAccepted,
                )
                .await
            }
            SendChatOutcome::Rejected { message, .. } => Err(message),
        }
    }

    async fn advance_until(
        &self,
        admission: &AutomaticContinuationAdmission,
        target: AutomaticContinuationPhase,
    ) -> Result<(), String> {
        let phases = [
            AutomaticContinuationPhase::Admitted,
            AutomaticContinuationPhase::SuccessorReserved,
            AutomaticContinuationPhase::OwnershipTransferred,
            AutomaticContinuationPhase::DispatchAccepted,
            AutomaticContinuationPhase::MessageSettled,
        ];
        let current_index = phases
            .iter()
            .position(|phase| *phase == admission.phase)
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
                debug!(predecessor = %admission.predecessor_conversation_id, "automatic continuation progressed")
            }
            Err(error) => {
                warn!(predecessor = %admission.predecessor_conversation_id, %error, "automatic continuation made no durable progress");
                if let Err(record_error) = runtime
                    .db()
                    .record_automatic_continuation_no_progress(
                        &admission.predecessor_conversation_id,
                        &error,
                    )
                    .await
                {
                    warn!(%record_error, "failed to persist automatic continuation failure");
                }
            }
        }
    }
}
