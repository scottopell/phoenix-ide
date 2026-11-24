//! Exact source matching and acknowledged requests for native Close application.
//!
//! This is a read-only predicate, NOT an admission token or an application
//! facade. Its caller must be the SOURCE conversation actor and keep exclusive
//! ownership from source matching through receipt lookup, admission and execution.
//! Calling it against a watch snapshot or persisted state does not establish that
//! ordering. Current-Coordinator and target checks still belong in the transaction.

use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::native_close::{NativeCloseCommand, NativeCloseIntent, NativeCloseRunRef};
use std::sync::Arc;

use crate::state_machine::ConvState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NativeCloseApplicationError {
    #[error("Close requires its exact live executing Coordinator source")]
    SourceRefused,
    #[error("Close application owner is unavailable")]
    Unavailable,
    #[error("Close storage rejected the command")]
    Storage,
    #[error("Close durable outcome could not be classified; authority is fenced")]
    FatalAuthority,
}

/// Keeps admission receipts distinct from fresh retained observations.
#[derive(Debug)]
pub(crate) enum NativeCloseApplicationResult {
    Initial(phoenix_db::native_close::NativeCloseInitialResult),
    Observe(phoenix_db::native_close::NativeCloseObserveResult),
    ExplicitRetry(phoenix_db::native_close::NativeCloseRetryResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeCloseSourceRefusal {
    NotExactExecutingSource,
}

/// Derives correlation only after matching the actual initial command occurrence.
///
/// There is deliberately no historical-canonical fallback: a stopped, completed,
/// pending or different source cannot reach receipt lookup, including on replay.
/// Cancelled-predecessor, observation and retry inputs are unsupported here rather
/// than normalized into initial admission. This function does not access receipts.
///
/// The actor must supply its own transcript ID, not an input field. No request UUID
/// or fingerprint is accepted from the public request.
pub(super) fn initial_command_from_executing_source(
    state: &ConvState,
    source_transcript_id: &str,
    source: &SourceToolCall,
    target: ProductConversationId,
) -> Result<NativeCloseCommand, NativeCloseSourceRefusal> {
    command_from_executing_source(
        state,
        source_transcript_id,
        source,
        NativeCloseIntent::Initial { target },
    )
}

pub(super) fn retry_command_from_executing_source(
    state: &ConvState,
    source_transcript_id: &str,
    source: &SourceToolCall,
    target: ProductConversationId,
    attempt_id: CloseAttemptId,
    failed_run_ordinal: CloseRunOrdinal,
) -> Result<NativeCloseCommand, NativeCloseSourceRefusal> {
    command_from_executing_source(
        state,
        source_transcript_id,
        source,
        NativeCloseIntent::ExplicitRetry {
            target,
            attempt_id,
            failed_run_ordinal,
        },
    )
}

pub(super) fn observe_command_from_executing_source(
    state: &ConvState,
    source_transcript_id: &str,
    source: &SourceToolCall,
    referenced: NativeCloseRunRef,
) -> Result<NativeCloseCommand, NativeCloseSourceRefusal> {
    command_from_executing_source(
        state,
        source_transcript_id,
        source,
        NativeCloseIntent::Observe {
            target: referenced.target,
            attempt_id: referenced.attempt_id,
            run_ordinal: referenced.run_ordinal,
        },
    )
}

fn command_from_executing_source(
    state: &ConvState,
    source_transcript_id: &str,
    source: &SourceToolCall,
    intent: NativeCloseIntent,
) -> Result<NativeCloseCommand, NativeCloseSourceRefusal> {
    let invalid = NativeCloseSourceRefusal::NotExactExecutingSource;
    if phoenix_core::domain::executing_source::verify_executing_source(
        state,
        source,
        "close_conversation",
        &intent.source_input(),
    )
    .is_err()
    {
        return Err(invalid);
    }
    let ConvState::ToolExecuting { current_tool, .. } = state else {
        return Err(invalid);
    };
    let executing_input = current_tool.input.to_value();
    if executing_input != intent.source_input() {
        return Err(invalid);
    }
    NativeCloseCommand::new(source_transcript_id.to_owned(), source.clone(), intent)
        .map_err(|_| invalid)
}

/// Owned queue envelope: neither caller cancellation nor a dropped reply can
/// release the target gate or abandon a started database transaction.
/// The source actor never acquires this gate and never calls another actor.
/// Cleanup is a separate mutation unit and is deliberately not dispatched here.
pub(crate) struct NativeCloseInitialRequest {
    pub source: SourceToolCall,
    pub backend: Arc<crate::runtime::RuntimeManager>,
    pub target: ProductConversationId,
    pub admitted: super::AdmittedOperation,
    pub target_gate: tokio::sync::OwnedMutexGuard<()>,
    pub response: tokio::sync::oneshot::Sender<
        Result<phoenix_db::native_close::NativeCloseInitialResult, NativeCloseApplicationError>,
    >,
}

impl super::RuntimeManager {
    /// Initial admission is routed to the actual SOURCE runtime, never a
    /// reconstructed target runtime. The target gate is taken before enqueue;
    /// waiting for it leaves the source actor free to settle Stop. Once queued,
    /// the envelope owns the whole mutation unit through durable classification.
    /// No inline self-message is used: only the detached tool adapter may call
    /// this route. A queued Stop is not an acknowledged durable Stop; whichever
    /// request the source actor processes first establishes the ordering.
    pub(crate) async fn apply_native_close_initial(
        self: &Arc<Self>,
        source_transcript_id: &str,
        source: SourceToolCall,
        target: ProductConversationId,
    ) -> Result<phoenix_db::native_close::NativeCloseInitialResult, NativeCloseApplicationError>
    {
        let gate = self.conversation_admission(target.as_str()).await;
        let target_gate = gate.lock_owned().await;
        let admitted = self
            .acquire_local_authority_pass()
            .map_err(|()| NativeCloseApplicationError::FatalAuthority)?;
        let sender = {
            let runtimes = self.runtimes.read().await;
            runtimes
                .get(source_transcript_id)
                .map(|handle| handle.acknowledged_event_tx.clone())
        }
        .ok_or(NativeCloseApplicationError::Unavailable)?;
        let (response, receiver) = tokio::sync::oneshot::channel();
        sender
            .send(
                super::AcknowledgedRuntimeRequest::ExecuteNativeCloseInitial(Box::new(
                    NativeCloseInitialRequest {
                        source,
                        backend: Arc::clone(self),
                        target,
                        admitted,
                        target_gate,
                        response,
                    },
                )),
            )
            .await
            .map_err(|_| NativeCloseApplicationError::Unavailable)?;
        receiver
            .await
            .map_err(|_| NativeCloseApplicationError::Unavailable)?
    }
}

mod observe;
pub(crate) use observe::NativeCloseObserveRequest;
mod retry;
pub(crate) use retry::NativeCloseRetryRequest;

#[cfg(test)]
mod tests;
