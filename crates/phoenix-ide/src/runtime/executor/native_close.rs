//! Source-owned initial Close admission and bounded runtime-process retirement.
//! Continues original Worktree/scope cleanup and completion under the same source.

mod post_retirement;
mod process_retirement;
pub(super) mod source_entry;

use super::{
    CancellationToken, ConversationRuntime, LlmClient, RuntimeSettlementAuthority, Storage,
    ToolExecutor,
};
use crate::db::CaptureCloseRetirementInventoryRequest;
use crate::runtime::close_retirement::{
    collect_native_initial_inspection, CloseRetirementInventoryPreparationError,
    NativeInitialInspection, NativeInitialInspectionFailure,
};
use crate::runtime::native_close_application::{
    initial_command_from_executing_source, NativeCloseApplicationError,
};
use phoenix_core::domain::db_schema::SourceToolCall;
use phoenix_core::domain::product_conversation::ProductConversationId;
use phoenix_db::native_close::NativeCloseInitialResult;
use phoenix_db::workflow::LocalAuthorityResult;

fn terminal_run_outcome_is_compatible(
    status: phoenix_core::domain::close::CloseRunStatus,
    outcome: phoenix_core::domain::close::CloseCompletionOutcome,
) -> bool {
    use phoenix_core::domain::close::{CloseCompletionOutcome as Outcome, CloseRunStatus};

    match status {
        CloseRunStatus::Completed => matches!(outcome, Outcome::Archived | Outcome::Cancelled),
        // Stopped runs can have a completed failed/cancelled obligation when the
        // durable Close work finished before the run itself was stopped.
        CloseRunStatus::Stopped => true,
        CloseRunStatus::Running => false,
    }
}

/// Refuse queued work without resuming ordinary actor execution during Close exit.
pub(super) fn refuse_during_exit(request: crate::runtime::AcknowledgedRuntimeRequest) {
    use crate::runtime::AcknowledgedRuntimeRequest as Request;
    match request {
        Request::Event(request) => {
            let _ = request
                .acknowledgement
                .send(Err("runtime is exiting".into()));
        }
        Request::ExecuteNativeCloseInitial(request) => {
            let _ = request
                .response
                .send(Err(NativeCloseApplicationError::Unavailable));
        }
        Request::ExecuteNativeCloseObserve(request) => {
            let _ = request
                .response
                .send(Err(NativeCloseApplicationError::Unavailable));
        }
        Request::ExecuteNativeCloseRetry(request) => {
            let _ = request
                .response
                .send(Err(NativeCloseApplicationError::Unavailable));
        }
    }
}

