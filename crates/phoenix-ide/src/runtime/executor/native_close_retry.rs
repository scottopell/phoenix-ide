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

#[derive(Debug)]
pub(crate) enum NativeRetryExecutionError {
    ResourceFailure(String),
    TerminalizedFailure(String),
    Unclassified,
}

mod admitted;
#[cfg(test)]
pub(crate) mod test_hooks;
pub(super) use admitted::AdmittedRetryCursor;

pub(super) struct NativeCloseRetryAssessment {
    command: NativeCloseCommand,
    original: crate::runtime::close_retirement::NativeCloseRetryAssessment,
    request: AdmitCloseSafeRetryRequest,
}

pub(super) enum NativeCloseRetryOriginAssessment {
    Anchored(NativeCloseRetryAssessment),
    CapturedLoss(
        crate::runtime::close_retirement::captured_loss_retry::CapturedLossRetryAssessment,
    ),
}

pub(super) struct NativeCloseRetryContinuation {
    command: NativeCloseCommand,
    request: crate::runtime::native_close_application::NativeCloseRetryRequest,
    worker: tokio::task::JoinHandle<
        Result<NativeCloseRetryOriginAssessment, NativeCloseApplicationError>,
    >,
}

impl NativeCloseRetryContinuation {
    pub(super) async fn ready(
        &mut self,
    ) -> Result<NativeCloseRetryOriginAssessment, NativeCloseApplicationError> {
        match (&mut self.worker).await {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(%error, "native Close retry assessment worker failed");
                Err(NativeCloseApplicationError::Storage)
            }
        }
    }

    pub(super) async fn join_before_exit(mut self) {
        // Do not abort: assessment may itself be awaiting blocking read-only probes.
        let _ = self.ready().await;
        let _ = self
            .request
            .response
            .send(Err(NativeCloseApplicationError::SourceRefused));
    }
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
    pub(super) async fn begin_native_close_retry(
        &mut self,
        request: crate::runtime::native_close_application::NativeCloseRetryRequest,
    ) -> Option<NativeCloseRetryContinuation> {
        let command = (|| {
            if !self.context.is_coordinator
                || self.context.is_sub_agent
                || self
                    .fatal_external_effect_cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
            {
                return Err(NativeCloseApplicationError::SourceRefused);
            }
            self.check_native_close_backend(request.backend.db())?;
            retry_command_from_executing_source(
                &self.state,
                &self.context.conversation_id,
                &request.source,
                request.target.clone(),
                request.attempt_id.clone(),
                request.failed_run_ordinal,
            )
            .map_err(|_| NativeCloseApplicationError::SourceRefused)
        })();
        let command = match command {
            Ok(command) => command,
            Err(error) => {
                let _ = request.response.send(Err(error));
                return None;
            }
        };
        match request
            .backend
            .db()
            .read_native_close_explicit_retry_receipt(&command)
            .await
        {
            Ok(Some(admitted)) => {
                let _ = request.response.send(Ok(NativeCloseRetryResult::Replay(
                    phoenix_db::native_close::NativeCloseReceipt {
                        request_id: command.request_id(),
                        admitted,
                    },
                )));
                return None;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "native Close retry receipt lookup failed");
                let _ = request
                    .response
                    .send(Err(NativeCloseApplicationError::Storage));
                return None;
            }
        }
        let backend = Arc::clone(&request.backend);
        let exact_command = command.clone();
        let runtime = tokio::runtime::Handle::current();
        #[cfg(test)]
        let hooks = test_hooks::Hooks::current();
        let worker = tokio::task::spawn_blocking(move || {
            runtime.block_on(async {
                let assessment = assess_retry_origin(&backend, &exact_command).await;
                #[cfg(test)]
                if matches!(&assessment, Ok(NativeCloseRetryOriginAssessment::CapturedLoss(_))) {
                    test_hooks::checkpoint(
                        hooks.as_ref(),
                        test_hooks::Point::CapturedLossAssessmentReady,
                    )
                    .await;
                }
                assessment
            })
        });
        Some(NativeCloseRetryContinuation {
            command,
            request,
            worker,
        })
    }

    pub(super) async fn finish_native_close_retry(
        &mut self,
        continuation: NativeCloseRetryContinuation,
        assessment: Result<NativeCloseRetryOriginAssessment, NativeCloseApplicationError>,
    ) -> (bool, Option<AdmittedRetryCursor>) {
        if !super::native_close::source_entry::source_matches(&self.state, &continuation.command) {
            let _ = continuation
                .request
                .response
                .send(Err(NativeCloseApplicationError::SourceRefused));
            return (false, None);
        }
        let assessment = match assessment {
            Ok(assessment) => assessment,
            Err(error) => {
                let _ = continuation.request.response.send(Err(error));
                return (false, None);
            }
        };
        let NativeCloseRetryContinuation { command, request, .. } = continuation;
        let crate::runtime::native_close_application::NativeCloseRetryRequest {
            backend, admitted, target_gate, attempt_gate, response, ..
        } = request;
        // Admission is still source-actor owned and follows queued Stop events.
        let mut authority = RuntimeSettlementAuthority {
            owner: Some(admitted),
            abandoned_reason: "native retry admission lost settlement authority",
        };
        let admission = match &assessment {
            NativeCloseRetryOriginAssessment::Anchored(assessment) => backend.db().apply_native_close_explicit_retry(assessment).await,
            NativeCloseRetryOriginAssessment::CapturedLoss(assessment) => backend.db().apply_native_close_captured_loss_retry(assessment).await,
        };
        match admission {
            LocalAuthorityResult::DurableFactUnclassified => {
                let _ = response.send(Err(NativeCloseApplicationError::FatalAuthority));
                (true, None)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                tracing::warn!(%error, "native retry admission refused");
                authority.owner = None;
                let _ = response.send(Err(NativeCloseApplicationError::Storage));
                (false, None)
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::Replay(receipt))) => {
                authority.owner = None;
                let _ = response.send(Ok(NativeCloseRetryResult::Replay(receipt)));
                (false, None)
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(NativeCloseRetryResult::NewlyAdmitted(receipt))) => {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
                let mut controller = super::native_close::source_entry::SourceEntryController::new(command.clone(), deadline);
                let _ = controller.revalidate(&self.state, &command);
                let worker = controller.worker();
                self.native_close_source_entry = Some(controller);
                let owner = authority.owner.take().expect("admission retained authority");
                (false, Some(AdmittedRetryCursor::new(command, receipt, backend, owner, target_gate, attempt_gate, response, worker, deadline.into_std())))
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn handle_native_close_retry(
        &mut self,
        request: crate::runtime::native_close_application::NativeCloseRetryRequest,
    ) -> bool {
        self.handle_native_close_retry_prepared(request, None).await
    }

    async fn handle_native_close_retry_prepared(
        &mut self,
        request: crate::runtime::native_close_application::NativeCloseRetryRequest,
        prepared: Option<(NativeCloseCommand, NativeCloseRetryOriginAssessment)>,
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
                prepared,
            )
            .await;
        let fatal = matches!(result, Err(NativeCloseApplicationError::FatalAuthority));
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
    /// Runs the retry solely in the acknowledged source actor; gate ownership
    /// transferred with the request is retained through durable classification.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn execute_native_close_retry(
        &mut self,
        source: SourceToolCall,
        target: ProductConversationId,
        attempt_id: CloseAttemptId,
        failed_run_ordinal: CloseRunOrdinal,
        backend: Arc<crate::runtime::RuntimeManager>,
        admitted: crate::runtime::AdmittedOperation,
        _target_gate: tokio::sync::OwnedMutexGuard<()>,
        _attempt_gate: tokio::sync::OwnedMutexGuard<()>,
        prepared: Option<(NativeCloseCommand, NativeCloseRetryOriginAssessment)>,
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
        self.check_native_close_backend(backend.db())?;
        let command = retry_command_from_executing_source(
            &self.state,
            &self.context.conversation_id,
            &source,
            target,
            attempt_id,
            failed_run_ordinal,
        )
        .map_err(|_| NativeCloseApplicationError::SourceRefused)?;

        // Recheck receipts after assessment; the actor may have serviced events.
        if let Some(admitted_run) = backend
            .db()
            .read_native_close_explicit_retry_receipt(&command)
            .await
            .map_err(|error| {
                tracing::warn!(%error, "native Close retry receipt lookup failed");
                #[cfg(test)]
                eprintln!("native Close retry receipt lookup failed: {error}");
                NativeCloseApplicationError::Storage
            })?
        {
            return Ok(NativeCloseRetryResult::Replay(
                phoenix_db::native_close::NativeCloseReceipt {
                    request_id: command.request_id(),
                    admitted: admitted_run,
                },
            ));
        }

        let assessment = match prepared {
            Some((exact_command, assessment)) if exact_command == command => assessment,
            Some(_) => return Err(NativeCloseApplicationError::SourceRefused),
            None => assess_retry_origin(&backend, &command).await?,
        };
        if self
            .fatal_external_effect_cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(NativeCloseApplicationError::SourceRefused);
        }

        let mut authority = RuntimeSettlementAuthority {
            owner: Some(admitted),
            abandoned_reason: "native Close retry admission or execution owner disappeared",
        };
        let admission = match &assessment {
            NativeCloseRetryOriginAssessment::Anchored(assessment) => {
                backend
                    .db()
                    .apply_native_close_explicit_retry(assessment)
                    .await
            }
            NativeCloseRetryOriginAssessment::CapturedLoss(assessment) => {
                backend
                    .db()
                    .apply_native_close_captured_loss_retry(assessment)
                    .await
            }
        };
        match admission {
            LocalAuthorityResult::DurableFactUnclassified => {
                Err(NativeCloseApplicationError::FatalAuthority)
            }
            LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                tracing::warn!(%error, "native Close retry admission failed");
                #[cfg(test)]
                eprintln!("native Close retry admission failed: {error}");
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

async fn assess_retry_origin(
    backend: &crate::runtime::RuntimeManager,
    command: &NativeCloseCommand,
) -> Result<NativeCloseRetryOriginAssessment, NativeCloseApplicationError> {
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
    let resources = backend
        .db()
        .list_close_expected_retirement_resources(failed_run.attempt_id.as_str())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "native Close retry origin lookup failed");
            NativeCloseApplicationError::Storage
        })?;
    let assessment = if resources.is_empty() {
        NativeCloseRetryOriginAssessment::CapturedLoss(
            backend
                .assess_native_close_captured_loss_retry(command)
                .await
                .map_err(|error| {
                    tracing::warn!(%error, "native Close captured-loss retry assessment failed");
                    NativeCloseApplicationError::Storage
                })?,
        )
    } else {
        let original = backend
            .assess_native_close_retry(&failed_run)
            .await
            .map_err(|error| {
                tracing::warn!(%error, "native Close retry assessment failed");
                NativeCloseApplicationError::Storage
            })?;
        let request = AdmitCloseSafeRetryRequest {
            failed_run,
            requested_by: CloseRetryRequestedBy::User,
            observed_at_us: chrono::Utc::now().timestamp_micros(),
            precondition_resolution:
                "explicit retry source and current retirement state revalidated".into(),
            safety_evidence: original.safety_evidence.clone(),
            remaining_effects: original.remaining_effects.clone(),
        };
        NativeCloseRetryOriginAssessment::Anchored(NativeCloseRetryAssessment {
            command: command.clone(),
            original,
            request,
        })
    };
    Ok(assessment)
}
