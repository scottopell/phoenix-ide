//! Explicit retry is admitted and executed only by the live source actor.

use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::native_close::NativeCloseRetryResult;
use std::sync::Arc;
use tokio::sync::{oneshot, OwnedMutexGuard};

use super::NativeCloseApplicationError;

pub(crate) struct NativeCloseRetryRequest {
    pub source: SourceToolCall,
    pub target: ProductConversationId,
    pub attempt_id: CloseAttemptId,
    pub failed_run_ordinal: CloseRunOrdinal,
    pub backend: Arc<super::super::RuntimeManager>,
    pub admitted: super::super::AdmittedOperation,
    pub target_gate: OwnedMutexGuard<()>,
    pub attempt_gate: OwnedMutexGuard<()>,
    pub response: oneshot::Sender<Result<NativeCloseRetryResult, NativeCloseApplicationError>>,
}

impl super::super::RuntimeManager {
    /// Locks in the shared resource order before enqueueing to the actual source actor.
    pub(crate) async fn apply_native_close_explicit_retry(
        self: &Arc<Self>,
        source_transcript_id: &str,
        source: SourceToolCall,
        target: ProductConversationId,
        attempt_id: CloseAttemptId,
        failed_run_ordinal: CloseRunOrdinal,
    ) -> Result<NativeCloseRetryResult, NativeCloseApplicationError> {
        let target_gate = self
            .conversation_admission(target.as_str())
            .await
            .lock_owned()
            .await;
        let attempt_gate = self
            .close_retirement_execution
            .lock(attempt_id.as_str())
            .await;
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
        let (response, receiver) = oneshot::channel();
        sender
            .send(
                crate::runtime::AcknowledgedRuntimeRequest::ExecuteNativeCloseRetry(Box::new(
                    NativeCloseRetryRequest {
                        source,
                        target,
                        attempt_id,
                        failed_run_ordinal,
                        backend: Arc::clone(self),
                        admitted,
                        target_gate,
                        attempt_gate,
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

#[cfg(test)]
mod tests {
    use super::super::{retry_command_from_executing_source, NativeCloseSourceRefusal};
    use crate::state_machine::state::{AssistantMessage, ToolCall, ToolInput};
    use crate::state_machine::ConvState;
    use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal};
    use phoenix_core::domain::db_schema::SourceToolCall;
    use phoenix_core::domain::llm_types::ContentBlock;
    use phoenix_core::domain::product_conversation::ProductConversationId;
    use serde_json::json;

    fn fixture(source: SourceToolCall, input: serde_json::Value) -> ConvState {
        ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                source.message_id,
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id,
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        }
    }

    #[test]
    fn explicit_retry_source_requires_exact_live_occurrence_and_exact_intent() {
        let source = SourceToolCall {
            message_id: "assistant".into(),
            tool_use_id: "close-retry".into(),
        };
        let target = ProductConversationId::parse("target").unwrap();
        let attempt_id = CloseAttemptId::parse("attempt").unwrap();
        let failed_run_ordinal = CloseRunOrdinal::parse(2).unwrap();
        let intent = phoenix_db::native_close::NativeCloseIntent::ExplicitRetry {
            target: target.clone(),
            attempt_id: attempt_id.clone(),
            failed_run_ordinal,
        };
        let input = intent.source_input();
        let state = fixture(source.clone(), input.clone());
        let command = retry_command_from_executing_source(
            &state,
            "source-transcript",
            &source,
            target.clone(),
            attempt_id.clone(),
            failed_run_ordinal,
        )
        .unwrap();
        assert_eq!(command.intent(), &intent);
        assert_eq!(
            retry_command_from_executing_source(
                &ConvState::Idle,
                "source-transcript",
                &source,
                target.clone(),
                attempt_id.clone(),
                failed_run_ordinal,
            ),
            Err(NativeCloseSourceRefusal::NotExactExecutingSource)
        );

        let wrong = fixture(
            source.clone(),
            json!({
                "action":"explicit_retry",
                "product_conversation_id":"target",
                "attempt_id":"attempt",
                "failed_run_ordinal":3
            }),
        );
        assert_eq!(
            retry_command_from_executing_source(
                &wrong,
                "source-transcript",
                &source,
                target,
                attempt_id,
                failed_run_ordinal,
            ),
            Err(NativeCloseSourceRefusal::NotExactExecutingSource)
        );
    }
}