/// One source-owned initial Close continuation. Its authority and target gate
/// remain live while the event loop advances the exact admitted receipt.
pub(super) struct NativeCloseInitialContinuation {
    #[cfg(test)]
    pub(super) settlement_entry_observer: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    pub(super) command: phoenix_db::native_close::NativeCloseCommand,
    pub(super) receipt: phoenix_db::native_close::NativeCloseReceipt,
    pub(super) admitted_result: NativeCloseInitialResult,
    pub(super) db: phoenix_db::Database,
    pub(super) phase: NativeCloseInitialContinuationPhase,
    inspection: Option<
        tokio::task::JoinHandle<Result<NativeInitialInspection, NativeInitialInspectionFailure>>,
    >,
    inventory: InventoryCapture,
    process_retirement: Option<process_retirement::ProcessRetirement>,
    post_retirement: Option<post_retirement::PostRetirement>,
    source_entry: Option<source_entry::SourceEntryWorker>,
    original_entry_deadline: Option<std::time::Instant>,
    exit_custody_deadline: Option<std::time::Instant>,
    backend: std::sync::Arc<crate::runtime::RuntimeManager>,
    #[cfg(test)]
    inventory_preparation_test_barrier: Option<InventoryPreparationTestBarrier>,
    #[cfg(test)]
    inspection_collection_test_barrier: Option<InspectionCollectionTestBarrier>,
    inspection_failure: Option<NativeInitialInspectionFailureIntent>,
    loss_terminalization: Option<NativeInitialLossTerminalizationIntent>,
    pub(super) terminal_observation_retry: bool,
    pub(super) authority: RuntimeSettlementAuthority,
    pub(super) target_gate: std::sync::Arc<tokio::sync::OwnedMutexGuard<()>>,
    pub(super) response: Option<
        tokio::sync::oneshot::Sender<Result<NativeCloseInitialResult, NativeCloseApplicationError>>,
    >,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeCloseInitialContinuationPhase {
    Prepare,
    Settle,
    WaitingForQuiescence,
    ReachedSettlementPhase(phoenix_core::domain::close::ClosePhase),
    PreparingInventory,
    Captured,
    RetiringProcesses,
    ProcessRetirementBlocked,
    RuntimeProcessesDone,
    AwaitingStopWorkConfirmation,
    CollectingInspection,
    InspectionRequired,
    ClassifyingInspectionFailure,
    ClassifyingLosses,
    ObservingTerminal,
    ExitObservationOnly,
    ClassifyingExitInterruption,
    Finished,
}

#[derive(Debug, Clone)]
struct NativeInitialInspectionFailureIntent {
    scope: phoenix_core::work_scope::WorkScopeId,
    reason: phoenix_core::domain::close::RetirementFailureReason,
    detail: String,
}

#[derive(Debug, Clone)]
struct NativeInitialLossTerminalizationIntent {
    request: phoenix_db::ReplaceCloseInspectionRequest,
    empty_generation: Option<String>,
}

enum InventoryCapture {
    NotStarted,
    Preparing(
        tokio::task::JoinHandle<
            Result<
                CaptureCloseRetirementInventoryRequest,
                CloseRetirementInventoryPreparationError,
            >,
        >,
    ),
    Held,
}

#[cfg(test)]
struct InspectionCollectionTestBarrier {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
    exited: tokio::sync::oneshot::Sender<()>,
}

#[cfg(test)]
struct InventoryPreparationTestBarrier {
    entered: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
    exited: tokio::sync::oneshot::Sender<()>,
}

#[cfg(test)]
struct InventoryPreparationWorkerExit(Option<tokio::sync::oneshot::Sender<()>>);

#[cfg(test)]
impl Drop for InventoryPreparationWorkerExit {
    fn drop(&mut self) {
        if let Some(exited) = self.0.take() {
            let _ = exited.send(());
        }
    }
}

impl InventoryCapture {
    fn abort_owned_worker(&mut self) {
        if let Self::Preparing(handle) = self {
            handle.abort();
        }
    }
}

pub(super) enum NativeCloseInitialAdmission {
    Finished {
        _result: NativeCloseInitialResult,
    },
    Continued {
        _result: NativeCloseInitialResult,
        continuation: Box<NativeCloseInitialContinuation>,
    },
}

impl Drop for NativeCloseInitialContinuation {
    fn drop(&mut self) {
        if let Some(handle) = self.inspection.take() {
            handle.abort();
        }
        self.inventory.abort_owned_worker();
        if let Some(progress) = &self.process_retirement {
            progress.abort();
        }
    }
}

impl NativeCloseInitialContinuation {
    /// Opens eligibility only from the actor's actual source and the capture's
    /// original deadline, before any original process or Worktree effect.
    pub(super) async fn ensure_source_entry(
        &mut self,
        controller: &mut Option<source_entry::SourceEntryController>,
        live_state: &phoenix_core::domain::sm_state::ConvState,
    ) -> Result<(), NativeCloseApplicationError> {
        if self.phase != NativeCloseInitialContinuationPhase::RetiringProcesses
            || self.source_entry.is_some()
            || !self
                .process_retirement
                .as_ref()
                .is_some_and(process_retirement::ProcessRetirement::has_original_effects)
        {
            return Ok(());
        }
        let (scopes, snapshot) = self
            .process_retirement
            .as_ref()
            .expect("guarded cursor")
            .captured_binding();
        let Ok(deadline) = self
            .backend
            .native_initial_original_entry_deadline(
                self.receipt.admitted.attempt_id.as_str(),
                scopes,
                snapshot,
            )
            .await
        else {
            self.phase = NativeCloseInitialContinuationPhase::ProcessRetirementBlocked;
            return Err(NativeCloseApplicationError::FatalAuthority);
        };
        // Preserve the exact capture deadline for post-process workers, never
        // refresh it when handing off to physical Worktree retirement.
        self.original_entry_deadline = Some(deadline.into_std());
        let mut original = source_entry::SourceEntryController::new(self.command.clone(), deadline);
        if original.revalidate(live_state, &self.command) != source_entry::ActorEntryResult::Updated
        {
            // A refusal is terminal for this eligibility cell, never a hot loop
            // constructing another controller for the same immutable command.
            self.source_entry = Some(original.worker());
            *controller = Some(original);
            self.phase = NativeCloseInitialContinuationPhase::InspectionRequired;
            return Err(NativeCloseApplicationError::SourceRefused);
        }
        // Dropping a previous actor-owned controller revokes its worker handles.
        self.source_entry = Some(original.worker());
        *controller = Some(original);
        Ok(())
    }

    /// Releases the source owner only after exact durable terminal evidence is observed.
    /// A failed or unrelated read leaves the continuation and its guards untouched.
    pub(super) async fn release_if_exactly_terminal(&mut self) -> bool {
        use phoenix_core::domain::close::{ClosePhase, CloseRunRef};

        let run = CloseRunRef {
            attempt_id: self.receipt.admitted.attempt_id.clone(),
            ordinal: self.receipt.admitted.run_ordinal,
        };
        let command_target_matches = matches!(
            self.command.intent(),
            phoenix_db::native_close::NativeCloseIntent::Initial { target }
                if target == &self.receipt.admitted.target
        );
        if self.command.request_id() != self.receipt.request_id || !command_target_matches {
            return false;
        }
        self.terminal_observation_retry = true;
        let Ok(observed_run) = self.db.get_close_run(&run).await else {
            return false;
        };
        let Ok(obligation) = self.db.get_close_obligation(run.attempt_id.as_str()).await else {
            return false;
        };
        if observed_run.run != run
            || obligation.attempt_id() != &run.attempt_id
            || obligation.product_conversation_id() != &self.receipt.admitted.target
            || obligation.phase() != ClosePhase::Completed
            || !obligation.close_outcome().is_some_and(|outcome| {
                terminal_run_outcome_is_compatible(observed_run.status, outcome)
            })
        {
            return false;
        }

        self.abort_and_join_inventory_preparation().await;
        match self.poll_exit_custody().await {
            post_retirement::ExitCustodyPoll::Classified => {}
            post_retirement::ExitCustodyPoll::Pending => return false,
            post_retirement::ExitCustodyPoll::Fatal => {
                self.backend
                    .fatal_local_authority_fence
                    .close("native_close_initial_terminal_custody_unclassified");
                return false;
            }
        }
        self.terminal_observation_retry = false;
        self.authority.owner = None;
        if let Some(response) = self.response.take() {
            let _ = response.send(Ok(self.admitted_result.clone()));
        }
        true
    }

    /// Abort inert preparation while retaining every handle for bounded custody
    /// classification. Resource permits stay manager-owned and fenced after a failed Close.
    pub(super) async fn abort_and_join_inventory_preparation(&mut self) {
        // Abort only inert preparation; retain every handle until bounded custody
        // polling classifies its result. Aborting a JoinHandle is not a join.
        if let Some(progress) = &self.process_retirement {
            progress.abort();
        }
        if let InventoryCapture::Preparing(handle) = &self.inventory {
            handle.abort();
        }
    }

