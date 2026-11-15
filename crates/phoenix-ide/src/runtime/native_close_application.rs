//! Source matching for the dormant native Close application.
//!
//! This is a read-only predicate, NOT an admission token or an application
//! facade. Its eventual caller must be the SOURCE conversation actor and keep
//! exclusive actor ownership from this check through receipt lookup and commit.
//! Calling it against a watch snapshot or persisted state does not establish that
//! ordering. Current-Coordinator and target checks still belong in the transaction.

use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::native_close::{NativeCloseCommand, NativeCloseIntent};
use serde_json::json;

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
#[allow(dead_code)] // Dormant until the source-owned transactional application is assembled.
pub(super) fn initial_command_from_executing_source(
    state: &ConvState,
    source_transcript_id: &str,
    source: &SourceToolCall,
    target: ProductConversationId,
) -> Result<NativeCloseCommand, NativeCloseSourceRefusal> {
    let invalid = NativeCloseSourceRefusal::NotExactExecutingSource;
    let ConvState::ToolExecuting {
        current_tool,
        remaining_tools,
        completed_results,
        assistant_message,
        ..
    } = state
    else {
        return Err(invalid);
    };
    if source.message_id.trim().is_empty()
        || source.tool_use_id.trim().is_empty()
        || assistant_message.message_id != source.message_id
        || current_tool.id != source.tool_use_id
        || current_tool.name() != "close_conversation"
        || remaining_tools.iter().any(|tool| tool.id == source.tool_use_id)
        || completed_results
            .iter()
            .any(|result| result.tool_use_id() == source.tool_use_id)
    {
        return Err(invalid);
    }
    let executing = current_tool.input.to_value();
    let expected = json!({"action": "initial", "product_conversation_id": target.as_str()});
    let mut occurrences = assistant_message.content.iter().filter_map(|block| {
        if let ContentBlock::ToolUse { id, name, input } = block {
            (id == &source.tool_use_id).then_some((name, input))
        } else {
            None
        }
    });
    let Some((name, canonical)) = occurrences.next() else {
        return Err(invalid);
    };
    if occurrences.next().is_some()
        || name != "close_conversation"
        || canonical != &executing
        || canonical != &expected
    {
        return Err(invalid);
    }
    NativeCloseCommand::new(
        source_transcript_id.to_owned(),
        source.clone(),
        NativeCloseIntent::Initial { target },
    )
    .map_err(|_| invalid)
}

/// Owned queue envelope: neither caller cancellation nor a dropped reply can
/// release the target gate or abandon a started database transaction.
/// The source actor never acquires this gate and never calls another actor.
/// Cleanup is a separate mutation unit and is deliberately not dispatched here.
pub(crate) struct NativeCloseInitialRequest {
    pub source: SourceToolCall,
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
    #[allow(dead_code)] // Dormant until the complete unified adapter is qualified.
    pub(crate) async fn apply_native_close_initial(
        &self,
        source_transcript_id: &str,
        source: SourceToolCall,
        target: ProductConversationId,
    ) -> Result<phoenix_db::native_close::NativeCloseInitialResult, NativeCloseApplicationError> {
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
            .send(super::AcknowledgedRuntimeRequest::ExecuteNativeCloseInitial(
                Box::new(NativeCloseInitialRequest {
                    source,
                    target,
                    admitted,
                    target_gate,
                    response,
                }),
            ))
            .await
            .map_err(|_| NativeCloseApplicationError::Unavailable)?;
        receiver
            .await
            .map_err(|_| NativeCloseApplicationError::Unavailable)?
    }
}

#[cfg(test)]
mod tests;
