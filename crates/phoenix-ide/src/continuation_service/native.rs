use super::ContinuationApplicationService;
use crate::api::{ContinueConversationResponse, ContinueConversationStatus};
use crate::runtime::AdmittedOperation;
use phoenix_core::domain::product_conversation::{
    ContinuationOpeningAuthority, ProductConversationId,
};
use phoenix_core::domain::sm_state::ConvState;
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{
    ContinueOutcome, CoordinatorContinuationOpening, CurrentCoordinatorContinuationRequest,
};
use phoenix_workflow::ClientTurnKey;

#[allow(
    clippy::struct_field_names,
    reason = "identity suffixes distinguish the native continuation request fields"
)]
pub(crate) struct NativeContinueRequest {
    pub source_transcript_id: String,
    pub product_conversation_id: ProductConversationId,
    pub predecessor_transcript_id: String,
    pub summary_message_id: String,
    pub first_message_id: ClientTurnKey,
    #[cfg(test)]
    pub source_gate_wait: Option<tokio::sync::oneshot::Sender<()>>,
}

pub(crate) enum NativeContinueCompletion {
    Established(Result<ContinueConversationResponse, String>),
    LocalAuthorityClosed,
}

#[cfg(test)]
impl NativeContinueCompletion {
    fn established(self) -> Result<ContinueConversationResponse, String> {
        match self {
            Self::Established(result) => result,
            Self::LocalAuthorityClosed => panic!("ordinary reply requires open local authority"),
        }
    }
}

struct ReservationPass {
    owner: Option<AdmittedOperation>,
}

impl ReservationPass {
    fn classify<T>(mut self, result: LocalAuthorityResult<T>) -> Result<T, String> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(value) => {
                self.owner.take();
                Ok(value)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                Err("continuation reservation durability is unclassified".to_string())
            }
        }
    }
}

impl Drop for ReservationPass {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.as_ref() {
            owner.close("native_continuation_reservation_unclassified");
        }
    }
}

impl ContinuationApplicationService {
    pub(crate) async fn continue_from_coordinator(
        &self,
        request: NativeContinueRequest,
    ) -> NativeContinueCompletion {
        let runtime = self.runtime.clone();
        let result = tokio::spawn(async move {
            Self::new(runtime)
                .continue_from_coordinator_owned(request)
                .await
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
        if self.runtime.local_authority_is_closed() {
            NativeContinueCompletion::LocalAuthorityClosed
        } else {
            NativeContinueCompletion::Established(result)
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "preserve the sequential native continuation admission and dispatch transaction"
    )]
    async fn continue_from_coordinator_owned(
        &self,
        request: NativeContinueRequest,
    ) -> Result<ContinueConversationResponse, String> {
        let mut owner = self.runtime.acquire_local_authority_pass().map_err(|()| {
            "runtime admission closed after fatal local authority loss".to_string()
        })?;
        let db = self.runtime.db();
        db.require_current_coordinator_transcript(&request.source_transcript_id)
            .await
            .map_err(|error| error.to_string())?;
        let source = db
            .get_conversation(&request.source_transcript_id)
            .await
            .map_err(|error| error.to_string())?;
        if source.product_conversation_id == request.product_conversation_id {
            return Err("native Continue cannot rotate its Coordinator source".to_string());
        }
        #[cfg(test)]
        if let Some(waiting) = request.source_gate_wait {
            let _ = waiting.send(());
        }
        let _source_gate = self
            .runtime
            .lock_product_message_admission(source.product_conversation_id.as_str())
            .await;
        db.require_current_coordinator_transcript(&request.source_transcript_id)
            .await
            .map_err(|error| error.to_string())?;
        let _target_gate = self
            .runtime
            .lock_product_message_admission(request.product_conversation_id.as_str())
            .await;
        let target_mutation = self
            .runtime
            .mutation_admission(&request.predecessor_transcript_id)
            .await
            .map_err(|error| error.to_string())?;
        let _target_mutation = target_mutation.lock().await;
        let predecessor = db
            .get_conversation(&request.predecessor_transcript_id)
            .await
            .map_err(|error| error.to_string())?;
        if predecessor.product_conversation_id != request.product_conversation_id {
            return Err("predecessor does not belong to the requested product".to_string());
        }
        if predecessor.continued_in_conv_id.is_none() {
            let ConvState::ContextExhausted { summary, .. } = &predecessor.state else {
                return Err("predecessor is not context-exhausted".to_string());
            };
            let review = self
                .runtime
                .review_continuation_admission(
                    &request.predecessor_transcript_id,
                    summary.clone(),
                    ContinuationOpeningAuthority::GeneratedPredecessorContext,
                )
                .await?;
            if !review.source.fits() {
                return Err(format!(
                    "generated handoff exceeds successor capacity: {:?}",
                    review.source
                ));
            }
        }
        let pass = ReservationPass {
            owner: Some(owner.reborrow()),
        };
        let (outcome, opening) = pass
            .classify(
                db.continue_from_current_coordinator(CurrentCoordinatorContinuationRequest {
                    source_transcript_id: &request.source_transcript_id,
                    product_conversation_id: &request.product_conversation_id,
                    predecessor_transcript_id: &request.predecessor_transcript_id,
                    summary_message_id: &request.summary_message_id,
                    first_message_id: request.first_message_id,
                })
                .await,
            )?
            .map_err(|error| error.to_string())?;
        let (successor, existed) = match outcome {
            ContinueOutcome::Created(successor) => (successor, false),
            ContinueOutcome::AlreadyContinued(successor) => (successor, true),
            ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                return Err(format!(
                    "predecessor is not context-exhausted: {state_variant}"
                ));
            }
        };
        let (status, error) = match opening {
            CoordinatorContinuationOpening::Accepted {
                accepted_message_id: _,
            } => (ContinueConversationStatus::AlreadyExists, None),
            CoordinatorContinuationOpening::Pending(intent) => {
                let review = self
                    .runtime
                    .review_reserved_continuation_opening(intent.clone())
                    .await?;
                if !review.source.fits() {
                    return Err(format!(
                        "generated handoff exceeds reserved successor capacity: {:?}",
                        review.source
                    ));
                }
                let (status, error) =
                    super::manual::dispatch_continuation_handoff(self, intent).await;
                let status = if existed {
                    super::manual::existing_continuation_status(status)
                } else {
                    status
                };
                (status, error)
            }
        };
        if self.runtime.local_authority_is_closed() {
            return Err("runtime admission closed after fatal local authority loss".to_string());
        }
        Ok(ContinueConversationResponse {
            conversation_id: successor.id,
            slug: successor.slug,
            status,
            error,
        })
    }
}

#[cfg(test)]
mod tests;
