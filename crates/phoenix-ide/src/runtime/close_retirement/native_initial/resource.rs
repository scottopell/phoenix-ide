//! Exact-resource preparation and externally retained initial entry.
//!
//! The source actor alone admits an owner. A separate task retains that owner
//! across cancellation and cannot construct its effect future before confirmed
//! SQLite writer release. These records are process-epoch custody, not recovery
//! permission after a restart.

use super::super::{
    require_browser_absent, require_terminal_absent, tmux_retirement_outcome, RuntimeManager,
};
use super::bash::{BashCustodySlot, BashCustodyState, BashEntryContext, BashStage};
use super::supervision::{EntryDecision, EntryGate, ResourceSupervisor};
use super::{NativeInitialObservation, NativeInitialStepError};
use crate::runtime::executor::{SourceEntryWorker, WorkerEntryResult};
use phoenix_core::domain::close::{
    CloseExpectedRetirementResource, LossItemIdentity, RetiredResourceKind,
};
use phoenix_db::native_close::{
    NativeCloseCommand, NativeCloseInitialEntryRelease, NativeCloseInitialResourceEntryDecision,
    NativeCloseInitialResourceEntryResult, NativeCloseReceipt,
};
use phoenix_db::Database;
use phoenix_terminal::session::{
    EnteredTerminalRetirement, PreparedTerminalRetirement, TerminalRetirementEntryError,
};
use phoenix_tools::{
    browser::session::{BrowserPreparedRetirement, BrowserRetirementCustody},
    tmux::registry::PreparedTmuxRetirement,
};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use tokio::sync::Mutex;
use tokio::sync::OwnedMutexGuard;

pub(crate) type ResourceObservation = Result<NativeInitialObservation, NativeInitialStepError>;
pub(crate) type ResourceObserver = Arc<Mutex<ResourceSupervisor<ResourceObservation>>>;

#[cfg(test)]
pub(crate) struct TerminalEntryTestHook {
    pub(crate) scope: phoenix_core::work_scope::WorkScopeId,
    pub(crate) reached: Option<tokio::sync::oneshot::Sender<bool>>,
    pub(crate) resume: tokio::sync::oneshot::Receiver<()>,
    pub(crate) pending: Option<tokio::sync::oneshot::Sender<bool>>,
    pub(crate) callback: Option<tokio::sync::oneshot::Sender<(bool, bool, bool)>>,
    pub(crate) completed: Option<tokio::sync::oneshot::Sender<()>>,
}

#[cfg(test)]
impl TerminalEntryTestHook {
    async fn observe_db_wait<F: std::future::Future>(
        &mut self,
        eligible_unrevoked_before_deadline: bool,
        future: F,
    ) -> F::Output {
        if let Some(reached) = self.reached.take() {
            let _ = reached.send(eligible_unrevoked_before_deadline);
        }
        let _ = (&mut self.resume).await;
        let mut future = std::pin::pin!(future);
        let first = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(future.as_mut(), cx))
        })
        .await;
        if let Some(pending) = self.pending.take() {
            let _ = pending.send(first.is_pending());
        }
        match first {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => future.await,
        }
    }
}

#[cfg(test)]
impl Drop for TerminalEntryTestHook {
    fn drop(&mut self) {
        if let Some(completed) = self.completed.take() {
            let _ = completed.send(());
        }
    }
}

pub(crate) enum TerminalCustodyState {
    Preparing,
    Prepared(PreparedTerminalRetirement),
    Refused {
        owner: PreparedTerminalRetirement,
        error: TerminalRetirementEntryError,
    },
    Entered(EnteredTerminalRetirement),
    Completing,
    Complete,
}

pub(crate) type TerminalCustodySlot = Arc<Mutex<TerminalCustodyState>>;

impl TerminalCustodyState {
    fn try_enter(&mut self) -> Result<(), TerminalRetirementEntryError> {
        let state = std::mem::replace(self, Self::Completing);
        match state {
            Self::Prepared(owner) => match owner.try_enter() {
                Ok(entered) => {
                    *self = Self::Entered(entered);
                    Ok(())
                }
                Err((owner, error)) => {
                    *self = Self::Refused { owner, error };
                    Err(error)
                }
            },
            Self::Refused { owner, error } => {
                *self = Self::Refused { owner, error };
                Err(error)
            }
            other => {
                *self = other;
                Err(TerminalRetirementEntryError::AlreadyRequested)
            }
        }
    }
}

