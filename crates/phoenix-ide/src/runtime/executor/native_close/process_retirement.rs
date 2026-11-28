//! Actor-owned runtime-process cursor. No worktree or scope completion authority.

use super::{
    NativeCloseApplicationError, NativeCloseInitialContinuation,
    NativeCloseInitialContinuationPhase as Phase,
};
use crate::runtime::close_retirement::native_initial::resource::{
    NativeInitialPreparedEntry, ResourceObserver,
};
use crate::runtime::close_retirement::native_initial::{
    NativeInitialObservation, NativeInitialProcessStep, NativeInitialStepError,
};
use futures::FutureExt;
use phoenix_core::domain::close::{
    AbsenceBasis, CloseExpectedRetirementResource, CloseRunRef, LossItemIdentity,
    RetiredResourceKind, RetirementOutcome,
};
use phoenix_core::work_scope::WorkScopeId;
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_db::{
    CloseProcessStepOutcome, CloseProcessStepSuccess, CloseRetirementBatchDispatch,
    FirstCloseRetirementDispatch, RecordCloseRetirementDispatchRequest,
    RecordCloseRetirementEvidenceRequest,
};
use std::sync::Arc;

/// The retained task owns release classification even after its observer goes
/// away. Only preparation tasks are abortable; entry replies are observations.
fn spawn_retained_entry(
    backend: Arc<crate::runtime::RuntimeManager>,
    entry: impl std::future::Future<Output = Result<ResourceObserver, NativeInitialStepError>>
        + Send
        + 'static,
) -> tokio::task::JoinHandle<Result<ResourceObserver, NativeInitialStepError>> {
    tokio::spawn(async move {
        let result = match std::panic::AssertUnwindSafe(entry).catch_unwind().await {
            Ok(result) => result,
            Err(_) => Err(NativeInitialStepError::FatalAuthority),
        };
        if matches!(&result, Err(NativeInitialStepError::FatalAuthority)) {
            backend.signal_fatal_local_authority("native_close_initial_retained_entry");
        }
        result
    })
}

#[derive(Debug)]
enum BlockedProcess {
    Worker { _error: NativeInitialStepError },
    Join { _error: String },
}

pub(super) struct ProcessRetirement {
    inventory: Vec<CloseExpectedRetirementResource>,
    snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
    scopes: Vec<WorkScopeId>,
    scope_index: usize,
    kind_index: usize,
    pending_dispatches: std::collections::VecDeque<FirstCloseRetirementDispatch>,
    // Only inert preparation belongs to the actor and may be aborted.
    worker:
        Option<tokio::task::JoinHandle<Result<NativeInitialPreparedEntry, NativeInitialStepError>>>,
    // Entry may already have crossed the first-effect boundary. Never abort it
    // when Stop, source loss, or a response cancellation interrupts this cursor.
    entry: Option<tokio::task::JoinHandle<Result<ResourceObserver, NativeInitialStepError>>>,
    resource_index: usize,
    observer: Option<ResourceObserver>,
    pending: Option<NativeInitialObservation>,
    evidence_index: usize,
    process_receipt_recorded: bool,
    /// Physical uncertainty is retained without manufacturing a failed resource.
    blocked: Option<BlockedProcess>,
    retry_backoff: bool,
}

impl ProcessRetirement {
    pub(super) fn has_resources(&self) -> bool {
        self.inventory.iter().any(|item| {
            NativeInitialProcessStep::ORDERED
                .iter()
                .any(|step| item.resource.kind() == step.retired_kind())
        })
    }

    /// The actor opens one original live-source cell for every physical stage,
    /// including a captured Worktree when no process resources were present.
    pub(super) fn has_original_effects(&self) -> bool {
        self.has_resources()
            || self
                .inventory
                .iter()
                .any(|item| item.resource.kind() == RetiredResourceKind::Worktree)
    }

