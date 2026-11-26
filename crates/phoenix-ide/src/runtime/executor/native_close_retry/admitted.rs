//! Joined, actor-serviced execution of the finite admitted successor plan.

use super::super::native_close::source_entry::{
    ActorEntryResult, SourceEntryController, SourceEntryWorker, WorkerEntryResult,
};
use super::super::RuntimeSettlementAuthority;
use super::NativeRetryExecutionError;
use crate::runtime::close_retirement::NativeRetryNoLiveResourcesError;
use crate::runtime::native_close_application::NativeCloseApplicationError;
use crate::runtime::{AdmittedOperation, RuntimeManager};
use crate::state_machine::ConvState;
use phoenix_core::domain::close::{ClosePhase, CloseRunRef, CloseRunStatus, RetiredResourceKind};
use phoenix_db::native_close::{
    NativeCloseCommand, NativeCloseReceipt, NativeCloseRetryExecutionContext,
    NativeCloseRetryResult,
};
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::CloseSafeRetryEffect;
use std::sync::Arc;
use tokio::sync::{oneshot, OwnedMutexGuard};

#[cfg(unix)]
use crate::runtime::close_retirement::native_initial::worktree::{
    prepare_original_worktree_git_until, NativeWorktreeGit, OriginalWorktreeEffectContext,
    WorktreePhysicalObservation, WorktreePhysicalOutcome,
};

enum Stage {
    Context,
    Select,
    ScopeRunning,
    ScopeEvidence,
    TmuxRunning,
    TmuxEvidence,
    WorktreeDispatch,
    WorktreeDraining,
    WorktreeRecordDispatch,
    WorktreeRecordDispatchRunning,
    WorktreePreparing,
    WorktreePhysical,
    WorktreeEvidence,
    Failure(String),
    Complete,
    ObserveTerminal,
    Fatal,
}

pub(in crate::runtime::executor) struct AdmittedRetryCursor {
    command: NativeCloseCommand,
    receipt: NativeCloseReceipt,
    backend: Arc<RuntimeManager>,
    context: Option<NativeCloseRetryExecutionContext>,
    order: Vec<usize>,
    index: usize,
    stage: Stage,
    deadline: std::time::Instant,
    next_service: tokio::time::Instant,
    source_entry: SourceEntryWorker,
    scope_worker: Option<tokio::task::JoinHandle<Result<(), NativeRetryExecutionError>>>,
    tmux_worker: Option<tokio::task::JoinHandle<Result<(), NativeRetryExecutionError>>>,
    drain_worker: Option<tokio::task::JoinHandle<Result<(), NativeRetryNoLiveResourcesError>>>,
    dispatch_worker: Option<tokio::task::JoinHandle<Result<(), NativeRetryExecutionError>>>,
    #[cfg(unix)]
    preparation: Option<tokio::task::JoinHandle<Result<Option<NativeWorktreeGit>, String>>>,
    #[cfg(unix)]
    physical: Option<WorktreePhysicalObservation>,
    #[cfg(unix)]
    stopped_physical_outcome: Option<WorktreePhysicalOutcome>,
    response: Option<oneshot::Sender<Result<NativeCloseRetryResult, NativeCloseApplicationError>>>,
    _target_gate: OwnedMutexGuard<()>,
    _attempt_gate: OwnedMutexGuard<()>,
    authority: RuntimeSettlementAuthority,
}

