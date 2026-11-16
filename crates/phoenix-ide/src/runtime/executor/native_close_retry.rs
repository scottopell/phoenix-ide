//! Source-owned explicit retry admission followed by execution of only a new successor.

use super::{
    CancellationToken, ConversationRuntime, LlmClient, RuntimeSettlementAuthority, Storage,
    ToolExecutor,
};
use crate::runtime::native_close_application::{
    retry_command_from_executing_source, NativeCloseApplicationError,
};
use phoenix_core::domain::close::{CloseAttemptId, CloseRunOrdinal, CloseRunRef};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::native_close::{
    NativeCloseCommand, NativeCloseIntent, NativeCloseRetryResult, NativeCloseRetrySourceAssessment,
};
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{
    AdmitCloseSafeRetryRequest, CloseRetryRequestedBy, StoppedCloseOriginalRetirementProof,
};
use std::sync::Arc;

struct NativeCloseRetryAssessment {
    command: NativeCloseCommand,
    original: crate::runtime::close_retirement::NativeCloseRetryAssessment,
    request: AdmitCloseSafeRetryRequest,
}

impl NativeCloseRetrySourceAssessment for NativeCloseRetryAssessment {
    fn command(&self) -> &NativeCloseCommand {
        &self.command
    }

    fn original_proof(&self) -> &StoppedCloseOriginalRetirementProof {
        &self.original.original
    }

    fn request(&self) -> &AdmitCloseSafeRetryRequest {
        &self.request
    }
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    /// Execute one queued request and report whether the actor must terminate.
    /// Both the production event loop and bounded actor tests use this handler.
    pub(super) async fn handle_native_close_retry(
        &mut self,
        request: crate::runtime::native_close_application::NativeCloseRetryRequest,
    ) -> bool {
        let crate::runtime::native_close_application::NativeCloseRetryRequest {
            source,
            target,
            attempt_id,
            failed_run_ordinal,
            backend,
            admitted,
            target_gate,
            attempt_gate,
            response,
        } = request;
        let result = self
            .execute_native_close_retry(
                source,
                target,
                attempt_id,
                failed_run_ordinal,
                backend,
                admitted,
                target_gate,
                attempt_gate,
            )
            .await;
        let fatal = matches!(
            result,
            Err(NativeCloseApplicationError::FatalAuthority)
        );
        let _ = response.send(result);
        fatal
    }
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    /// Bind backend work to the actor's installed database handle, not a path or
    /// independently opened pool. Cloned handles preserve this authority identity.
    pub(super) fn check_native_close_retry_backend(
        &self,
        backend_db: &phoenix_db::Database,
    ) -> Result<(), NativeCloseApplicationError> {
        if self
            .native_task_admission_db
            .as_ref()
            .is_some_and(|actor_db| actor_db.shares_authority_with(backend_db))
        {
            Ok(())
        } else {
            Err(NativeCloseApplicationError::SourceRefused)
        }
    }

    /// Runs the retry solely in the acknowledged source actor; gate ownership
    /// transferred with the request is retained through durable classification.
    // Keep receipt lookup, admission, execution and settlement ownership together.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) async fn execute_native_close_retry(
        &mut self,
        source: SourceToolCall,
        target: ProductConversationId,
        attempt_id: CloseAttemptId,
        failed_run_ordinal: CloseRunOrdinal,
        backend: Arc<crate::runtime::RuntimeManager>,
        admitted: crate::runtime::AdmittedOperation,
        _target_gate: tokio::sync::OwnedMutexGuard<()>,
        _attempt_gate: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<NativeCloseRetryResult, NativeCloseApplicationError> {
        if !self.context.is_coordinator
            || self.context.is_sub_agent
            || self
                .fatal_external_effect_cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(NativeCloseApplicationError::SourceRefused);
        }
        self.check_native_close_retry_backend(backend.db())?;
        let command = retry_command_from_executing_source(
            &self.state,
            &self.context.conversation_id,
            &source,
            target,
            attempt_id,
            failed_run_ordinal,
        )
        .map_err(|_| NativeCloseApplicationError::SourceRefused)?;

        // The retained receipt is resolved before any live retry assessment. The
        // backend is already bound to this actor's database authority above.
        if let Some(admitted_run) = backend
            .db()
            .read_native_close_explicit_retry_receipt(&command)
            .await
            .map_err(|_| NativeCloseApplicationError::Storage)?
        {
            return Ok(NativeCloseRetryResult::Replay(
                phoenix_db::native_close::NativeCloseReceipt {
                    request_id: command.request_id(),
                    admitted: admitted_run,
                },
            ));
        }

        let NativeCloseIntent::ExplicitRetry {
            attempt_id,
            failed_run_ordinal,
            ..
        } = command.intent()
        else {
            return Err(NativeCloseApplicationError::SourceRefused);
        };
        let failed_run = CloseRunRef {
            attempt_id: attempt_id.clone(),
            ordinal: *failed_run_ordinal,
        };
        let original = backend
            .assess_native_close_retry(&failed_run)
            .await
            .map_err(|_| NativeCloseApplicationError::Storage)?;
        let request = AdmitCloseSafeRetryRequest {
            failed_run,
            requested_by: CloseRetryRequestedBy::User,
            observed_at_us: chrono::Utc::now().timestamp_micros(),
            precondition_resolution:
                "explicit retry source and current retirement state revalidated".into(),
            safety_evidence: original.safety_evidence.clone(),
            remaining_effects: original.remaining_effects.clone(),
        };
        let assessment = NativeCloseRetryAssessment {
            command: command.clone(),
            original,
            request,
        };

        let mut authority = RuntimeSettlementAuthority {
            owner: Some(admitted),
            abandoned_reason: "native Close retry admission or execution owner disappeared",
        };
        match backend
            .db()
            .apply_native_close_explicit_retry(&assessment)
            .await
        {
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(NativeCloseApplicationError::FatalAuthority)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(_)) => {
                authority.owner = None;
                Err(NativeCloseApplicationError::Storage)
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::Replay(
                receipt,
            ))) => {
                authority.owner = None;
                Ok(NativeCloseRetryResult::Replay(receipt))
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(
                NativeCloseRetryResult::NewlyAdmitted(receipt),
            )) => {
                match backend
                    .execute_native_close_retry(&command, &receipt.admitted)
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(())) => {
                        authority.owner = None;
                        Ok(NativeCloseRetryResult::NewlyAdmitted(receipt))
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(_)) => {
                        authority.owner = None;
                        Err(NativeCloseApplicationError::Storage)
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
            }
        }
    }
}