    async fn poll_exit_custody(&mut self) -> post_retirement::ExitCustodyPoll {
        // Original physical entry keeps its captured budget. Read-only work
        // started before capture gets one bounded exit budget, never rearmed.
        let deadline = Some(*self.exit_custody_deadline.get_or_insert_with(|| {
            self.original_entry_deadline.unwrap_or_else(|| {
                (tokio::time::Instant::now() + super::CANCELLATION_DEADLINE).into_std()
            })
        }));
        if let Some(inspection) = self.inspection.as_ref() {
            if !inspection.is_finished() {
                return if deadline.is_none_or(|deadline| std::time::Instant::now() >= deadline) {
                    post_retirement::ExitCustodyPoll::Fatal
                } else {
                    post_retirement::ExitCustodyPoll::Pending
                };
            }
            let inspection = self.inspection.take().expect("checked inspection");
            if inspection.await.is_err() {
                return post_retirement::ExitCustodyPoll::Fatal;
            }
        }
        if let InventoryCapture::Preparing(handle) = &self.inventory {
            if !handle.is_finished() {
                return if deadline.is_none_or(|deadline| std::time::Instant::now() >= deadline) {
                    post_retirement::ExitCustodyPoll::Fatal
                } else {
                    post_retirement::ExitCustodyPoll::Pending
                };
            }
            let InventoryCapture::Preparing(handle) =
                std::mem::replace(&mut self.inventory, InventoryCapture::NotStarted)
            else {
                unreachable!();
            };
            if let Err(error) = handle.await {
                if !error.is_cancelled() {
                    return post_retirement::ExitCustodyPoll::Fatal;
                }
            }
        }
        if let Some(processes) = self.process_retirement.as_mut() {
            match processes.poll_exit_custody(deadline).await {
                post_retirement::ExitCustodyPoll::Classified => {}
                other => return other,
            }
        }
        if let Some(post_retirement) = self.post_retirement.as_mut() {
            return post_retirement.poll_exit_custody(deadline).await;
        }
        post_retirement::ExitCustodyPoll::Classified
    }

    pub(super) fn collecting_inspection(&self) -> bool {
        self.phase == NativeCloseInitialContinuationPhase::CollectingInspection
    }

    pub(super) fn freeze_for_exit_observation(&mut self) {
        if let Some(progress) = &self.process_retirement {
            progress.abort();
        }
        // Keep the read-only collector handle owned until exit custody polling
        // consumes its result or the fatal fence retains this continuation.
        if let InventoryCapture::Preparing(handle) = &self.inventory {
            handle.abort();
        }
        self.phase = NativeCloseInitialContinuationPhase::ExitObservationOnly;
        self.terminal_observation_retry = true;
    }

    /// Exit settlement is a distinct admitted operation, not permission to
    /// resume preparation, inspection, retirement, or source-bound completion.
    pub(super) fn begin_exit_interruption(&mut self) {
        self.freeze_for_exit_observation();
        self.phase = NativeCloseInitialContinuationPhase::ClassifyingExitInterruption;
    }

    pub(super) fn exit_observation_only(&self) -> bool {
        self.phase == NativeCloseInitialContinuationPhase::ExitObservationOnly
    }

    pub(super) fn requires_periodic_step(&self) -> bool {
        matches!(
            self.phase,
            NativeCloseInitialContinuationPhase::CollectingInspection
                | NativeCloseInitialContinuationPhase::WaitingForQuiescence
                | NativeCloseInitialContinuationPhase::ReachedSettlementPhase(
                    phoenix_core::domain::close::ClosePhase::RetirementRequested
                )
                | NativeCloseInitialContinuationPhase::PreparingInventory
                | NativeCloseInitialContinuationPhase::Captured
                | NativeCloseInitialContinuationPhase::RetiringProcesses
                | NativeCloseInitialContinuationPhase::RuntimeProcessesDone
                | NativeCloseInitialContinuationPhase::ClassifyingInspectionFailure
                | NativeCloseInitialContinuationPhase::ClassifyingLosses
                | NativeCloseInitialContinuationPhase::ObservingTerminal
                | NativeCloseInitialContinuationPhase::ClassifyingExitInterruption
        )
    }

    pub(super) async fn observe_terminal_if_required(&mut self) -> bool {
        self.terminal_observation_retry && self.release_if_exactly_terminal().await
    }

    pub(super) fn next_service_at(&self) -> tokio::time::Instant {
        if self.phase == NativeCloseInitialContinuationPhase::Captured {
            return tokio::time::Instant::now() + std::time::Duration::from_millis(1);
        }
        if self.phase == NativeCloseInitialContinuationPhase::RetiringProcesses {
            return tokio::time::Instant::now()
                + self
                    .process_retirement
                    .as_ref()
                    .expect("runtime cursor retained")
                    .service_delay();
        }
        if self.phase == NativeCloseInitialContinuationPhase::RuntimeProcessesDone {
            return tokio::time::Instant::now() + std::time::Duration::from_millis(1);
        }
        let delay_ms = if self.collecting_inspection()
            || self.phase == NativeCloseInitialContinuationPhase::PreparingInventory
        {
            100
        } else {
            250
        };
        tokio::time::Instant::now() + std::time::Duration::from_millis(delay_ms)
    }

    pub(super) fn needs_service(&self, step_ready: bool) -> bool {
        self.terminal_observation_retry || step_ready || self.requires_periodic_step()
    }

