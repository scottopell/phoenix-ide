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
    FederatedCoordinatorContinuationRequest,
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

#[derive(Debug)]
pub(crate) struct FederatedNativeContinueRequest {
    pub source: phoenix_db::FederatedContinuationSource,
    pub product_conversation_id: ProductConversationId,
    pub predecessor_transcript_id: String,
    pub summary_message_id: String,
    pub first_message_id: ClientTurnKey,
}

enum NativeContinueAdmission {
    CurrentCoordinator {
        source_transcript_id: String,
        #[cfg(test)]
        source_gate_wait: Option<tokio::sync::oneshot::Sender<()>>,
    },
    FederatedPeer {
        authority: phoenix_db::FederationSendAuthority,
        source: phoenix_db::FederatedContinuationSource,
    },
}

struct NativeContinueOwnedRequest {
    admission: NativeContinueAdmission,
    product_conversation_id: ProductConversationId,
    predecessor_transcript_id: String,
    summary_message_id: String,
    first_message_id: ClientTurnKey,
}

#[derive(Debug)]
pub(crate) enum NativeContinueError {
    Message(String),
    Database(phoenix_db::DbError),
}

impl From<String> for NativeContinueError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl std::fmt::Display for NativeContinueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            Self::Database(error) => error.fmt(formatter),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FederatedNativeOpeningEvidence {
    Pending { message_id: String },
    Accepted { message_id: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FederatedNativePendingDispatch {
    Accepted,
    AlreadyExists,
    DispatchFailed { error: String },
}

impl FederatedNativePendingDispatch {
    fn from_parts(
        status: ContinueConversationStatus,
        error: Option<String>,
    ) -> Result<Self, NativeContinueError> {
        match (status, error) {
            (ContinueConversationStatus::Accepted, None) => Ok(Self::Accepted),
            (ContinueConversationStatus::AlreadyExists, None) => Ok(Self::AlreadyExists),
            (ContinueConversationStatus::DispatchFailed, Some(error)) if !error.is_empty() => {
                Ok(Self::DispatchFailed { error })
            }
            _ => Err(NativeContinueError::Message(
                "continuation dispatch returned an inconsistent status/error pair".into(),
            )),
        }
    }

    fn into_parts(self) -> (ContinueConversationStatus, Option<String>) {
        match self {
            Self::Accepted => (ContinueConversationStatus::Accepted, None),
            Self::AlreadyExists => (ContinueConversationStatus::AlreadyExists, None),
            Self::DispatchFailed { error } => {
                (ContinueConversationStatus::DispatchFailed, Some(error))
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum FederatedNativeContinueResult {
    Pending {
        conversation_id: String,
        slug: Option<String>,
        opening_message_id: String,
        dispatch: FederatedNativePendingDispatch,
    },
    Accepted {
        conversation_id: String,
        slug: Option<String>,
        opening_message_id: String,
    },
}

impl FederatedNativeContinueResult {
    fn into_response(self) -> ContinueConversationResponse {
        self.into_response_and_opening().0
    }

    pub(crate) fn into_response_and_opening(
        self,
    ) -> (ContinueConversationResponse, FederatedNativeOpeningEvidence) {
        match self {
            Self::Pending {
                conversation_id,
                slug,
                opening_message_id,
                dispatch,
            } => {
                let (status, error) = dispatch.into_parts();
                (
                    ContinueConversationResponse {
                        conversation_id,
                        slug,
                        status,
                        error,
                    },
                    FederatedNativeOpeningEvidence::Pending {
                        message_id: opening_message_id,
                    },
                )
            }
            Self::Accepted {
                conversation_id,
                slug,
                opening_message_id,
            } => (
                ContinueConversationResponse {
                    conversation_id,
                    slug,
                    status: ContinueConversationStatus::AlreadyExists,
                    error: None,
                },
                FederatedNativeOpeningEvidence::Accepted {
                    message_id: opening_message_id,
                },
            ),
        }
    }
}

pub(crate) enum FederatedNativeContinueCompletion {
    Established(Result<FederatedNativeContinueResult, NativeContinueError>),
    LocalAuthorityClosed,
}

pub(crate) enum NativeContinueCompletion {
    Established(Result<ContinueConversationResponse, NativeContinueError>),
    LocalAuthorityClosed,
}

#[cfg(test)]
impl NativeContinueCompletion {
    fn established(self) -> Result<ContinueConversationResponse, String> {
        match self {
            Self::Established(result) => result.map_err(|error| error.to_string()),
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
        let request = NativeContinueOwnedRequest {
            admission: NativeContinueAdmission::CurrentCoordinator {
                source_transcript_id: request.source_transcript_id,
                #[cfg(test)]
                source_gate_wait: request.source_gate_wait,
            },
            product_conversation_id: request.product_conversation_id,
            predecessor_transcript_id: request.predecessor_transcript_id,
            summary_message_id: request.summary_message_id,
            first_message_id: request.first_message_id,
        };
        match self.continue_with_admission(request).await {
            FederatedNativeContinueCompletion::Established(result) => {
                NativeContinueCompletion::Established(result.map(|result| result.into_response()))
            }
            FederatedNativeContinueCompletion::LocalAuthorityClosed => {
                NativeContinueCompletion::LocalAuthorityClosed
            }
        }
    }

    pub(crate) async fn continue_from_federated_coordinator(
        &self,
        request: FederatedNativeContinueRequest,
        authority: phoenix_db::FederationSendAuthority,
    ) -> FederatedNativeContinueCompletion {
        let request = NativeContinueOwnedRequest {
            admission: NativeContinueAdmission::FederatedPeer {
                authority,
                source: request.source,
            },
            product_conversation_id: request.product_conversation_id,
            predecessor_transcript_id: request.predecessor_transcript_id,
            summary_message_id: request.summary_message_id,
            first_message_id: request.first_message_id,
        };
        self.continue_with_admission(request).await
    }

    async fn continue_with_admission(
        &self,
        request: NativeContinueOwnedRequest,
    ) -> FederatedNativeContinueCompletion {
        let runtime = self.runtime.clone();
        let result = tokio::spawn(async move {
            Self::new(runtime)
                .continue_from_coordinator_owned(request)
                .await
        })
        .await
        .unwrap_or_else(|error| Err(NativeContinueError::Message(error.to_string())));
        if self.runtime.local_authority_is_closed() {
            FederatedNativeContinueCompletion::LocalAuthorityClosed
        } else {
            FederatedNativeContinueCompletion::Established(result)
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "preserve the sequential native continuation admission and dispatch transaction"
    )]
    async fn continue_from_coordinator_owned(
        &self,
        mut request: NativeContinueOwnedRequest,
    ) -> Result<FederatedNativeContinueResult, NativeContinueError> {
        let mut owner = self.runtime.acquire_local_authority_pass().map_err(|()| {
            "runtime admission closed after fatal local authority loss".to_string()
        })?;
        let db = self.runtime.db();
        let _source_gate = match &mut request.admission {
            NativeContinueAdmission::CurrentCoordinator {
                source_transcript_id,
                #[cfg(test)]
                source_gate_wait,
            } => {
                db.require_current_coordinator_transcript(source_transcript_id)
                    .await
                    .map_err(|error| error.to_string())?;
                let source = db
                    .get_conversation(source_transcript_id)
                    .await
                    .map_err(|error| error.to_string())?;
                if source.product_conversation_id == request.product_conversation_id {
                    return Err("native Continue cannot rotate its Coordinator source"
                        .to_string()
                        .into());
                }
                #[cfg(test)]
                if let Some(waiting) = source_gate_wait.take() {
                    let _ = waiting.send(());
                }
                let gate = self
                    .runtime
                    .lock_product_message_admission(source.product_conversation_id.as_str())
                    .await;
                db.require_current_coordinator_transcript(source_transcript_id)
                    .await
                    .map_err(|error| error.to_string())?;
                Some(gate)
            }
            NativeContinueAdmission::FederatedPeer { .. } => None,
        };
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
            return Err("predecessor does not belong to the requested product"
                .to_string()
                .into());
        }
        if predecessor.continued_in_conv_id.is_none() {
            let ConvState::ContextExhausted { summary, .. } = &predecessor.state else {
                return Err("predecessor is not context-exhausted".to_string().into());
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
                )
                .into());
            }
        }
        let pass = ReservationPass {
            owner: Some(owner.reborrow()),
        };
        let reservation = match &request.admission {
            NativeContinueAdmission::FederatedPeer { authority, source } => {
                db.continue_from_federated_coordinator(FederatedCoordinatorContinuationRequest {
                    authority,
                    source,
                    product_conversation_id: &request.product_conversation_id,
                    predecessor_transcript_id: &request.predecessor_transcript_id,
                    summary_message_id: &request.summary_message_id,
                    first_message_id: request.first_message_id.clone(),
                })
                .await
            }
            NativeContinueAdmission::CurrentCoordinator {
                source_transcript_id,
                ..
            } => {
                db.continue_from_current_coordinator(CurrentCoordinatorContinuationRequest {
                    source_transcript_id,
                    product_conversation_id: &request.product_conversation_id,
                    predecessor_transcript_id: &request.predecessor_transcript_id,
                    summary_message_id: &request.summary_message_id,
                    first_message_id: request.first_message_id.clone(),
                })
                .await
            }
        };
        let (outcome, opening) = pass
            .classify(reservation)?
            .map_err(NativeContinueError::Database)?;
        let (successor, existed) = match outcome {
            ContinueOutcome::Created(successor) => (successor, false),
            ContinueOutcome::AlreadyContinued(successor) => (successor, true),
            ContinueOutcome::ParentNotContextExhausted { state_variant } => {
                return Err(
                    format!("predecessor is not context-exhausted: {state_variant}").into(),
                );
            }
        };
        let result = match opening {
            CoordinatorContinuationOpening::Accepted {
                accepted_message_id,
            } => FederatedNativeContinueResult::Accepted {
                conversation_id: successor.id,
                slug: successor.slug,
                opening_message_id: accepted_message_id,
            },
            CoordinatorContinuationOpening::Pending(intent) => {
                let opening_message_id = intent.message_id.to_string();
                let review = self
                    .runtime
                    .review_reserved_continuation_opening(intent.clone())
                    .await?;
                if !review.source.fits() {
                    return Err(format!(
                        "generated handoff exceeds reserved successor capacity: {:?}",
                        review.source
                    )
                    .into());
                }
                let (status, error) =
                    super::manual::dispatch_continuation_handoff(self, intent).await;
                let status = if existed {
                    super::manual::existing_continuation_status(status)
                } else {
                    status
                };
                FederatedNativeContinueResult::Pending {
                    conversation_id: successor.id,
                    slug: successor.slug,
                    opening_message_id,
                    dispatch: FederatedNativePendingDispatch::from_parts(status, error)?,
                }
            }
        };
        if self.runtime.local_authority_is_closed() {
            return Err("runtime admission closed after fatal local authority loss"
                .to_string()
                .into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