impl AdmittedRetryCursor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        command: NativeCloseCommand,
        receipt: NativeCloseReceipt,
        backend: Arc<RuntimeManager>,
        admitted: AdmittedOperation,
        target_gate: OwnedMutexGuard<()>,
        attempt_gate: OwnedMutexGuard<()>,
        response: oneshot::Sender<Result<NativeCloseRetryResult, NativeCloseApplicationError>>,
        source_entry: SourceEntryWorker,
        deadline: std::time::Instant,
    ) -> Self {
        Self {
            command,
            receipt,
            backend,
            context: None,
            order: Vec::new(),
            index: 0,
            stage: Stage::Context,
            deadline,
            next_service: tokio::time::Instant::now(),
            source_entry,
            scope_worker: None,
            tmux_worker: None,
            drain_worker: None,
            dispatch_worker: None,
            #[cfg(unix)]
            preparation: None,
            #[cfg(unix)]
            physical: None,
            #[cfg(unix)]
            stopped_physical_outcome: None,
            response: Some(response),
            _target_gate: target_gate,
            _attempt_gate: attempt_gate,
            authority: RuntimeSettlementAuthority {
                owner: Some(admitted),
                abandoned_reason: "admitted native retry cursor lost settlement custody",
            },
        }
    }

    pub(in crate::runtime::executor) fn next_service_at(&self) -> tokio::time::Instant {
        self.next_service
    }

    fn effect(&self) -> &CloseSafeRetryEffect {
        &self
            .context
            .as_ref()
            .expect("retained retry context")
            .progress
            .pending[self.order[self.index]]
    }

    pub(in crate::runtime::executor) async fn join_before_exit(mut self) {
        let fatal_exit = matches!(self.stage, Stage::Fatal)
            || self
                .backend
                .fatal_local_authority_fence
                .fatal_authority_loss_is_latched();
        let retain_custody = fatal_exit || self.join_workers().await;
        // An exit is not terminal evidence. Dropping armed authority fences it.
        if let Some(response) = self.response.take() {
            let _ = response.send(Err(NativeCloseApplicationError::FatalAuthority));
        }
        if retain_custody {
            // Keep target/attempt locks and the physical observer alive while an
            // unknown or deadline-expired worker still owns the native effect.
            self.backend
                .fatal_local_authority_fence
                .close("native_close_retry_exit_retained_custody");
            std::mem::forget(self);
        }
    }

    async fn join_workers(&mut self) -> bool {
        let deadline = tokio::time::Instant::from_std(self.deadline);
        // Every worker shares the admitted absolute budget. A timeout must not
        // take/drop a handle whose mutation or writer custody remains unknown.
        macro_rules! join_owned_worker {
            ($field:ident) => {
                if let Some(worker) = self.$field.as_mut() {
                    match tokio::time::timeout_at(deadline, &mut *worker).await {
                        Ok(Ok(result)) => {
                            // Even a completed worker error can mean that its durable
                            // authority is unknown. Keep custody rather than releasing
                            // the gates on actor exit.
                            let unclassified = result.is_err();
                            self.$field.take();
                            if unclassified {
                                return true;
                            }
                        }
                        Ok(Err(_)) | Err(_) => return true,
                    }
                }
            };
        }
        join_owned_worker!(drain_worker);
        join_owned_worker!(scope_worker);
        join_owned_worker!(tmux_worker);
        join_owned_worker!(dispatch_worker);
        #[cfg(unix)]
        {
            // spawn_blocking cannot be aborted after entry. Join only within
            // the admitted attempt's original deadline, retaining the cursor if
            // the worker still owns preparation when that budget expires.
            if let Some(preparation) = self.preparation.as_mut() {
                let deadline = tokio::time::Instant::from_std(self.deadline);
                match tokio::time::timeout_at(deadline, &mut *preparation).await {
                    Ok(Ok(_result)) => {
                        // timeout_at already consumed the completed JoinHandle result.
                        self.preparation.take();
                    }
                    Ok(Err(_)) => {
                        self.preparation.take();
                        return true;
                    }
                    Err(_) => return true,
                }
            }
            if let Some(physical) = self.physical.as_ref() {
                if physical.has_fatal_custody() {
                    return true;
                }
                if !physical.is_ready() {
                    let deadline = tokio::time::Instant::from_std(self.deadline);
                    if !matches!(
                        tokio::time::timeout_at(deadline, physical.observe()).await,
                        Ok(Ok(_))
                    ) {
                        return true;
                    }
                } else if physical.observe().await.is_err() {
                    return true;
                }
            }
        }
        false
    }

    fn finish(&mut self, result: Result<NativeCloseRetryResult, NativeCloseApplicationError>) {
        self.authority.owner = None;
        if let Some(response) = self.response.take() {
            let _ = response.send(result);
        }
    }

    fn accept(&mut self, result: Result<(), NativeRetryExecutionError>) -> bool {
        match result {
            Ok(()) => {
                self.index += 1;
                self.stage = Stage::Select;
                false
            }
            Err(NativeRetryExecutionError::ResourceFailure(detail)) => {
                self.stage = Stage::Failure(detail);
                false
            }
            Err(NativeRetryExecutionError::TerminalizedFailure(_)) => {
                self.finish(Err(NativeCloseApplicationError::Storage));
                true
            }
            Err(NativeRetryExecutionError::Unclassified) => {
                self.stage = Stage::Fatal;
                false
            }
        }
    }

    // Returns true only when terminal custody has been released. Fatal preserves it.
    pub(in crate::runtime::executor) async fn step(
        &mut self,
        live: &ConvState,
        controller: &mut Option<SourceEntryController>,
    ) -> Result<bool, NativeCloseApplicationError> {
        let result = self.step_inner(live, controller).await;
        self.next_service = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
        result
    }

    #[allow(clippy::too_many_lines)] // One actor-owned stage machine preserves source checks between every effect.
    async fn step_inner(
        &mut self,
        live: &ConvState,
        controller: &mut Option<SourceEntryController>,
    ) -> Result<bool, NativeCloseApplicationError> {
        if matches!(self.stage, Stage::Fatal) {
            return Err(NativeCloseApplicationError::FatalAuthority);
        }
        #[cfg(unix)]
        // An unfinished physical worker owns its original source/deadline and checks
        // them at every leaf. Idle polling must not contend with those leaves for
        // eligibility. Revalidate again once readiness proves custody was released.
        if matches!(self.stage, Stage::WorktreePhysical)
            && self
                .physical
                .as_ref()
                .is_some_and(|physical| !physical.is_ready())
        {
            #[cfg(test)]
            if let Some(hooks) = super::test_hooks::Hooks::current() {
                hooks.record_pending_tick();
            }
            return Ok(false);
        }
        let current = if let Some(controller) = controller.as_mut() {
            match controller.revalidate(live, &self.command) {
                ActorEntryResult::Updated => true,
                ActorEntryResult::Busy
                    if self
                        .scope_worker
                        .as_ref()
                        .is_some_and(tokio::task::JoinHandle::is_finished) =>
                {
                    false
                }
                ActorEntryResult::Busy
                    if self
                        .tmux_worker
                        .as_ref()
                        .is_some_and(tokio::task::JoinHandle::is_finished) =>
                {
                    false
                }
                ActorEntryResult::Busy
                    if self
                        .dispatch_worker
                        .as_ref()
                        .is_some_and(tokio::task::JoinHandle::is_finished) =>
                {
                    false
                }
                ActorEntryResult::Busy => return Ok(false),
                ActorEntryResult::Refused => false,
            }
        } else {
            false
        };
        if !current
            && !matches!(
                self.stage,
                Stage::ScopeRunning
                    | Stage::TmuxRunning
                    | Stage::WorktreeDraining
                    | Stage::WorktreeRecordDispatchRunning
                    | Stage::WorktreePreparing
                    | Stage::WorktreePhysical
                    | Stage::Failure(_)
            )
        {
            self.stage = Stage::ObserveTerminal;
        }
        let stage = std::mem::replace(&mut self.stage, Stage::Fatal);
        match stage {
            Stage::Context => {
                let context = self
                    .backend
                    .db()
                    .read_native_close_retry_execution_context(
                        &self.command,
                        &self.receipt.admitted,
                    )
                    .await;
                let Ok(context) = context else {
                    self.stage = Stage::ObserveTerminal;
                    return Ok(false);
                };
                self.order = crate::runtime::close_retirement::close_safe_retry_execution_order(
                    &context.progress.pending,
                );
                self.context = Some(context);
                self.stage = Stage::Select;
            }
            Stage::Select => {
                if self.index == self.order.len() {
                    self.stage = Stage::Complete;
                } else {
                    let effect = self.effect().clone();
                    match effect.resource.kind() {
                        RetiredResourceKind::Worktree => self.stage = Stage::WorktreeDispatch,
                        RetiredResourceKind::TmuxServer => {
                            self.tmux_worker =
                                Some(self.backend.spawn_native_retry_tmux_with_source_entry(
                                    self.command.clone(),
                                    self.receipt.admitted.clone(),
                                    self.context.as_ref().expect("context").clone(),
                                    effect,
                                    live.clone(),
                                    self.source_entry.clone(),
                                    self.deadline,
                                ));
                            self.stage = Stage::TmuxRunning;
                        }
                        RetiredResourceKind::WorkScope => {
                            self.scope_worker =
                                Some(self.backend.spawn_native_retry_scope_with_source_entry(
                                    self.command.clone(),
                                    self.receipt.admitted.clone(),
                                    effect,
                                    self.source_entry.clone(),
                                    self.deadline,
                                ));
                            self.stage = Stage::ScopeRunning;
                        }
                        _ => {
                            self.stage =
                                Stage::Failure("unsupported native retry resource kind".into());
                        }
                    }
                }
            }
            Stage::TmuxRunning => {
                let worker = self.tmux_worker.as_ref().expect("retained tmux worker");
                if !worker.is_finished() {
                    self.stage = Stage::TmuxRunning;
                    return Ok(false);
                }
                let result = self.tmux_worker.take().expect("retained tmux worker").await;
                if matches!(
                    &result,
                    Err(_) | Ok(Err(NativeRetryExecutionError::Unclassified))
                ) {
                    self.stage = Stage::Fatal;
                    return Err(NativeCloseApplicationError::FatalAuthority);
                }
                if current {
                    match result {
                        Ok(Ok(())) => self.stage = Stage::TmuxEvidence,
                        Ok(Err(error)) => {
                            if self.accept(Err(error)) {
                                return Ok(true);
                            }
                        }
                        Err(_) => self.stage = Stage::Fatal,
                    }
                } else {
                    tracing::debug!(
                        ?result,
                        "joined tmux retry after source loss; no current-source success recorded"
                    );
                    self.stage = Stage::ObserveTerminal;
                }
            }
            Stage::TmuxEvidence => {
                let result = self
                    .backend
                    .native_retry_record_success(
                        &self.command,
                        &self.receipt.admitted,
                        self.effect(),
                        "joined exact native retry tmux retirement",
                    )
                    .await;
                if self.accept(result) {
                    return Ok(true);
                }
            }
            Stage::ScopeRunning => {
                let worker = self
                    .scope_worker
                    .as_ref()
                    .expect("retained WorkScope worker");
                if !worker.is_finished() {
                    self.stage = Stage::ScopeRunning;
                    return Ok(false);
                }
                let result = self
                    .scope_worker
                    .take()
                    .expect("retained WorkScope worker")
                    .await;
                if matches!(
                    &result,
                    Err(_) | Ok(Err(NativeRetryExecutionError::Unclassified))
                ) {
                    self.stage = Stage::Fatal;
                    return Err(NativeCloseApplicationError::FatalAuthority);
                }
                if current {
                    match result {
                        Ok(Ok(())) => self.stage = Stage::ScopeEvidence,
                        Ok(Err(error)) => {
                            if self.accept(Err(error)) {
                                return Ok(true);
                            }
                        }
                        Err(_) => self.stage = Stage::Fatal,
                    }
                } else {
                    self.stage = Stage::ObserveTerminal;
                }
            }
            Stage::ScopeEvidence => {
                let result = self
                    .backend
                    .native_retry_record_success(
                        &self.command,
                        &self.receipt.admitted,
                        self.effect(),
                        "independent native retry WorkScope retirement",
                    )
                    .await;
                if self.accept(result) {
                    return Ok(true);
                }
            }
            Stage::WorktreeDispatch => {
                let backend = Arc::clone(&self.backend);
                let scope = self.effect().scope.clone();
                let deadline = tokio::time::Instant::from_std(self.deadline);
                // This worker owns only read-only inventory: timeout drops no DB
                // transaction or source lease. The actor remains selectable for Stop.
                self.drain_worker = Some(tokio::spawn(async move {
                    tokio::time::timeout_at(
                        deadline,
                        backend.native_retry_no_live_resources(&scope),
                    )
                    .await
                    .unwrap_or(Err(NativeRetryNoLiveResourcesError::Unclassified))
                }));
                self.stage = Stage::WorktreeDraining;
            }
            Stage::WorktreeDraining => {
                let worker = self.drain_worker.as_ref().expect("retained runtime drain");
                if !worker.is_finished() {
                    self.stage = Stage::WorktreeDraining;
                    return Ok(false);
                }
                let result = self
                    .drain_worker
                    .take()
                    .expect("retained runtime drain")
                    .await;
                self.stage = if current {
                    match result {
                        Ok(Ok(())) => Stage::WorktreeRecordDispatch,
                        Ok(Err(NativeRetryNoLiveResourcesError::Blocked)) => Stage::Failure(
                            "native retry scope runtime drain not established".into(),
                        ),
                        _ => Stage::Fatal,
                    }
                } else {
                    Stage::ObserveTerminal
                };
            }
            Stage::WorktreeRecordDispatch => {
                // A separate actor pass revalidates the original source after drain
                // and before dispatch metadata or any physical worker is entered.
                let effect = self.effect().clone();
                let backend = Arc::clone(&self.backend);
                let command = self.command.clone();
                let admitted = self.receipt.admitted.clone();
                let source_entry = self.source_entry.clone();
                let deadline = self.deadline;
                let runtime = tokio::runtime::Handle::current();
                #[cfg(test)]
                let hooks = super::test_hooks::Hooks::current();
                self.dispatch_worker = Some(tokio::task::spawn_blocking(move || {
                    runtime.block_on(async move {
                        #[cfg(test)]
                        super::test_hooks::checkpoint(
                            hooks.as_ref(),
                            super::test_hooks::Point::WorktreeDispatchBeforeWriter,
                        )
                        .await;
                        let (dispatch, lease) = backend.db()
                            .record_native_close_retry_retirement_dispatch_with_entry(
                                &command, &admitted, &effect,
                                || match source_entry.try_acquire_first_effect(&command) {
                                    WorkerEntryResult::Ran(lease) if std::time::Instant::now() < deadline => Ok(lease),
                                    _ => Err(crate::db::DbError::CloseFoundationPrecondition(
                                        "original native retry source unavailable at Worktree dispatch".into(),
                                    )),
                                },
                            ).await;
                        if matches!(dispatch, LocalAuthorityResult::DurableFactUnclassified) {
                            backend.fatal_local_authority_fence.close(
                                "native_retry_worktree_dispatch_release_unclassified",
                            );
                            // Consume the entire option so its destructor cannot retain
                            // a borrow while custody of the source owner is forgotten.
                            std::mem::forget(lease);
                            std::mem::forget(source_entry);
                            return Err(NativeRetryExecutionError::Unclassified);
                        }
                        drop(lease);
                        match dispatch {
                            LocalAuthorityResult::DurableFactEstablished(Ok(())) => Ok(()),
                            LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                                Err(NativeRetryExecutionError::ResourceFailure(error.to_string()))
                            }
                            LocalAuthorityResult::DurableFactUnclassified => unreachable!(),
                        }
                    })
                }));
                self.stage = Stage::WorktreeRecordDispatchRunning;
            }
            Stage::WorktreeRecordDispatchRunning => {
                let worker = self
                    .dispatch_worker
                    .as_ref()
                    .expect("retained Worktree dispatch worker");
                if !worker.is_finished() {
                    self.stage = Stage::WorktreeRecordDispatchRunning;
                    return Ok(false);
                }
                let result = self
                    .dispatch_worker
                    .take()
                    .expect("retained Worktree dispatch worker")
                    .await;
                if matches!(
                    &result,
                    Err(_) | Ok(Err(NativeRetryExecutionError::Unclassified))
                ) {
                    self.stage = Stage::Fatal;
                    return Err(NativeCloseApplicationError::FatalAuthority);
                }
                if current {
                    match result {
                        Ok(Ok(())) => self.stage = Stage::WorktreePreparing,
                        Ok(Err(NativeRetryExecutionError::ResourceFailure(detail))) => {
                            self.stage = Stage::Failure(detail);
                        }
                        Ok(Err(NativeRetryExecutionError::TerminalizedFailure(detail))) => {
                            if self
                                .accept(Err(NativeRetryExecutionError::TerminalizedFailure(detail)))
                            {
                                return Ok(true);
                            }
                        }
                        Ok(Err(NativeRetryExecutionError::Unclassified)) | Err(_) => {
                            unreachable!("fatal dispatch result handled above")
                        }
                    }
                } else {
                    tracing::debug!(?result, "joined Worktree dispatch after source loss; no physical preparation entered");
                    self.stage = Stage::ObserveTerminal;
                }
            }
            Stage::WorktreePreparing => {
                #[cfg(unix)]
                {
                    if let Some(worker) = self.preparation.as_mut() {
                        if !worker.is_finished() {
                            self.stage = Stage::WorktreePreparing;
                            return Ok(false);
                        }
                        let prepared = worker.await;
                        self.preparation = None;
                        match prepared {
                            Ok(Ok(Some(prepared))) if current => {
                                let effect = self.effect().clone();
                                let original = OriginalWorktreeEffectContext::new(
                                    self.backend.db().clone(),
                                    self.command.clone(),
                                    self.receipt.clone(),
                                    live.clone(),
                                    effect.scope.clone(),
                                    self.context.as_ref().expect("context").snapshot.clone(),
                                    effect.resource.clone(),
                                    self.source_entry.clone(),
                                    self.deadline,
                                    Arc::clone(&self.backend.fatal_local_authority_fence),
                                );
                                match prepared.into_continuation(original) {
                                    Ok(continuation) => {
                                        let probe = self.backend.native_retry_close_writer_probe(
                                            &self.receipt,
                                            &effect,
                                            &self.context.as_ref().expect("context").snapshot,
                                        );
                                        self.physical = Some(continuation.spawn_physical(
                                            tokio::runtime::Handle::current(),
                                            probe,
                                        ));
                                        self.stage = Stage::WorktreePhysical;
                                    }
                                    Err(detail) => self.stage = Stage::Failure(detail),
                                }
                            }
                            Ok(Ok(_)) if !current => self.stage = Stage::ObserveTerminal,
                            Ok(Ok(None)) => {
                                self.stage = Stage::Failure(
                                    "captured Worktree absent without a physical retirement proof"
                                        .into(),
                                );
                            }
                            Ok(Err(detail)) => self.stage = Stage::Failure(detail),
                            Err(_) => {}
                            Ok(Ok(Some(_))) => unreachable!("current source handled above"),
                        }
                    } else if current {
                        let context = self.context.as_ref().expect("context");
                        let effect = self.effect();
                        let identity = context
                            .scopes
                            .iter()
                            .find(|scope| scope.scope == effect.scope)
                            .and_then(|scope| scope.captured_worktree.as_ref());
                        if let Some(
                            phoenix_core::domain::close::CapturedWorktreeIdentity::Resolved(
                                identity,
                            ),
                        ) = identity
                        {
                            let identity = identity.clone();
                            let deadline = self.deadline;
                            #[cfg(test)]
                            let hooks = super::test_hooks::Hooks::current();
                            #[cfg(test)]
                            let runtime = tokio::runtime::Handle::current();
                            self.preparation = Some(tokio::task::spawn_blocking(move || {
                                let prepared =
                                    prepare_original_worktree_git_until(&identity, deadline);
                                #[cfg(test)]
                                if matches!(&prepared, Ok(Some(_))) {
                                    runtime.block_on(super::test_hooks::checkpoint(
                                        hooks.as_ref(),
                                        super::test_hooks::Point::WorktreePreparationReady,
                                    ));
                                }
                                prepared
                            }));
                            self.stage = Stage::WorktreePreparing;
                        } else {
                            self.stage =
                                Stage::Failure("captured Worktree identity unresolved".into());
                        }
                    } else {
                        self.stage = Stage::ObserveTerminal;
                    }
                }
                #[cfg(not(unix))]
                {
                    self.stage = Stage::Failure(
                        "native physical Worktree retry unavailable on this platform".into(),
                    );
                }
            }
            Stage::WorktreePhysical => {
                #[cfg(unix)]
                {
                    let physical = self
                        .physical
                        .as_ref()
                        .expect("retained physical observation");
                    if !physical.is_ready() {
                        self.stage = Stage::WorktreePhysical;
                        return Ok(false);
                    }
                    match physical.observe().await {
                        Ok(outcome) if !current => {
                            tracing::debug!(?outcome, "joined native retry physical outcome after source loss; preserving stopped failure, not recording current-source success");
                            self.stopped_physical_outcome = Some(outcome);
                            self.stage = Stage::ObserveTerminal;
                        }
                        Ok(WorktreePhysicalOutcome::Retired) => {
                            self.stage = Stage::WorktreeEvidence;
                        }
                        Ok(
                            WorktreePhysicalOutcome::StopFailed { detail }
                            | WorktreePhysicalOutcome::ReinspectionRequired { detail }
                            | WorktreePhysicalOutcome::Residual { detail },
                        ) => self.stage = Stage::Failure(detail),
                        Err(_) => {}
                    }
                    self.physical = None;
                }
            }
            Stage::WorktreeEvidence => {
                let result = self
                    .backend
                    .native_retry_record_success(
                        &self.command,
                        &self.receipt.admitted,
                        self.effect(),
                        "joined original-context native Worktree retirement",
                    )
                    .await;
                if self.accept(result) {
                    return Ok(true);
                }
            }
            Stage::Failure(detail) => {
                if !current {
                    let run = CloseRunRef {
                        attempt_id: self.receipt.admitted.attempt_id.clone(),
                        ordinal: self.receipt.admitted.run_ordinal,
                    };
                    match self.backend.db().get_close_run(&run).await {
                        Ok(observed)
                            if observed.run == run
                                && observed.status == CloseRunStatus::Stopped =>
                        {
                            self.stage = Stage::ObserveTerminal;
                            return Ok(false);
                        }
                        Ok(observed)
                            if observed.run == run
                                && observed.status == CloseRunStatus::Running => {}
                        _ => return Err(NativeCloseApplicationError::FatalAuthority),
                    }
                }
                let result = self
                    .backend
                    .native_retry_failure::<()>(
                        &self.command,
                        &self.receipt.admitted,
                        self.context.as_ref().expect("context"),
                        self.effect(),
                        &detail,
                    )
                    .await;
                if self.accept(result) {
                    return Ok(true);
                }
            }
            Stage::Complete => {
                match self
                    .backend
                    .db()
                    .complete_native_close_retry(&self.command, &self.receipt.admitted)
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(_)) => {
                        self.finish(Ok(NativeCloseRetryResult::NewlyAdmitted(
                            self.receipt.clone(),
                        )));
                        return Ok(true);
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(_)) => {
                        self.stage = Stage::ObserveTerminal;
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {}
                }
            }
            Stage::ObserveTerminal => {
                let run = CloseRunRef {
                    attempt_id: self.receipt.admitted.attempt_id.clone(),
                    ordinal: self.receipt.admitted.run_ordinal,
                };
                if let (Ok(observed), Ok(obligation)) = (
                    self.backend.db().get_close_run(&run).await,
                    self.backend
                        .db()
                        .get_close_obligation(run.attempt_id.as_str())
                        .await,
                ) {
                    if observed.run == run
                        && observed.status == CloseRunStatus::Running
                        && std::time::Instant::now() >= self.deadline
                    {
                        if self.index < self.order.len() {
                            self.stage = Stage::Failure(
                                "original native retry source deadline expired".into(),
                            );
                            return Ok(false);
                        }
                        // No pending effect remains for the resource-failure API. Do not
                        // replay completion under a lost source or fabricate a failed effect.
                        tracing::error!("native retry source deadline expired without a pending effect; fail-stopping with custody armed");
                        return Err(NativeCloseApplicationError::FatalAuthority);
                    }
                    if observed.run == run
                        && observed.status == CloseRunStatus::Stopped
                        && obligation.phase() == ClosePhase::Completed
                    {
                        if let Ok(failures) = self
                            .backend
                            .db()
                            .list_close_cleanup_failures(run.attempt_id.as_str())
                            .await
                        {
                            if failures.iter().any(|failure| {
                                failure.run_ordinal == run.ordinal
                                    && failure.occurrence.failure_occurrence_id
                                        == run.failure_occurrence_id()
                            }) {
                                #[cfg(unix)]
                                tracing::debug!(outcome = ?self.stopped_physical_outcome, "observed exact stopped native retry failure; joined outcome retained locally, no post-Stop success synthesized");
                                self.finish(Err(NativeCloseApplicationError::Storage));
                                return Ok(true);
                            }
                        }
                    }
                }
                if std::time::Instant::now() >= self.deadline {
                    tracing::error!("native retry terminal evidence unavailable at original deadline; fail-stopping with custody armed");
                    return Err(NativeCloseApplicationError::FatalAuthority);
                }
                self.stage = Stage::ObserveTerminal;
            }
            Stage::Fatal => return Err(NativeCloseApplicationError::FatalAuthority),
        }
        if matches!(self.stage, Stage::Fatal) {
            Err(NativeCloseApplicationError::FatalAuthority)
        } else {
            Ok(false)
        }
    }
}