    pub(super) fn captured_binding(
        &self,
    ) -> (
        &[WorkScopeId],
        &phoenix_core::domain::close::CloseRetirementSnapshot,
    ) {
        (&self.scopes, &self.snapshot)
    }
    pub(super) fn captured_inventory(&self) -> &[CloseExpectedRetirementResource] {
        &self.inventory
    }

    pub(super) fn new(
        inventory: Vec<CloseExpectedRetirementResource>,
        scopes: Vec<WorkScopeId>,
        snapshot: phoenix_core::domain::close::CloseRetirementSnapshot,
    ) -> Self {
        Self {
            inventory,
            snapshot,
            scopes,
            scope_index: 0,
            kind_index: 0,
            pending_dispatches: std::collections::VecDeque::new(),
            worker: None,
            entry: None,
            resource_index: 0,
            observer: None,
            pending: None,
            evidence_index: 0,
            process_receipt_recorded: false,
            blocked: None,
            retry_backoff: false,
        }
    }

    pub(super) fn service_delay(&self) -> std::time::Duration {
        // Give ordinary events a service opportunity after each bounded cursor
        // mutation, without spending the original two-second tmux permit budget
        // on a quarter-second sleep for every dispatch/evidence row.
        if self.retry_backoff {
            std::time::Duration::from_millis(250)
        } else if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
            || self
                .entry
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
        {
            std::time::Duration::from_millis(10)
        } else {
            std::time::Duration::from_millis(1)
        }
    }