pub(crate) struct NativeInitialResourceRecord {
    pub(crate) expected: CloseExpectedRetirementResource,
    pub(crate) observer: ResourceObserver,
    state: Arc<AtomicU8>,
    pub(crate) terminal: TerminalCustodySlot,
    pub(crate) bash: BashCustodySlot,
    _browser_custody: Arc<std::sync::Mutex<Option<BrowserRetirementCustody>>>,
}

const RESERVED: u8 = 0;
const ENTERED: u8 = 1;
const CANCELLED_BEFORE_ENTRY: u8 = 2;
const RETAINED_REFUSAL: u8 = 3;

/// Actor-local reservation lifetime. Dropping an inert preparation or rejected
/// admission makes only that reservation reusable; transferred ownership can
/// never be reset, including when writer release is unknown.
struct EntryReservation {
    state: Arc<AtomicU8>,
}

impl Drop for EntryReservation {
    fn drop(&mut self) {
        let _ = self.state.compare_exchange(
            RESERVED,
            CANCELLED_BEFORE_ENTRY,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

impl NativeInitialResourceRecord {
    pub(crate) fn retains_entry(&self) -> bool {
        self.state.load(Ordering::Acquire) != CANCELLED_BEFORE_ENTRY
    }
}

enum PreparedResource {
    Bash(BashCustodySlot),
    Tmux(PreparedTmuxRetirement),
    Terminal(TerminalCustodySlot),
    Browser(BrowserPreparedRetirement),
}

struct EnteredResource {
    resource: PreparedResource,
    bash_context: Option<BashEntryContext>,
    expected: CloseExpectedRetirementResource,
    custody: BrowserRetirementCustody,
}

impl EnteredResource {
    async fn execute(self) -> ResourceObservation {
        let Self {
            resource,
            expected,
            custody,
            bash_context,
        } = self;
        let kind = expected.resource.kind();
        let failure = |detail: String| NativeInitialStepError::ProcessFailure {
            kind,
            resources: vec![expected.resource.clone()],
            detail,
        };
        let outcome = match resource {
            PreparedResource::Tmux(owner) => {
                let outcome = owner
                    .complete()
                    .await
                    .map_err(|error| failure(error.to_string()))?;
                let outcome =
                    tmux_retirement_outcome(outcome).map_err(|(_, detail)| failure(detail))?;
                // Keep source/target custody until the physical owner reports completion.
                drop(custody);
                return Ok(NativeInitialObservation::Tmux {
                    resources: vec![expected.resource],
                    outcome,
                    observed_at_us: chrono::Utc::now().timestamp_micros(),
                });
            }
            PreparedResource::Bash(slot) => {
                let context = bash_context.ok_or(NativeInitialStepError::FatalAuthority)?;
                match context.complete(slot, &expected).await {
                    Ok(outcome) => Ok(outcome),
                    Err(error) => return Err(error),
                }
            }
            PreparedResource::Terminal(slot) => {
                let entered = {
                    let mut state = slot.lock().await;
                    match std::mem::replace(&mut *state, TerminalCustodyState::Completing) {
                        TerminalCustodyState::Entered(entered) => entered,
                        other => {
                            *state = other;
                            return Err(NativeInitialStepError::FatalAuthority);
                        }
                    }
                };
                // Completion runs in the retained worker after confirmed writer release;
                // this does not move the actor's observation wait off its current path.
                let outcome = require_terminal_absent(entered.complete().await);
                *slot.lock().await = TerminalCustodyState::Complete;
                outcome
            }
            PreparedResource::Browser(owner) => {
                require_browser_absent(owner.execute(custody.clone()).await)
            }
        }
        .map_err(failure)?;
        let process_kind = match kind {
            RetiredResourceKind::BashProcessGroup => {
                phoenix_db::CloseProcessResourceKind::BashProcessGroup
            }
            RetiredResourceKind::PtySession => phoenix_db::CloseProcessResourceKind::PtySession,
            RetiredResourceKind::BrowserSession => {
                phoenix_db::CloseProcessResourceKind::BrowserSession
            }
            _ => return Err(NativeInitialStepError::SealedInventoryMismatch),
        };
        let LossItemIdentity::Opaque(identity) = expected.resource.identity() else {
            return Err(NativeInitialStepError::SealedInventoryMismatch);
        };
        let identities = vec![identity.clone()];
        drop(custody);
        Ok(NativeInitialObservation::Process {
            kind: process_kind,
            identities,
            outcome,
            observed_at_us: chrono::Utc::now().timestamp_micros(),
        })
    }
}

fn release_browser_custody_after_verified_result<T, E>(
    custody: &Arc<std::sync::Mutex<Option<BrowserRetirementCustody>>>,
    result: &Result<T, E>,
) {
    if result.is_ok() {
        let released = custody
            .lock()
            .expect("browser custody lock poisoned")
            .take();
        drop(released);
    }
}

/// Prepared outside SQLite; dropping before entry has no physical effects.
/// The observer has already been retained in the backend, independently of this
/// actor-local value. After transfer, even loss of this future cannot release it.
pub(crate) struct NativeInitialPreparedEntry {
    fatal: Arc<crate::runtime::FatalLocalAuthorityFence>,
    expected: CloseExpectedRetirementResource,
    owner: EnteredResource,
    gate: EntryGate<EnteredResource>,
    observer: ResourceObserver,
    reservation: EntryReservation,
    terminal: TerminalCustodySlot,
    #[cfg(test)]
    test_hook: Option<TerminalEntryTestHook>,
}

/// Cancellation/panic after first writer entry closes local authority. The
/// backend record independently retains the complete physical owner.
struct EntryAuthorityGuard {
    fatal: Arc<crate::runtime::FatalLocalAuthorityFence>,
    armed: bool,
}

impl Drop for EntryAuthorityGuard {
    fn drop(&mut self) {
        if self.armed {
            self.fatal.close("native_close_initial_resource_entry");
        }
    }
}

impl NativeInitialPreparedEntry {
    pub(crate) async fn enter(
        self,
        db: &Database,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
        live_source: &phoenix_core::domain::sm_state::ConvState,
        source_entry: &SourceEntryWorker,
    ) -> Result<ResourceObserver, NativeInitialStepError> {
        let Self {
            fatal,
            expected,
            mut owner,
            mut gate,
            observer,
            reservation,
            terminal,
            #[cfg(test)]
            mut test_hook,
        } = self;
        if let PreparedResource::Bash(slot) = &owner.resource {
            let mut authority = EntryAuthorityGuard {
                fatal: Arc::clone(&fatal),
                armed: false,
            };
            let slot = Arc::clone(slot);
            owner.bash_context = Some(BashEntryContext {
                db: db.clone(),
                command: command.clone(),
                receipt: receipt.clone(),
                live_source: live_source.clone(),
                source_entry: source_entry.clone(),
                fatal,
            });
            // The independently retained record already owns the full native
            // resource. Acquire its lock before SQLite, never in the callback.
            let mut state = slot.lock().await;
            if state.stage != BashStage::Prepared {
                return Err(NativeInitialStepError::FatalAuthority);
            }
            state.stage = BashStage::StopWriterPending;
            authority.armed = true;
            let result = db
                .with_native_close_initial_process_first_effect(
                    command,
                    receipt,
                    live_source,
                    expected.scope.clone(),
                    expected.snapshot.clone(),
                    &expected.resource,
                    || {
                        let admission = source_entry.try_first_effect(command, || {
                            #[cfg(test)]
                            if let Some(hook) = state.test_hook.as_ref() {
                                hook.stats.stop_attempts.fetch_add(1, Ordering::SeqCst);
                            }
                            state
                                .owner
                                .as_mut()
                                .expect("record retains Bash owner")
                                .try_request_stop()
                        });
                        match admission {
                            WorkerEntryResult::Ran(result) => {
                                state.stop_failure = result.err().map(|error| error.to_string());
                                reservation.state.store(ENTERED, Ordering::Release);
                                match gate.transfer(owner) {
                                    EntryDecision::Transferred => {
                                        NativeCloseInitialResourceEntryDecision::Entered
                                    }
                                    EntryDecision::NotTransferred(_shell) => {
                                        reservation
                                            .state
                                            .store(RETAINED_REFUSAL, Ordering::Release);
                                        NativeCloseInitialResourceEntryDecision::Refused
                                    }
                                }
                            }
                            WorkerEntryResult::Busy | WorkerEntryResult::Refused => {
                                reservation.state.store(RETAINED_REFUSAL, Ordering::Release);
                                NativeCloseInitialResourceEntryDecision::Refused
                            }
                        }
                    },
                )
                .await;
            match result {
                Ok((
                    NativeCloseInitialResourceEntryDecision::Entered,
                    NativeCloseInitialEntryRelease::Confirmed,
                )) => {
                    state.stage = BashStage::StopReleased;
                    drop(state);
                    if !gate.confirm_release() {
                        return Err(NativeInitialStepError::FatalAuthority);
                    }
                    authority.armed = false;
                    return Ok(observer);
                }
                Ok((
                    NativeCloseInitialResourceEntryDecision::Refused,
                    NativeCloseInitialEntryRelease::Confirmed,
                ))
                | Err(phoenix_db::native_close::NativeCloseInitialFirstEffectError::NotStarted(
                    _,
                ))
                | Err(phoenix_db::native_close::NativeCloseInitialFirstEffectError::Rejected {
                    release: NativeCloseInitialEntryRelease::Confirmed,
                    ..
                }) => {
                    authority.armed = false;
                    // A failed handoff may follow a real stop. Every writer is
                    // definitely released: compensate explicitly, even revoked.
                    state.stage = BashStage::WritersReleased;
                    let cleanup = state
                        .owner
                        .as_mut()
                        .expect("retained owner")
                        .cleanup_after_writer_release();
                    state.stage = BashStage::CleanupBlocked;
                    gate.cancel_before_entry();
                    return Err(NativeInitialStepError::ProcessFailure {
                        kind: expected.resource.kind(), resources: vec![expected.resource],
                        detail: format!("Bash stop admission refused; original owner retained; cleanup: {cleanup:?}"),
                    });
                }
                _ => {
                    state.stage = BashStage::Quarantined;
                    reservation.state.store(ENTERED, Ordering::Release);
                    return Err(NativeInitialStepError::FatalAuthority);
                }
            }
        }
        if let PreparedResource::Terminal(slot) = &owner.resource {
            let mut slot_guard: OwnedMutexGuard<TerminalCustodyState> =
                Arc::clone(slot).lock_owned().await;
            let reservation_state = Arc::clone(&reservation.state);
            #[cfg(test)]
            let callback = test_hook.as_mut().and_then(|hook| hook.callback.take());
            let mut authority = EntryAuthorityGuard {
                fatal: Arc::clone(&fatal),
                armed: true,
            };
            let entry = db.with_native_close_initial_process_first_effect(
                command,
                receipt,
                live_source,
                expected.scope.clone(),
                expected.snapshot.clone(),
                &expected.resource,
                || {
                    #[cfg(test)]
                    let already_revoked = source_entry.is_revoked_for_test();
                    let admission =
                        source_entry.try_first_effect(command, || slot_guard.try_enter());
                    // Sample AFTER refusal: monotonic time then excludes expiry
                    // inside try_first_effect, even if this task was preempted.
                    #[cfg(test)]
                    let original_deadline_unexpired =
                        source_entry.original_deadline_unexpired_for_test();
                    #[cfg(test)]
                    if let Some(callback) = callback {
                        let _ = callback.send((
                            already_revoked,
                            original_deadline_unexpired,
                            matches!(&admission, WorkerEntryResult::Refused),
                        ));
                    }
                    match admission {
                        WorkerEntryResult::Ran(Ok(())) => {
                            // Retain actual entry before any handoff/release await.
                            reservation_state.store(ENTERED, Ordering::Release);
                            match gate.transfer(owner) {
                                EntryDecision::Transferred => {
                                    reservation_state.store(ENTERED, Ordering::Release);
                                    NativeCloseInitialResourceEntryDecision::Entered
                                }
                                EntryDecision::NotTransferred(_owner) => {
                                    reservation_state.store(RETAINED_REFUSAL, Ordering::Release);
                                    NativeCloseInitialResourceEntryDecision::Refused
                                }
                            }
                        }
                        WorkerEntryResult::Ran(Err(_))
                        | WorkerEntryResult::Busy
                        | WorkerEntryResult::Refused => {
                            reservation_state.store(RETAINED_REFUSAL, Ordering::Release);
                            NativeCloseInitialResourceEntryDecision::Refused
                        }
                    }
                },
            );
            #[cfg(test)]
            let result = match test_hook.as_mut() {
                Some(hook) => {
                    hook.observe_db_wait(
                        source_entry.is_eligible_unrevoked_before_deadline_for_test(),
                        entry,
                    )
                    .await
                }
                None => entry.await,
            };
            #[cfg(not(test))]
            let result = entry.await;
            drop(slot_guard);
            return match result {
                Ok((
                    NativeCloseInitialResourceEntryDecision::Entered,
                    NativeCloseInitialEntryRelease::Confirmed,
                )) => {
                    if !gate.confirm_release() {
                        return Err(NativeInitialStepError::FatalAuthority);
                    }
                    authority.armed = false;
                    Ok(observer)
                }
                Ok((
                    NativeCloseInitialResourceEntryDecision::Refused,
                    NativeCloseInitialEntryRelease::Confirmed,
                )) => {
                    authority.armed = false;
                    gate.cancel_before_entry();
                    match &*terminal.lock().await {
                        TerminalCustodyState::Refused { error, .. } => Err(NativeInitialStepError::ProcessFailure {
                            kind: expected.resource.kind(), resources: vec![expected.resource],
                            detail: format!("terminal first-effect refused: {error:?}"),
                        }),
                        TerminalCustodyState::Prepared(_) => Err(NativeInitialStepError::ProcessFailure {
                            kind: expected.resource.kind(), resources: vec![expected.resource],
                            detail: "live source entry unavailable or revoked; original terminal owner retained".into(),
                        }),
                        _ => Err(NativeInitialStepError::FatalAuthority),
                    }
                }
                Err(phoenix_db::native_close::NativeCloseInitialFirstEffectError::NotStarted(
                    error,
                )) => {
                    authority.armed = false;
                    gate.cancel_before_entry();
                    Err(NativeInitialStepError::ProcessFailure {
                        kind: expected.resource.kind(),
                        resources: vec![expected.resource],
                        detail: format!("terminal entry not started: {error:?}"),
                    })
                }
                Err(phoenix_db::native_close::NativeCloseInitialFirstEffectError::Rejected {
                    error,
                    release: NativeCloseInitialEntryRelease::Confirmed,
                }) => {
                    authority.armed = false;
                    gate.cancel_before_entry();
                    Err(NativeInitialStepError::ProcessFailure {
                        kind: expected.resource.kind(),
                        resources: vec![expected.resource],
                        detail: format!("terminal entry rejected: {error:?}"),
                    })
                }
                _ => {
                    reservation.state.store(ENTERED, Ordering::Release);
                    Err(NativeInitialStepError::FatalAuthority)
                }
            };
        }
        let result = db
            .enter_native_close_initial_process_resource(
                command,
                receipt,
                live_source,
                expected.scope.clone(),
                expected.snapshot.clone(),
                &expected.resource,
                || match source_entry.try_first_effect(command, || gate.transfer(owner)) {
                    WorkerEntryResult::Ran(EntryDecision::Transferred) => {
                        reservation.state.store(ENTERED, Ordering::Release);
                        NativeCloseInitialResourceEntryDecision::Entered
                    }
                    WorkerEntryResult::Ran(EntryDecision::NotTransferred(owner)) => {
                        // No supervisor accepted this owner; it is still inert.
                        drop(owner);
                        NativeCloseInitialResourceEntryDecision::Refused
                    }
                    WorkerEntryResult::Busy | WorkerEntryResult::Refused => {
                        NativeCloseInitialResourceEntryDecision::Refused
                    }
                },
            )
            .await;
        match result {
            NativeCloseInitialResourceEntryResult::Invoked {
                decision: NativeCloseInitialResourceEntryDecision::Entered,
                release: NativeCloseInitialEntryRelease::Confirmed,
            } => {
                if !gate.confirm_release() {
                    return Err(NativeInitialStepError::FatalAuthority);
                }
                Ok(observer)
            }
            NativeCloseInitialResourceEntryResult::NotStarted(error)
            | NativeCloseInitialResourceEntryResult::Rejected {
                error,
                release: NativeCloseInitialEntryRelease::Confirmed,
            } => {
                gate.cancel_before_entry();
                Err(NativeInitialStepError::ProcessFailure {
                    kind: expected.resource.kind(),
                    resources: vec![expected.resource],
                    detail: error.to_string(),
                })
            }
            // Missing/unconfirmed release quarantines the transferred owner.
            // It is neither permission to execute nor evidence of non-entry.
            _ => Err(NativeInitialStepError::FatalAuthority),
        }
    }
}

impl RuntimeManager {
    #[cfg(test)]
    pub(crate) async fn set_native_bash_stage2_test_hook(
        &self,
        hook: super::bash::BashStage2TestHook,
    ) {
        self.close_retirement_leases.lock().await.bash_stage2_hook = Some(hook);
    }

    #[cfg(test)]
    pub(crate) async fn set_terminal_entry_test_hook(&self, hook: TerminalEntryTestHook) {
        let mut leases = self.close_retirement_leases.lock().await;
        assert!(leases.terminal_entry_hook.replace(hook).is_none());
    }

    pub(crate) async fn prepare_native_initial_resource(
        &self,
        expected: CloseExpectedRetirementResource,
        custody: BrowserRetirementCustody,
    ) -> Result<NativeInitialPreparedEntry, NativeInitialStepError> {
        if self
            .fatal_local_authority_fence
            .fatal_authority_loss_is_latched()
        {
            return Err(NativeInitialStepError::FatalAuthority);
        }
        // These legacy owners start nested physical work only after transfer.
        // A transfer callback is not admission at that physical sink. Preserve
        // the exact captured resource as blocked until its staged adapter exists;
        // never omit it or treat it as absent to advance scope finalization.
        if matches!(
            expected.resource.kind(),
            RetiredResourceKind::TmuxServer | RetiredResourceKind::BrowserSession
        ) {
            return Err(NativeInitialStepError::ProcessFailure {
                kind: expected.resource.kind(), resources: vec![expected.resource],
                detail: "native initial retirement requires a staged physical-effect adapter for this resource".into(),
            });
        }
        let key = (
            expected.attempt_id.as_str().to_string(),
            expected.scope.clone(),
        );
        let lease = {
            let mut leases = self.close_retirement_leases.lock().await;
            if leases
                .native_initial_supervisors
                .iter()
                .any(|record| record.expected == expected && record.retains_entry())
            {
                return Err(NativeInitialStepError::AlreadyEntered);
            }
            leases
                .reserve_native_initial_lease(&key)
                .map_err(|_| NativeInitialStepError::MissingLease)?
        };
        let lease = lease.lock().await;
        if !lease.capture_evidence.as_ref().is_some_and(|binding| {
            binding.verifies_request(&expected.snapshot)
                && binding.deadline() == lease.browser.deadline()
        }) || !lease.resources.contains(&expected.resource)
        {
            return Err(NativeInitialStepError::SealedInventoryMismatch);
        }
        // Claim the exact resource after the per-lease wait but BEFORE any
        // physical preparation. A rival must not wait on a prepared owner's
        // locks merely to discover that this resource was already reserved.
        let state = Arc::new(AtomicU8::new(RESERVED));
        let reservation = EntryReservation {
            state: Arc::clone(&state),
        };
        let terminal = Arc::new(Mutex::new(TerminalCustodyState::Preparing));
        let bash = Arc::new(Mutex::new(BashCustodyState::preparing()));
        let browser_custody = Arc::new(std::sync::Mutex::new(Some(custody.clone())));
        let worker_custody = Arc::clone(&browser_custody);
        let (gate, supervisor) =
            ResourceSupervisor::spawn(move |entered: EnteredResource| async move {
                let result = entered.execute().await;
                // The result is Ok only after the physical owner verified retirement.
                // Release the manager's live source/target pin in the retained worker,
                // independently of whether an actor is still observing this result.
                release_browser_custody_after_verified_result(&worker_custody, &result);
                result
            });
        let observer = Arc::new(Mutex::new(supervisor));
        {
            let mut leases = self.close_retirement_leases.lock().await;
            if leases
                .native_initial_supervisors
                .iter()
                .any(|record| record.expected == expected && record.retains_entry())
            {
                return Err(NativeInitialStepError::AlreadyEntered);
            }
            leases
                .native_initial_supervisors
                .retain(|record| record.expected != expected || record.retains_entry());
            leases
                .native_initial_supervisors
                .push(NativeInitialResourceRecord {
                    expected: expected.clone(),
                    observer: observer.clone(),
                    state,
                    terminal: Arc::clone(&terminal),
                    bash: Arc::clone(&bash),
                    _browser_custody: Arc::clone(&browser_custody),
                });
        }
        let LossItemIdentity::Opaque(identity) = expected.resource.identity() else {
            return Err(NativeInitialStepError::SealedInventoryMismatch);
        };
        let fail = |detail: String| NativeInitialStepError::ProcessFailure {
            kind: expected.resource.kind(),
            resources: vec![expected.resource.clone()],
            detail,
        };
        // Reserve the inert owner before the source-bound writer callback.
        // Completion may still await subsystem locks after ownership transfer;
        // that transfer is not proof of first physical effect or its completion.
        // Original permit deadlines are retained; no per-entry budget is minted.
        let resource = match expected.resource.kind() {
            RetiredResourceKind::BashProcessGroup => {
                let target = lease
                    .bash
                    .exact_process_groups
                    .iter()
                    .find(|target| target.stable_resource_identity() == identity.as_str())
                    .ok_or(NativeInitialStepError::SealedInventoryMismatch)?;
                let native = lease
                    .bash
                    .prepare_target(target, lease.browser.deadline())
                    .await
                    .map_err(|error| fail(error.to_string()))?
                    .prepare_native()
                    .await
                    .map_err(|error| fail(error.to_string()))?;
                let mut slot = bash.lock().await;
                slot.owner = Some(native);
                slot.stage = BashStage::Prepared;
                reservation.state.store(RETAINED_REFUSAL, Ordering::Release);
                #[cfg(test)]
                {
                    let mut leases = self.close_retirement_leases.lock().await;
                    if leases.bash_stage2_hook.as_ref().is_some_and(|hook| {
                        hook.scope == expected.scope && hook.pgid == target.pgid
                    }) {
                        slot.test_hook = leases.bash_stage2_hook.take();
                    }
                }
                // A retained registry writer is not a reusable inert reservation.
                // Never restore this original lease while its exact owner exists.
                drop(slot);
                PreparedResource::Bash(Arc::clone(&bash))
            }
            RetiredResourceKind::TmuxServer => {
                if lease.tmux.instance.stable_identity() != identity.as_str() {
                    return Err(NativeInitialStepError::SealedInventoryMismatch);
                }
                PreparedResource::Tmux(
                    self.tmux_registry()
                        .prepare_retirement(&lease.tmux)
                        .await
                        .map_err(|error| fail(format!("{error:?}")))?,
                )
            }
            RetiredResourceKind::PtySession => {
                if !lease
                    .terminal
                    .instance
                    .as_ref()
                    .is_some_and(|instance| instance.stable_identity() == identity.as_str())
                {
                    return Err(NativeInitialStepError::SealedInventoryMismatch);
                }
                let prepared = self
                    .terminals
                    .prepare_retirement(&lease.terminal)
                    .map_err(|error| fail(format!("{error:?}")))?;
                *terminal.lock().await = TerminalCustodyState::Prepared(prepared);
                PreparedResource::Terminal(Arc::clone(&terminal))
            }
            RetiredResourceKind::BrowserSession => {
                let instance = lease
                    .browser
                    .instances
                    .iter()
                    .find(|instance| instance.stable_identity() == identity.as_str())
                    .ok_or(NativeInitialStepError::SealedInventoryMismatch)?;
                PreparedResource::Browser(
                    self.browser_sessions()
                        .prepare_retirement_resource(
                            &lease.browser,
                            instance,
                            lease.browser.deadline(),
                        )
                        .await
                        .map_err(fail)?,
                )
            }
            _ => return Err(NativeInitialStepError::SealedInventoryMismatch),
        };
        let owner = EnteredResource {
            resource,
            expected: expected.clone(),
            custody,
            bash_context: None,
        };
        drop(lease);
        #[cfg(test)]
        let test_hook = {
            let mut leases = self.close_retirement_leases.lock().await;
            if expected.resource.kind() == RetiredResourceKind::PtySession
                && leases
                    .terminal_entry_hook
                    .as_ref()
                    .is_some_and(|hook| hook.scope == expected.scope)
            {
                leases.terminal_entry_hook.take()
            } else {
                None
            }
        };
        Ok(NativeInitialPreparedEntry {
            fatal: Arc::clone(&self.fatal_local_authority_fence),
            expected,
            owner,
            gate,
            observer,
            reservation,
            terminal,
            #[cfg(test)]
            test_hook,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CustodyDropProbe {
        dropped: Arc<std::sync::atomic::AtomicBool>,
        _target_guard: OwnedMutexGuard<()>,
    }

    impl Drop for CustodyDropProbe {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn verified_worker_result_releases_live_pin_but_keeps_cached_outcome() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let target_gate = Arc::new(Mutex::new(()));
        let target_guard = Arc::clone(&target_gate).lock_owned().await;
        let payload = Arc::new(CustodyDropProbe {
            dropped: Arc::clone(&dropped),
            _target_guard: target_guard,
        });
        let custody = Arc::new(std::sync::Mutex::new(Some(BrowserRetirementCustody::new(
            payload,
        ))));
        assert!(target_gate.try_lock().is_err());

        let worker_custody = Arc::clone(&custody);
        let (mut gate, mut supervisor) =
            ResourceSupervisor::<Result<&'static str, ()>>::spawn(move |()| async move {
                let result = Ok("verified");
                release_browser_custody_after_verified_result(&worker_custody, &result);
                result
            });
        assert!(matches!(gate.transfer(()), EntryDecision::Transferred));
        assert!(gate.confirm_release());

        assert_eq!(supervisor.observe().await, Ok(Ok("verified")));
        assert_eq!(supervisor.observe().await, Ok(Ok("verified")));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(target_gate.try_lock().is_ok());
        assert!(custody.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn unresolved_worker_result_retains_live_pin() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let target_gate = Arc::new(Mutex::new(()));
        let target_guard = Arc::clone(&target_gate).lock_owned().await;
        let payload = Arc::new(CustodyDropProbe {
            dropped: Arc::clone(&dropped),
            _target_guard: target_guard,
        });
        let custody = Arc::new(std::sync::Mutex::new(Some(BrowserRetirementCustody::new(
            payload,
        ))));
        let worker_custody = Arc::clone(&custody);
        let (mut gate, mut supervisor) =
            ResourceSupervisor::<Result<(), ()>>::spawn(move |()| async move {
                let result = Err(());
                release_browser_custody_after_verified_result(&worker_custody, &result);
                result
            });
        assert!(matches!(gate.transfer(()), EntryDecision::Transferred));
        assert!(gate.confirm_release());
        assert_eq!(supervisor.observe().await, Ok(Err(())));
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(target_gate.try_lock().is_err());
        assert!(custody.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn verified_release_unlocks_target_after_observer_disappears() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let target_gate = Arc::new(Mutex::new(()));
        let target_guard = Arc::clone(&target_gate).lock_owned().await;
        let payload = Arc::new(CustodyDropProbe {
            dropped: Arc::clone(&dropped),
            _target_guard: target_guard,
        });
        let custody = Arc::new(std::sync::Mutex::new(Some(BrowserRetirementCustody::new(
            payload,
        ))));
        let worker_custody = Arc::clone(&custody);
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();
        let (mut gate, supervisor) =
            ResourceSupervisor::<Result<(), ()>>::spawn(move |()| async move {
                let result = Ok(());
                release_browser_custody_after_verified_result(&worker_custody, &result);
                let _ = completed_tx.send(());
                result
            });
        assert!(matches!(gate.transfer(()), EntryDecision::Transferred));
        assert!(target_gate.try_lock().is_err());

        // Dropping the observer detaches its worker handle; release and completion
        // still proceed in the retained worker independently of result delivery.
        drop(supervisor);
        assert!(gate.confirm_release());
        completed_rx
            .await
            .expect("worker released custody before signaling completion");

        assert!(dropped.load(Ordering::SeqCst));
        assert!(target_gate.try_lock().is_ok());
        assert!(custody.lock().unwrap().is_none());
    }

    #[test]
    fn unresolved_first_writer_guard_latches_fatal_without_observer() {
        let fatal = crate::runtime::FatalLocalAuthorityFence::new();
        let guard = EntryAuthorityGuard {
            fatal: Arc::clone(&fatal),
            armed: true,
        };
        drop(guard);
        assert!(fatal.fatal_authority_loss_is_latched());
        assert!(fatal.try_acquire().is_err());
    }

    #[test]
    fn confirmed_first_writer_guard_does_not_latch_fatal() {
        let fatal = crate::runtime::FatalLocalAuthorityFence::new();
        let mut guard = EntryAuthorityGuard {
            fatal: Arc::clone(&fatal),
            armed: true,
        };
        guard.armed = false;
        drop(guard);
        assert!(!fatal.fatal_authority_loss_is_latched());
        assert!(fatal.try_acquire().is_ok());
    }

    #[test]
    fn rejected_or_dropped_preentry_reservation_is_not_entered() {
        let state = Arc::new(AtomicU8::new(RESERVED));
        let reservation = EntryReservation {
            state: Arc::clone(&state),
        };
        // Mirrors a definite DB validation rejection: the synchronous transfer
        // callback has not run, so dropping this actor-local value is nonentry.
        drop(reservation);
        assert_eq!(state.load(Ordering::Acquire), CANCELLED_BEFORE_ENTRY);
    }

    #[test]
    fn transferred_reservation_cannot_be_reused_when_writer_release_is_unknown() {
        let state = Arc::new(AtomicU8::new(RESERVED));
        let reservation = EntryReservation {
            state: Arc::clone(&state),
        };
        state.store(ENTERED, Ordering::Release);
        drop(reservation);
        assert_eq!(state.load(Ordering::Acquire), ENTERED);
    }

    #[test]
    fn retained_refusal_reservation_cannot_be_reused() {
        let state = Arc::new(AtomicU8::new(RESERVED));
        let reservation = EntryReservation {
            state: Arc::clone(&state),
        };
        state.store(RETAINED_REFUSAL, Ordering::Release);
        drop(reservation);
        assert_eq!(state.load(Ordering::Acquire), RETAINED_REFUSAL);
        assert_ne!(state.load(Ordering::Acquire), CANCELLED_BEFORE_ENTRY);
    }
}