    pub(super) fn should_step(&self, step_ready: bool) -> bool {
        !self.exit_observation_only() && (step_ready || self.requires_periodic_step())
    }

    #[cfg(test)]
    pub(super) async fn await_inspection_completion_for_test(&mut self) {
        let handle = self
            .inspection
            .take()
            .expect("inspection must be collecting");
        let result = handle.await;
        self.inspection = Some(tokio::spawn(async move {
            result.expect("collector task must finish")
        }));
        // The collector's actual result was awaited above; only schedule the completed
        // replacement handle so the ordinary actor step can consume it unchanged.
        while !self.inspection.as_ref().unwrap().is_finished() {
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(super) fn set_inventory_preparation_test_barrier(
        &mut self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
        exited: tokio::sync::oneshot::Sender<()>,
    ) {
        self.inventory_preparation_test_barrier = Some(InventoryPreparationTestBarrier {
            entered,
            release,
            exited,
        });
    }

    #[cfg(test)]
    pub(super) fn set_inspection_collection_test_barrier(
        &mut self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
        exited: tokio::sync::oneshot::Sender<()>,
    ) {
        self.inspection_collection_test_barrier = Some(InspectionCollectionTestBarrier {
            entered,
            release,
            exited,
        });
    }

    fn live_source_matches(&self, live_source: &phoenix_core::domain::sm_state::ConvState) -> bool {
        let phoenix_db::native_close::NativeCloseIntent::Initial { target } = self.command.intent()
        else {
            return false;
        };
        initial_command_from_executing_source(
            live_source,
            self.command.source_transcript_id(),
            self.command.source(),
            target.clone(),
        )
        .is_ok_and(|command| command == self.command)
    }

    /// Settle only the response observer. Revocation does not release the
    /// admitted owner, target gate, or independently supervised physical work.
    fn settle_source_refused(&mut self) {
        if let Some(response) = self.response.take() {
            let _ = response.send(Err(NativeCloseApplicationError::SourceRefused));
        }
    }

    pub(super) fn abort_inspection_if_source_revoked(
        &mut self,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) {
        if self.live_source_matches(live_source) {
            return;
        }
        // Also refuse a revoked observer between phase workers, before exact
        // durable-terminal observation can settle it as an admission success.
        self.settle_source_refused();
        if self.collecting_inspection()
            || matches!(self.inventory, InventoryCapture::Preparing(_))
            || self.process_retirement.is_some()
        {
            // Keep the collector handle: it may currently await a nested
            // spawn_blocking inspection, and source revocation is not custody release.
            if let InventoryCapture::Preparing(handle) = &self.inventory {
                handle.abort();
            }
            if let Some(progress) = &self.process_retirement {
                progress.abort();
            }
            self.phase = NativeCloseInitialContinuationPhase::InspectionRequired;
            tracing::warn!("native Close inspection result invalidated after source revocation; retaining owner for exact terminal classification");
        }
    }

    async fn step_post_retirement(
        &mut self,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> Result<NativeCloseInitialContinuationPhase, NativeCloseApplicationError> {
        if !self.live_source_matches(live_source) {
            self.phase = NativeCloseInitialContinuationPhase::InspectionRequired;
            self.settle_source_refused();
            return Err(NativeCloseApplicationError::SourceRefused);
        }
        if self.post_retirement.is_none() {
            let Some(processes) = self.process_retirement.as_ref() else {
                self.phase = NativeCloseInitialContinuationPhase::ProcessRetirementBlocked;
                tracing::error!(
                    "native Close reached post-process phase without captured inventory"
                );
                return Ok(self.phase);
            };
            let (scopes, snapshot) = processes.captured_binding();
            match post_retirement::PostRetirement::new(
                processes.captured_inventory().to_vec(),
                scopes.to_vec(),
                snapshot.clone(),
            ) {
                Ok(cursor) => self.post_retirement = Some(cursor),
                Err(detail) => {
                    self.phase = NativeCloseInitialContinuationPhase::ProcessRetirementBlocked;
                    tracing::error!(%detail, "native Close post-process cursor rejected captured inventory");
                    return Ok(self.phase);
                }
            }
        }
        match self
            .post_retirement
            .as_mut()
            .expect("cursor initialized")
            .step(
                &self.command,
                &self.receipt,
                &self.db,
                &self.backend,
                live_source,
                self.source_entry.as_ref(),
                self.original_entry_deadline,
            )
            .await
        {
            Ok(
                post_retirement::PostRetirementStep::Retiring
                | post_retirement::PostRetirementStep::Unclassified,
            ) => Ok(NativeCloseInitialContinuationPhase::RuntimeProcessesDone),
            Ok(post_retirement::PostRetirementStep::Completed(_)) => {
                self.phase = NativeCloseInitialContinuationPhase::ObservingTerminal;
                Ok(self.phase)
            }
            Ok(post_retirement::PostRetirementStep::Blocked) => {
                self.phase = NativeCloseInitialContinuationPhase::ProcessRetirementBlocked;
                Ok(self.phase)
            }
            Ok(post_retirement::PostRetirementStep::FatalAuthority) => {
                Err(NativeCloseApplicationError::FatalAuthority)
            }
            Err(error) => {
                tracing::warn!(%error, "native Close post-process boundary rolled back; retaining original cursor");
                Ok(NativeCloseInitialContinuationPhase::RuntimeProcessesDone)
            }
        }
    }

    /// Advances one source-owned boundary without sleeping in the actor. The
    /// read-only collector runs separately; its result is committed only here.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn step(
        &mut self,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> Result<NativeCloseInitialContinuationPhase, NativeCloseApplicationError> {
        use NativeCloseInitialContinuationPhase as Phase;
        if self
            .authority
            .owner
            .as_ref()
            .and_then(|owner| owner.fence.as_ref())
            .is_some_and(|fence| fence.fatal_authority_loss_is_latched())
        {
            self.abort_and_join_inventory_preparation().await;
            return Err(NativeCloseApplicationError::FatalAuthority);
        }
        if self.exit_observation_only() {
            return Ok(self.phase);
        }
        #[cfg(test)]
        if matches!(self.phase, Phase::Settle | Phase::WaitingForQuiescence) {
            if let Some(observer) = &self.settlement_entry_observer {
                let _ = observer.send(());
            }
        }
        let result = match self.phase {
            Phase::ClassifyingExitInterruption => {
                match self.poll_exit_custody().await {
                    post_retirement::ExitCustodyPoll::Pending => return Ok(self.phase),
                    post_retirement::ExitCustodyPoll::Fatal => {
                        self.backend.fatal_local_authority_fence.close(
                            "native_close_initial_exit_custody_unclassified",
                        );
                        return Err(NativeCloseApplicationError::FatalAuthority);
                    }
                    post_retirement::ExitCustodyPoll::Classified => {}
                }
                match self
                    .db
                    .interrupt_native_close_initial(&self.command, &self.receipt)
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(_)) => {
                        self.phase = Phase::ObservingTerminal;
                        if self.release_if_exactly_terminal().await {
                            self.phase = Phase::Finished;
                        }
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        tracing::warn!(%error, "native Close exit interruption definitely rolled back; retaining owner and retrying exit settlement only");
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
            }
            Phase::Prepare => match self
                .db
                .prepare_native_close_initial_execution(&self.command, &self.receipt, live_source)
                .await
            {
                LocalAuthorityResult::DurableFactEstablished(Ok(execution)) => {
                    self.phase = match execution.phase {
                        phoenix_core::domain::close::ClosePhase::SettlingActiveWork => {
                            Phase::Settle
                        }
                        phoenix_core::domain::close::ClosePhase::AwaitingStopWorkConfirmation => {
                            Phase::AwaitingStopWorkConfirmation
                        }
                        phase => Phase::ReachedSettlementPhase(phase),
                    };
                    Ok(self.phase)
                }
                LocalAuthorityResult::DurableFactEstablished(Err(_)) => {
                    self.phase = Phase::InspectionRequired;
                    Err(NativeCloseApplicationError::Storage)
                }
                LocalAuthorityResult::DurableFactUnclassified => {
                    Err(NativeCloseApplicationError::FatalAuthority)
                }
            },
            Phase::Settle | Phase::WaitingForQuiescence => match self
                .db
                .try_settle_native_close_initial(&self.command, &self.receipt, live_source)
                .await
            {
                LocalAuthorityResult::DurableFactEstablished(Ok(
                    phoenix_db::native_close::NativeCloseInitialSettlement::WaitingForQuiescence,
                )) => {
                    self.phase = Phase::WaitingForQuiescence;
                    Ok(self.phase)
                }
                LocalAuthorityResult::DurableFactEstablished(Ok(
                    phoenix_db::native_close::NativeCloseInitialSettlement::Advanced(execution),
                )) => {
                    self.phase = Phase::ReachedSettlementPhase(execution.phase);
                    Ok(self.phase)
                }
                LocalAuthorityResult::DurableFactEstablished(Err(_)) => {
                    self.phase = Phase::InspectionRequired;
                    Err(NativeCloseApplicationError::Storage)
                }
                LocalAuthorityResult::DurableFactUnclassified => {
                    Err(NativeCloseApplicationError::FatalAuthority)
                }
            },
            Phase::ReachedSettlementPhase(
                phoenix_core::domain::close::ClosePhase::RetirementRequested,
            ) => {
                if !self.live_source_matches(live_source) {
                    self.phase = Phase::InspectionRequired;
                    self.settle_source_refused();
                    return Err(NativeCloseApplicationError::SourceRefused);
                }
                let obligation = match self
                    .db
                    .get_close_obligation(self.receipt.admitted.attempt_id.as_str())
                    .await
                {
                    Ok(obligation) => obligation,
                    Err(error) => {
                        tracing::warn!(%error, "native Close retirement obligation lookup failed; parking source owner");
                        self.phase = Phase::InspectionRequired;
                        return Err(NativeCloseApplicationError::Storage);
                    }
                };
                let Some(snapshot) = obligation.snapshot().cloned() else {
                    tracing::warn!("native Close retirement obligation has no inventory snapshot; parking source owner");
                    self.phase = Phase::InspectionRequired;
                    return Err(NativeCloseApplicationError::Storage);
                };
                let backend = std::sync::Arc::clone(&self.backend);
                let attempt = self.receipt.admitted.attempt_id.clone();
                #[cfg(test)]
                let test_barrier = self.inventory_preparation_test_barrier.take();
                // The worker shares the already-admitted authority and exact gate.
                // Even if the actor itself is cancelled, aborting its JoinHandle
                // cannot release either boundary before the worker future is dropped.
                let worker_owner = self
                    .authority
                    .owner
                    .as_mut()
                    .expect("inventory preparation requires retained source authority")
                    .reborrow();
                let worker_gate = std::sync::Arc::clone(&self.target_gate);
                self.inventory = InventoryCapture::Preparing(tokio::spawn(async move {
                    let _custody = (worker_owner, worker_gate);
                    #[cfg(test)]
                    let _worker_exit = if let Some(barrier) = test_barrier {
                        let InventoryPreparationTestBarrier {
                            entered,
                            release,
                            exited,
                        } = barrier;
                        let worker_exit = InventoryPreparationWorkerExit(Some(exited));
                        let _ = entered.send(());
                        release
                            .await
                            .expect("inventory preparation test barrier must be released");
                        Some(worker_exit)
                    } else {
                        None
                    };
                    backend
                        .prepare_native_close_initial_retirement_inventory(attempt, snapshot)
                        .await
                }));
                self.phase = Phase::PreparingInventory;
                Ok(self.phase)
            }
            Phase::PreparingInventory => {
                if !self.live_source_matches(live_source) {
                    self.abort_inspection_if_source_revoked(live_source);
                    self.abort_and_join_inventory_preparation().await;
                    self.phase = Phase::InspectionRequired;
                    self.settle_source_refused();
                    return Err(NativeCloseApplicationError::SourceRefused);
                }
                match &self.inventory {
                    InventoryCapture::Preparing(handle) if handle.is_finished() => {}
                    _ => return Ok(self.phase),
                }
                let old = std::mem::replace(&mut self.inventory, InventoryCapture::NotStarted);
                let request = match old {
                    InventoryCapture::Preparing(handle) => match handle.await {
                        Ok(Ok(request)) => request,
                        Ok(Err(CloseRetirementInventoryPreparationError::CapturedScopeIdentity { scope, reason, detail })) => {
                            // The existing source-bound failure boundary accepts only
                            // AwaitingRetirementInspection, not RetirementRequested.
                            // Keep this exact scope evidence blocked, without forging
                            // an inspection failure in the wrong DB phase.
                            tracing::warn!(%scope, ?reason, %detail, "native Close inventory preparation blocked at retirement boundary");
                            self.phase = Phase::InspectionRequired;
                            return Err(NativeCloseApplicationError::Storage);
                        }
                        Ok(Err(CloseRetirementInventoryPreparationError::BackendStorageOrUnattributed { detail })) => {
                            tracing::error!(%detail, "native Close inventory preparation failed without scope evidence");
                            self.phase = Phase::InspectionRequired;
                            return Err(NativeCloseApplicationError::Storage);
                        }
                        Err(error) => {
                            tracing::error!(%error, "native Close inventory preparation worker failed");
                            self.phase = Phase::InspectionRequired;
                            return Err(NativeCloseApplicationError::Storage);
                        }
                    },
                    _ => unreachable!("preparing phase owns preparation worker"),
                };
                let scope_order = request
                    .scopes
                    .iter()
                    .map(|scope| scope.scope.clone())
                    .collect();
                let captured_snapshot = request.snapshot.clone();
                match self
                    .db
                    .capture_native_close_initial_retirement_inventory(
                        &self.command,
                        &self.receipt,
                        live_source,
                        request,
                    )
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(resources)) => {
                        self.process_retirement = Some(process_retirement::ProcessRetirement::new(
                            resources,
                            scope_order,
                            captured_snapshot,
                        ));
                        self.inventory = InventoryCapture::Held;
                        self.phase = Phase::Captured;
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        tracing::warn!(%error, "native Close inventory capture rejected; retaining lease custody and source owner");
                        self.phase = Phase::InspectionRequired;
                        Err(NativeCloseApplicationError::Storage)
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
            }
            Phase::ReachedSettlementPhase(
                phoenix_core::domain::close::ClosePhase::AwaitingRetirementInspection,
            ) => {
                if !self.live_source_matches(live_source) {
                    self.phase = Phase::InspectionRequired;
                    self.settle_source_refused();
                    return Err(NativeCloseApplicationError::SourceRefused);
                }
                let db = self.db.clone();
                let attempt_id = self.receipt.admitted.attempt_id.clone();
                #[cfg(test)]
                let test_barrier = self.inspection_collection_test_barrier.take();
                self.inspection = Some(tokio::spawn(async move {
                    #[cfg(test)]
                    let _worker_exit = if let Some(barrier) = test_barrier {
                        let InspectionCollectionTestBarrier { entered, release, exited } = barrier;
                        let worker_exit = InventoryPreparationWorkerExit(Some(exited));
                        let _ = entered.send(());
                        let _ = release.await;
                        Some(worker_exit)
                    } else {
                        None
                    };
                    collect_native_initial_inspection(&db, attempt_id).await
                }));
                self.phase = Phase::CollectingInspection;
                Ok(self.phase)
            }
            Phase::CollectingInspection => {
                if !self.live_source_matches(live_source) {
                    self.abort_inspection_if_source_revoked(live_source);
                    self.settle_source_refused();
                    return Err(NativeCloseApplicationError::SourceRefused);
                }
                if !self
                    .inspection
                    .as_ref()
                    .is_some_and(tokio::task::JoinHandle::is_finished)
                {
                    return Ok(self.phase);
                }
                let handle = self
                    .inspection
                    .take()
                    .expect("collecting inspection has owned handle");
                let collected = match handle.await {
                    Ok(Ok(collected)) => collected,
                    Ok(Err(NativeInitialInspectionFailure::Scope {
                        scope,
                        reason,
                        detail,
                    })) => {
                        tracing::warn!(%scope, ?reason, %detail, "native Close read-only inspection requires durable terminal classification");
                        self.inspection_failure = Some(NativeInitialInspectionFailureIntent {
                            scope,
                            reason,
                            detail,
                        });
                        self.phase = Phase::ClassifyingInspectionFailure;
                        return Ok(self.phase);
                    }
                    Ok(Err(NativeInitialInspectionFailure::Storage(detail))) => {
                        tracing::error!(%detail, "native Close inspection storage failure has no captured-scope authority; retaining blocked owner without fabricating classification");
                        self.phase = Phase::InspectionRequired;
                        return Err(NativeCloseApplicationError::Storage);
                    }
                    Err(error) => {
                        tracing::error!(%error, "native Close inspection worker join failure has no captured-scope authority; retaining blocked owner without fabricating classification");
                        self.phase = Phase::InspectionRequired;
                        return Err(NativeCloseApplicationError::Storage);
                    }
                };
                let loss_count: usize = collected
                    .request
                    .scopes
                    .iter()
                    .map(|scope| scope.losses.len())
                    .sum();
                if loss_count > 0 {
                    self.loss_terminalization = Some(NativeInitialLossTerminalizationIntent {
                        request: collected.request,
                        empty_generation: collected.empty_generation,
                    });
                    self.phase = Phase::ClassifyingLosses;
                    return Ok(self.phase);
                }
                match self
                    .db
                    .replace_native_close_initial_inspection(
                        &self.command,
                        &self.receipt,
                        live_source,
                        collected.request,
                        collected.empty_generation.as_deref(),
                    )
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(())) => {
                        let obligation = match self
                            .db
                            .get_close_obligation(self.receipt.admitted.attempt_id.as_str())
                            .await
                        {
                            Ok(obligation) => obligation,
                            Err(error) => {
                                tracing::warn!(%error, "initial inspection committed but its resulting phase could not be read");
                                self.phase = Phase::InspectionRequired;
                                return Err(NativeCloseApplicationError::Storage);
                            }
                        };
                        self.phase = Phase::ReachedSettlementPhase(obligation.phase());
                        tracing::info!(loss_count, ?self.phase, "native Close initial inspection persisted under exact live source; retirement remains pending");
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        tracing::warn!(%error, "native Close initial inspection replacement rejected; retaining repair-required owner");
                        self.phase = Phase::InspectionRequired;
                        Err(NativeCloseApplicationError::Storage)
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
            }
            Phase::ClassifyingInspectionFailure => {
                let failure = self
                    .inspection_failure
                    .as_ref()
                    .expect("classification phase owns typed failure intent")
                    .clone();
                match self
                    .db
                    .terminalize_native_close_initial_inspection_failure(
                        &self.command,
                        &self.receipt,
                        live_source,
                        &failure.scope,
                        failure.reason,
                        &failure.detail,
                    )
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(_obligation)) => {
                        self.inspection_failure = None;
                        self.phase = Phase::ObservingTerminal;
                        if self.release_if_exactly_terminal().await {
                            self.phase = Phase::Finished;
                        }
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        tracing::warn!(%error, "native Close failure classification definitely rolled back; retaining owner and retrying classification only");
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
            }
            Phase::ClassifyingLosses => {
                let intent = self
                    .loss_terminalization
                    .as_ref()
                    .expect("loss classification phase owns exact inspection intent");
                match self
                    .db
                    .terminalize_native_close_initial_losses(
                        &self.command,
                        &self.receipt,
                        live_source,
                        intent.request.clone(),
                        intent.empty_generation.as_deref(),
                        "initial inspection found retained content blockers; no retirement effects were run",
                    )
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(_obligation)) => {
                        self.loss_terminalization = None;
                        self.phase = Phase::ObservingTerminal;
                        if self.release_if_exactly_terminal().await {
                            self.phase = Phase::Finished;
                        }
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        tracing::warn!(%error, "native Close loss terminalization definitely rolled back; retaining owner and retrying terminalization only");
                        Ok(self.phase)
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
            }
            Phase::ObservingTerminal => {
                if self.release_if_exactly_terminal().await {
                    self.phase = Phase::Finished;
                }
                Ok(self.phase)
            }
            Phase::Captured => {
                self.phase = Phase::RetiringProcesses;
                Ok(self.phase)
            }
            Phase::RetiringProcesses => self.step_process_retirement(live_source).await,
            Phase::RuntimeProcessesDone => self.step_post_retirement(live_source).await,
            Phase::ReachedSettlementPhase(_)
            | Phase::ProcessRetirementBlocked
            | Phase::AwaitingStopWorkConfirmation
            | Phase::InspectionRequired
            | Phase::ExitObservationOnly
            | Phase::Finished => Ok(self.phase),
        };
        if matches!(&result, Err(NativeCloseApplicationError::FatalAuthority)) {
            self.abort_and_join_inventory_preparation().await;
        }
        result
    }
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    /// Production admission transfers authority only for a newly admitted receipt.
    /// Replays and classified errors complete as before and have no continuation.
    pub(super) async fn admit_native_close_initial(
        &mut self,
        backend: &std::sync::Arc<crate::runtime::RuntimeManager>,
        source: SourceToolCall,
        target: ProductConversationId,
        admitted: crate::runtime::AdmittedOperation,
        target_gate: tokio::sync::OwnedMutexGuard<()>,
        response: tokio::sync::oneshot::Sender<
            Result<NativeCloseInitialResult, NativeCloseApplicationError>,
        >,
    ) -> Result<NativeCloseInitialAdmission, NativeCloseApplicationError> {
        let mut authority = RuntimeSettlementAuthority {
            owner: Some(admitted),
            abandoned_reason: "native Close initial continuation owner disappeared",
        };
        let preflight = (|| -> Result<_, NativeCloseApplicationError> {
            self.check_native_close_backend(backend.db())?;
            if !self.context.is_coordinator
                || self.context.is_sub_agent
                || self
                    .fatal_external_effect_cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
            {
                return Err(NativeCloseApplicationError::SourceRefused);
            }
            let command = initial_command_from_executing_source(
                &self.state,
                &self.context.conversation_id,
                &source,
                target,
            )
            .map_err(|_| NativeCloseApplicationError::SourceRefused)?;
            let db = self
                .actor_database
                .as_ref()
                .ok_or(NativeCloseApplicationError::Unavailable)?;
            Ok((command, db))
        })();
        let (command, db) = match preflight {
            Ok(ready) => ready,
            Err(error) => {
                authority.owner = None;
                drop(target_gate);
                let _ = response.send(Err(error.clone()));
                return Err(error);
            }
        };
        match db.apply_native_close_initial(&command).await {
            LocalAuthorityResult::DurableFactEstablished(Ok(
                NativeCloseInitialResult::NewlyAdmitted(receipt),
            )) => {
                let result = NativeCloseInitialResult::NewlyAdmitted(receipt.clone());
                let continuation = NativeCloseInitialContinuation {
                    #[cfg(test)]
                    settlement_entry_observer: None,
                    command,
                    receipt,
                    admitted_result: result.clone(),
                    db: db.clone(),
                    phase: NativeCloseInitialContinuationPhase::Prepare,
                    inspection: None,
                    inventory: InventoryCapture::NotStarted,
                    process_retirement: None,
                    post_retirement: None,
                    source_entry: None,
                    original_entry_deadline: None,
                    exit_custody_deadline: None,
                    backend: std::sync::Arc::clone(backend),
                    #[cfg(test)]
                    inventory_preparation_test_barrier: None,
                    #[cfg(test)]
                    inspection_collection_test_barrier: None,
                    inspection_failure: None,
                    loss_terminalization: None,
                    terminal_observation_retry: false,
                    authority,
                    target_gate: std::sync::Arc::new(target_gate),
                    response: Some(response),
                };
                Ok(NativeCloseInitialAdmission::Continued {
                    _result: result,
                    continuation: Box::new(continuation),
                })
            }
            LocalAuthorityResult::DurableFactEstablished(Ok(result)) => {
                authority.owner = None;
                drop(target_gate);
                let _ = response.send(Ok(result.clone()));
                Ok(NativeCloseInitialAdmission::Finished { _result: result })
            }
            LocalAuthorityResult::DurableFactEstablished(Err(_)) => {
                authority.owner = None;
                drop(target_gate);
                let _ = response.send(Err(NativeCloseApplicationError::Storage));
                Err(NativeCloseApplicationError::Storage)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                drop(target_gate);
                let _ = response.send(Err(NativeCloseApplicationError::FatalAuthority));
                Err(NativeCloseApplicationError::FatalAuthority)
            }
        }
    }
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    /// Bind backend work to the actor's installed database authority, not its path.
    /// A cloned database handle preserves this identity; a separately opened pool does not.
    pub(super) fn check_native_close_backend(
        &self,
        backend_db: &phoenix_db::Database,
    ) -> Result<(), NativeCloseApplicationError> {
        if self
            .actor_database
            .as_ref()
            .is_some_and(|actor_db| actor_db.shares_authority_with(backend_db))
        {
            Ok(())
        } else {
            Err(NativeCloseApplicationError::SourceRefused)
        }
    }

    /// Invoked only by the SOURCE actor's dedicated acknowledged request branch.
    /// No actor RPC, target lock acquisition or cleanup is permitted here. Exclusive
    /// actor ownership spans live source matching, receipt lookup, actual DB commit
    /// and durable-result classification. Source/target DB authority is rechecked
    /// transactionally, not inferred from the queued request or watch state.
    #[cfg(test)]
    pub(super) async fn execute_native_close_initial(
        &mut self,
        backend: &crate::runtime::RuntimeManager,
        source: SourceToolCall,
        target: ProductConversationId,
        admitted: crate::runtime::AdmittedOperation,
    ) -> Result<NativeCloseInitialResult, NativeCloseApplicationError> {
        self.check_native_close_backend(backend.db())?;
        if !self.context.is_coordinator
            || self.context.is_sub_agent
            || self
                .fatal_external_effect_cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(NativeCloseApplicationError::SourceRefused);
        }
        // This is supporting correlation matching, not a separately reusable permit.
        let command = initial_command_from_executing_source(
            &self.state,
            &self.context.conversation_id,
            &source,
            target,
        )
        .map_err(|_| NativeCloseApplicationError::SourceRefused)?;
        // Production already installs the same database used by native admission.
        // An absent test/dormant dependency fails closed; no synthetic manager.
        let db = self
            .actor_database
            .as_ref()
            .ok_or(NativeCloseApplicationError::Unavailable)?;
        // Once this boundary starts, losing its future before a typed result must
        // close authority before releasing the admitted owner (REQ-BED-033).
        let mut authority = RuntimeSettlementAuthority {
            owner: Some(admitted),
            abandoned_reason: "native Close initial admission owner disappeared",
        };
        match db.apply_native_close_initial(&command).await {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                // Both committed success and established rollback are classified.
                authority.owner = None;
                result.map_err(|_| NativeCloseApplicationError::Storage)
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                // Keep the guard armed: Drop closes the fence before owner release.
                Err(NativeCloseApplicationError::FatalAuthority)
            }
        }
    }
}

#[cfg(test)]
mod terminal_compatibility_tests {
    use super::terminal_run_outcome_is_compatible;
    use phoenix_core::domain::close::{CloseCompletionOutcome, CloseRunStatus};

    #[test]
    fn terminal_run_status_and_outcome_matrix_is_explicit() {
        let outcomes = [
            CloseCompletionOutcome::Archived,
            CloseCompletionOutcome::Cancelled,
            CloseCompletionOutcome::ArchivedCleanupAttention,
            CloseCompletionOutcome::CloseIncomplete,
        ];
        let cases = [
            (CloseRunStatus::Running, [false, false, false, false]),
            (CloseRunStatus::Stopped, [true, true, true, true]),
            (CloseRunStatus::Completed, [true, true, false, false]),
        ];
        for (status, expected) in cases {
            for (outcome, expected) in outcomes.into_iter().zip(expected) {
                assert_eq!(
                    terminal_run_outcome_is_compatible(status, outcome),
                    expected,
                    "{status:?} with {outcome:?}",
                );
            }
        }
    }
}