    pub(super) fn abort(&self) {
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
    #[cfg(test)]
    pub(super) fn has_unresolved_exit_custody(&self) -> bool {
        self.worker.is_some() || self.entry.is_some()
    }

    pub(super) async fn poll_exit_custody(
        &mut self,
        deadline: Option<std::time::Instant>,
    ) -> super::post_retirement::ExitCustodyPoll {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
            || self
                .entry
                .as_ref()
                .is_some_and(|entry| !entry.is_finished())
        {
            return if deadline.is_none_or(|deadline| std::time::Instant::now() >= deadline) {
                super::post_retirement::ExitCustodyPoll::Fatal
            } else {
                super::post_retirement::ExitCustodyPoll::Pending
            };
        }

        if let Some(worker) = self.worker.take() {
            // This is inert preparation; cancellation is a classified outcome.
            if let Err(error) = worker.await {
                if !error.is_cancelled() {
                    tracing::error!(%error, "native original process preparation exit observation lost");
                    return super::post_retirement::ExitCustodyPoll::Fatal;
                }
            }
        }
        if let Some(entry) = self.entry.take() {
            match entry.await {
                Ok(Ok(observer)) => self.observer = Some(observer),
                Ok(Err(NativeInitialStepError::FatalAuthority)) => {
                    return super::post_retirement::ExitCustodyPoll::Fatal;
                }
                Ok(Err(error)) => self.blocked = Some(BlockedProcess::Worker { _error: error }),
                Err(error) => {
                    self.blocked = Some(BlockedProcess::Join {
                        _error: error.to_string(),
                    });
                }
            }
        }
        super::post_retirement::ExitCustodyPoll::Classified
    }

    fn current(&self) -> Option<(WorkScopeId, NativeInitialProcessStep)> {
        self.scopes.get(self.scope_index).map(|scope| {
            (
                scope.clone(),
                NativeInitialProcessStep::ORDERED[self.kind_index],
            )
        })
    }

    fn current_resources(
        &self,
        scope: &WorkScopeId,
        step: NativeInitialProcessStep,
    ) -> Vec<CloseExpectedRetirementResource> {
        self.inventory
            .iter()
            .filter(|item| &item.scope == scope && item.resource.kind() == step.retired_kind())
            .cloned()
            .collect()
    }

    fn advance(&mut self) {
        self.pending = None;
        self.evidence_index = 0;
        self.process_receipt_recorded = false;
        debug_assert!(self.pending_dispatches.is_empty());
        self.resource_index = 0;
        self.observer = None;
        self.kind_index += 1;
        if self.kind_index == NativeInitialProcessStep::ORDERED.len() {
            self.kind_index = 0;
            self.scope_index += 1;
        }
    }
}

impl NativeCloseInitialContinuation {
    #[allow(clippy::too_many_lines)] // Cursor transitions cover each original process resource with custody retained across awaits.
    pub(super) async fn step_process_retirement(
        &mut self,
        live_source: &phoenix_core::domain::sm_state::ConvState,
    ) -> Result<Phase, NativeCloseApplicationError> {
        if !self.live_source_matches(live_source) {
            self.abort_inventory_preparation();
            self.phase = Phase::InspectionRequired;
            return Err(NativeCloseApplicationError::SourceRefused);
        }
        // Keep the cursor in the continuation across every await/error path.
        let progress = self
            .process_retirement
            .as_mut()
            .expect("captured inventory owns runtime cursor");
        if progress.blocked.is_some() {
            self.phase = Phase::ProcessRetirementBlocked;
            return Ok(self.phase);
        }
        let Some((scope, step)) = progress.current() else {
            self.phase = Phase::RuntimeProcessesDone;
            return Ok(self.phase);
        };
        let resources = progress.current_resources(&scope, step);
        progress.retry_backoff = false;
        if !resources.is_empty()
            && progress.resource_index == 0
            && progress.pending_dispatches.is_empty()
            && progress.entry.is_none()
            && progress.pending.is_none()
            && progress.observer.is_none()
        {
            let requests = resources
                .iter()
                .map(|resource| RecordCloseRetirementDispatchRequest {
                    attempt_id: resource.attempt_id.clone(),
                    scope: resource.scope.clone(),
                    snapshot: resource.snapshot.clone(),
                    resource: resource.resource.clone(),
                })
                .collect::<Vec<_>>();
            match self
                .db
                .record_native_close_initial_process_dispatches(
                    &self.command,
                    &self.receipt,
                    live_source,
                    &requests,
                )
                .await
            {
                LocalAuthorityResult::DurableFactEstablished(Ok(
                    CloseRetirementBatchDispatch::First(dispatches),
                )) => {
                    progress.pending_dispatches = dispatches.into();
                }
                LocalAuthorityResult::DurableFactEstablished(Ok(
                    CloseRetirementBatchDispatch::Replay,
                )) => {
                    self.phase = Phase::ObservingTerminal;
                }
                LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                    tracing::warn!(%error, "native initial process dispatch batch rolled back");
                    progress.retry_backoff = true;
                }
                LocalAuthorityResult::DurableFactUnclassified => {
                    return Err(NativeCloseApplicationError::FatalAuthority)
                }
            }
            return Ok(self.phase);
        }
        let Some(resource) = resources.get(progress.resource_index).cloned() else {
            progress.advance();
            return Ok(self.phase);
        };
        // Dispatches above cover the complete original kind inventory; admission
        // below transfers exactly ONE prepared resource under current source/Stop.
        let resources = [resource.clone()];
        if progress.pending.is_none() && progress.observer.is_none() {
            if let Some(entry) = &progress.entry {
                if !entry.is_finished() {
                    return Ok(self.phase);
                }
                match progress.entry.take().expect("entry handle retained").await {
                    Ok(Ok(observer)) => progress.observer = Some(observer),
                    Ok(Err(NativeInitialStepError::FatalAuthority)) => {
                        return Err(NativeCloseApplicationError::FatalAuthority)
                    }
                    Ok(Err(error)) => {
                        progress.blocked = Some(BlockedProcess::Worker { _error: error });
                    }
                    Err(error) => {
                        progress.blocked = Some(BlockedProcess::Join {
                            _error: error.to_string(),
                        });
                    }
                }
                if progress.blocked.is_some() {
                    self.phase = Phase::ProcessRetirementBlocked;
                }
                return Ok(self.phase);
            }
            if progress.worker.is_none() {
                let Some(owner) = self.authority.owner.as_mut() else {
                    return Err(NativeCloseApplicationError::SourceRefused);
                };
                let source_custody = owner.reborrow();
                let target_custody = std::sync::Arc::clone(&self.target_gate);
                let backend = std::sync::Arc::clone(&self.backend);
                let custody = phoenix_tools::browser::session::BrowserRetirementCustody::new(
                    std::sync::Arc::new((source_custody, target_custody)),
                );
                progress.worker = Some(tokio::spawn(async move {
                    backend
                        .prepare_native_initial_resource(resource, custody)
                        .await
                }));
                return Ok(self.phase);
            }
            if !progress
                .worker
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                return Ok(self.phase);
            }
            let result = progress
                .worker
                .take()
                .expect("preparation owns inert worker")
                .await;
            let prepared = match result {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(NativeInitialStepError::FatalAuthority)) => {
                    return Err(NativeCloseApplicationError::FatalAuthority)
                }
                other => {
                    let blocked = match other {
                        Ok(Err(error)) => BlockedProcess::Worker { _error: error },
                        Err(error) => BlockedProcess::Join {
                            _error: error.to_string(),
                        },
                        Ok(Ok(_)) => unreachable!("prepared owner handled above"),
                    };
                    progress.blocked = Some(blocked);
                    self.phase = Phase::ProcessRetirementBlocked;
                    return Ok(self.phase);
                }
            };
            // No SQLite callback waits for locks or polls an effect. Missing DB
            // return/release keeps the independently retained owner quarantined.
            let Some(source_entry) = self.source_entry.clone() else {
                progress.blocked = Some(BlockedProcess::Worker {
                    _error: NativeInitialStepError::FatalAuthority,
                });
                return Err(NativeCloseApplicationError::FatalAuthority);
            };
            let db = self.db.clone();
            let command = self.command.clone();
            let receipt = self.receipt.clone();
            // This snapshot is only an input to the durable validator. It cannot
            // open eligibility: the actor-owned worker gates the physical sink.
            let durable_source = live_source.clone();
            let backend = Arc::clone(&self.backend);
            let dispatch = progress
                .pending_dispatches
                .pop_front()
                .expect("prepared process resource retains its first dispatch");
            progress.entry = Some(spawn_retained_entry(backend, async move {
                Box::pin(prepared.enter(
                    &db,
                    &command,
                    &receipt,
                    &durable_source,
                    &source_entry,
                    dispatch,
                ))
                .await
            }));
            return Ok(self.phase);
        }
        if progress.pending.is_none() {
            let mut observer = progress
                .observer
                .as_ref()
                .expect("entered resource retains external observation")
                .lock()
                .await;
            if !observer.task().is_finished() {
                return Ok(self.phase);
            }
            match observer.observe().await {
                Ok(Ok(observation)) => progress.pending = Some(observation),
                Ok(Err(NativeInitialStepError::FatalAuthority)) => {
                    return Err(NativeCloseApplicationError::FatalAuthority)
                }
                Ok(Err(error)) => progress.blocked = Some(BlockedProcess::Worker { _error: error }),
                Err(error) => {
                    progress.blocked = Some(BlockedProcess::Join {
                        _error: format!("{error:?}"),
                    });
                }
            }
            if progress.blocked.is_some() {
                self.phase = Phase::ProcessRetirementBlocked;
                return Ok(self.phase);
            }
        }
        let observation = progress
            .pending
            .as_ref()
            .expect("finished worker retains observation");
        // The worker verifies exact membership; additionally tie each returned
        // observation to the actor's currently dispatched kind before any write.
        let (process, outcome) = match observation {
            NativeInitialObservation::Process {
                kind,
                identities,
                outcome,
                observed_at_us,
            } => {
                let expected_kind = match step {
                    NativeInitialProcessStep::Bash => {
                        phoenix_db::CloseProcessResourceKind::BashProcessGroup
                    }
                    NativeInitialProcessStep::Terminal => {
                        phoenix_db::CloseProcessResourceKind::PtySession
                    }
                    NativeInitialProcessStep::Browser => {
                        phoenix_db::CloseProcessResourceKind::BrowserSession
                    }
                    NativeInitialProcessStep::Tmux => {
                        return Err(NativeCloseApplicationError::FatalAuthority)
                    }
                };
                if *kind != expected_kind || identities.len() != resources.len() {
                    return Err(NativeCloseApplicationError::FatalAuthority);
                }
                (
                    Some((*kind, identities, *outcome, *observed_at_us)),
                    match outcome {
                        CloseProcessStepOutcome::Retired => RetirementOutcome::Retired,
                        CloseProcessStepOutcome::AbsenceVerified => {
                            RetirementOutcome::AbsenceAdopted {
                                absence_basis: AbsenceBasis::SameAttemptPriorRetirement,
                            }
                        }
                    },
                )
            }
            NativeInitialObservation::Tmux {
                resources: observed,
                outcome,
                ..
            } => {
                if step != NativeInitialProcessStep::Tmux || observed.len() != resources.len() {
                    return Err(NativeCloseApplicationError::FatalAuthority);
                }
                (None, outcome.clone())
            }
        };
        let Some(resource) = resources.get(progress.evidence_index) else {
            progress.advance();
            return Ok(self.phase);
        };
        if let Some((kind, identities, process_outcome, observed_at_us)) = process {
            let LossItemIdentity::Opaque(identity) = resource.resource.identity() else {
                return Err(NativeCloseApplicationError::FatalAuthority);
            };
            if !identities.contains(identity) {
                return Err(NativeCloseApplicationError::FatalAuthority);
            }
            if !progress.process_receipt_recorded {
                match self
                    .db
                    .record_native_close_initial_process_step_success(
                        &self.command,
                        &self.receipt,
                        live_source,
                        resource.snapshot.clone(),
                        CloseProcessStepSuccess {
                            run: CloseRunRef::initial(self.receipt.admitted.attempt_id.clone()),
                            scope: scope.clone(),
                            resource_kind: kind,
                            identity: identity.clone(),
                            outcome: process_outcome,
                            observed_at_us,
                        },
                    )
                    .await
                {
                    LocalAuthorityResult::DurableFactEstablished(Ok(())) => {
                        progress.process_receipt_recorded = true;
                    }
                    LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                        progress.retry_backoff = true;
                        tracing::warn!(%error, "native initial process evidence rolled back; retrying evidence only");
                    }
                    LocalAuthorityResult::DurableFactUnclassified => {
                        return Err(NativeCloseApplicationError::FatalAuthority)
                    }
                }
                return Ok(self.phase);
            }
        }
        match self
            .db
            .record_native_close_initial_retirement_evidence(
                &self.command,
                &self.receipt,
                live_source,
                RecordCloseRetirementEvidenceRequest {
                    attempt_id: resource.attempt_id.clone(),
                    scope: resource.scope.clone(),
                    snapshot: resource.snapshot.clone(),
                    resource: resource.resource.clone(),
                    outcome,
                    detail: None,
                },
            )
            .await
        {
            LocalAuthorityResult::DurableFactEstablished(Ok(())) => {
                // Observation is durable before selecting the next original
                // resource. Never rerun an effect to retry evidence persistence.
                progress.resource_index += 1;
                progress.pending = None;
                progress.observer = None;
                progress.process_receipt_recorded = false;
            }
            LocalAuthorityResult::DurableFactEstablished(Err(error)) => {
                progress.retry_backoff = true;
                tracing::warn!(%error, "native initial retirement evidence rolled back; retrying evidence only");
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                return Err(NativeCloseApplicationError::FatalAuthority)
            }
        }
        Ok(self.phase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_only_inventory_needs_original_source_without_process_leases() {
        use phoenix_core::domain::close::{
            CloseAttemptId, CloseRetirementSnapshot, GitPathIdentity, LossItemIdentity,
            RetiredResourceIdentity, WorktreeFingerprint, WorktreeId, WorktreeIdentity,
        };

        let scope = WorkScopeId::parse("scope").unwrap();
        let snapshot = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let resource = CloseExpectedRetirementResource {
            attempt_id: CloseAttemptId::parse("attempt").unwrap(),
            scope: scope.clone(),
            snapshot: snapshot.clone(),
            resource: RetiredResourceIdentity::parse(
                RetiredResourceKind::Worktree,
                LossItemIdentity::Worktree(WorktreeIdentity::from_parts(
                    WorktreeId::parse("linked").unwrap(),
                    WorktreeFingerprint::parse("fingerprint").unwrap(),
                    GitPathIdentity::from_bytes(b"linked".to_vec()),
                )),
            )
            .unwrap(),
        };
        let worktree =
            ProcessRetirement::new(vec![resource], vec![scope.clone()], snapshot.clone());
        assert!(!worktree.has_resources());
        assert!(worktree.has_original_effects());
        let no_physical_resources = ProcessRetirement::new(Vec::new(), vec![scope], snapshot);
        assert!(!no_physical_resources.has_resources());
        assert!(!no_physical_resources.has_original_effects());
    }

    #[tokio::test]
    async fn exit_custody_poll_waits_for_retained_process_entry_before_classifying() {
        use super::super::post_retirement::ExitCustodyPoll;

        let snapshot =
            phoenix_core::domain::close::CloseRetirementSnapshot::parse("original", "fingerprint")
                .unwrap();
        let mut progress = ProcessRetirement::new(Vec::new(), Vec::new(), snapshot);
        let (release, released) = tokio::sync::oneshot::channel();
        let (finished, completion) = tokio::sync::oneshot::channel();
        progress.entry = Some(tokio::spawn(async move {
            released.await.unwrap();
            // No await follows this witness: on this current-thread runtime
            // the JoinHandle is finished before the observing test resumes.
            finished.send(()).unwrap();
            Err(NativeInitialStepError::FatalAuthority)
        }));

        assert!(matches!(
            progress
                .poll_exit_custody(Some(
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                ))
                .await,
            ExitCustodyPoll::Pending
        ));
        assert!(progress.has_unresolved_exit_custody());

        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), completion)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            progress
                .poll_exit_custody(Some(
                    std::time::Instant::now() + std::time::Duration::from_secs(1),
                ))
                .await,
            ExitCustodyPoll::Fatal
        ));
        assert!(!progress.has_unresolved_exit_custody());
    }

    #[tokio::test]
    async fn dropped_entry_reply_still_latches_fatal_writer_classification() {
        let backend = Arc::new(crate::runtime::RuntimeManager::new(
            phoenix_db::Database::open_in_memory().await.unwrap(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            crate::platform::PlatformCapability::None {
                details: "retained entry test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        let mut fatal = backend.fatal_local_authority_fence.tx.subscribe();
        let (release, released) = tokio::sync::oneshot::channel();
        let task = spawn_retained_entry(Arc::clone(&backend), async move {
            released.await.unwrap();
            Err(NativeInitialStepError::FatalAuthority)
        });
        // Equivalent to Stop/terminal observation releasing the cursor while
        // first-effect writer classification is still pending. No native work.
        drop(task);
        assert!(!backend.fatal_local_authority_fence.is_closed());
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), fatal.changed())
            .await
            .expect("detached entry must publish its fatal classification")
            .unwrap();
        assert_eq!(*fatal.borrow(), Some("native_close_initial_retained_entry"));
        assert!(backend
            .fatal_local_authority_fence
            .fatal_authority_loss_is_latched());
    }
}
