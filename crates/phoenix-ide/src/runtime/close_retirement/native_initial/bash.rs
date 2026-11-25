//! The record, not the observing actor or work future, owns the native Bash
//! resource through both independently admitted physical stages.

use std::sync::Arc;

use futures::FutureExt;
use phoenix_core::domain::{close::CloseExpectedRetirementResource, sm_state::ConvState};
use phoenix_db::native_close::{
    NativeCloseCommand, NativeCloseInitialEntryRelease, NativeCloseInitialFirstEffectError,
    NativeCloseReceipt,
};
use phoenix_db::{CloseProcessStepOutcome, Database};
use phoenix_tools::bash::registry::{NativeBashRetirement, NativeBashRetirementOutcome};
use tokio::sync::Mutex;

use super::NativeInitialStepError;
use crate::runtime::{
    executor::{SourceEntryWorker, WorkerEntryResult},
    FatalLocalAuthorityFence,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BashStage {
    Preparing,
    Prepared,
    StopWriterPending,
    StopReleased,
    StopObserved,
    GroupWriterPending,
    WritersReleased,
    CleanupBlocked,
    CompensatedRefusal,
    Verified,
    Quarantined,
}

#[cfg(test)]
pub(crate) struct BashStage2TestHook {
    pub(crate) scope: phoenix_core::work_scope::WorkScopeId,
    pub(crate) pgid: i32,
    pub(crate) reached: tokio::sync::oneshot::Sender<(bool, BashCustodySlot)>,
    pub(crate) resume: tokio::sync::oneshot::Receiver<()>,
    pub(crate) callback: tokio::sync::oneshot::Sender<(bool, bool, bool)>,
    pub(crate) inject_unknown_release: bool,
    pub(crate) completed: tokio::sync::oneshot::Sender<()>,
    pub(crate) stats: Arc<BashStage2TestStats>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct BashStage2TestStats {
    pub(crate) stop_attempts: std::sync::atomic::AtomicUsize,
    pub(crate) group_attempts: std::sync::atomic::AtomicUsize,
    pub(crate) group_signal_succeeded: std::sync::atomic::AtomicBool,
    pub(crate) cleanup_attempts: std::sync::atomic::AtomicUsize,
    pub(crate) precleanup_handback_refused: std::sync::atomic::AtomicBool,
    pub(crate) actual_confirmed_release: std::sync::atomic::AtomicBool,
}

pub(crate) struct BashCustodyState {
    pub(crate) owner: Option<NativeBashRetirement>,
    pub(crate) stage: BashStage,
    pub(crate) stop_failure: Option<String>,
    #[cfg(test)]
    pub(crate) test_hook: Option<BashStage2TestHook>,
}

impl BashCustodyState {
    pub(crate) fn preparing() -> Self {
        Self {
            owner: None,
            stage: BashStage::Preparing,
            stop_failure: None,
            #[cfg(test)]
            test_hook: None,
        }
    }
}

pub(crate) type BashCustodySlot = Arc<Mutex<BashCustodyState>>;

/// Captured once from the original entry. The second stage deliberately uses
/// the same command, inventory, durable source and irreversible live-source gate.
pub(super) struct BashEntryContext {
    pub(super) db: Database,
    pub(super) command: NativeCloseCommand,
    pub(super) receipt: NativeCloseReceipt,
    pub(super) live_source: ConvState,
    pub(super) source_entry: SourceEntryWorker,
    pub(super) fatal: Arc<FatalLocalAuthorityFence>,
}

/// Closing/dropping an observation receiver never owns this guard. If the
/// retained task itself is lost while authority is unresolved, fail-stop rather
/// than silently treating its missing result as ordinary resource failure.
struct BashAuthorityGuard {
    fatal: Arc<FatalLocalAuthorityFence>,
    armed: bool,
}

impl Drop for BashAuthorityGuard {
    fn drop(&mut self) {
        if self.armed {
            self.fatal.close("native_close_initial_bash_group_entry");
        }
    }
}

fn failure(expected: &CloseExpectedRetirementResource, detail: String) -> NativeInitialStepError {
    NativeInitialStepError::ProcessFailure {
        kind: expected.resource.kind(),
        resources: vec![expected.resource.clone()],
        detail,
    }
}

impl BashEntryContext {
    pub(super) async fn complete(
        self,
        slot: BashCustodySlot,
        expected: &CloseExpectedRetirementResource,
    ) -> Result<CloseProcessStepOutcome, NativeInitialStepError> {
        let mut authority = BashAuthorityGuard {
            fatal: Arc::clone(&self.fatal),
            armed: true,
        };
        // Catch in the retained task, not in the actor waiting on its observer.
        #[cfg(test)]
        let completion = {
            let mut state = slot.lock().await;
            state.test_hook.as_mut().map(|hook| {
                let (replacement, _) = tokio::sync::oneshot::channel();
                std::mem::replace(&mut hook.completed, replacement)
            })
        };
        let result = std::panic::AssertUnwindSafe(self.complete_inner(&slot, expected))
            .catch_unwind()
            .await;
        let result = match result {
            Ok(Err(NativeInitialStepError::FatalAuthority)) | Err(_) => {
                self.fatal.close("native_close_initial_bash_group_entry");
                slot.lock().await.stage = BashStage::Quarantined;
                Err(NativeInitialStepError::FatalAuthority)
            }
            Ok(result) => {
                authority.armed = false;
                result
            }
        };
        #[cfg(test)]
        if let Some(completion) = completion {
            let _ = completion.send(());
        }
        result
    }

    // The stages below are one custody-locked protocol; extracting phases would
    // obscure the lock/authority boundary that prevents an unsafe handoff.
    #[allow(clippy::too_many_lines)]
    async fn complete_inner(
        &self,
        slot: &BashCustodySlot,
        expected: &CloseExpectedRetirementResource,
    ) -> Result<CloseProcessStepOutcome, NativeInitialStepError> {
        // Acquired before the SQLite writer; no custody locks are acquired by
        // either callback. The owner is borrowed, never removed from its record.
        let mut state = slot.lock().await;
        if state.stage != BashStage::StopReleased {
            return Err(NativeInitialStepError::FatalAuthority);
        }
        let mut problem = state.stop_failure.take();
        let mut confirmed_group_refusal = false;
        #[cfg(test)]
        let mut test_hook = state.test_hook.take();
        if problem.is_none() {
            let owner = state
                .owner
                .as_mut()
                .ok_or(NativeInitialStepError::FatalAuthority)?;
            if let Err(error) = owner.observe_stopped().await {
                problem = Some(format!("Bash stop observation failed: {error}"));
            } else {
                state.stage = BashStage::StopObserved;
                #[cfg(test)]
                if let Some(hook) = test_hook.as_mut() {
                    let (replacement, _) = tokio::sync::oneshot::channel();
                    let reached = std::mem::replace(&mut hook.reached, replacement);
                    hook.stats.precleanup_handback_refused.store(
                        matches!(
                            state.owner.as_mut().unwrap().handback_compensated_refusal().await,
                            Err(phoenix_tools::bash::registry::NativeBashRetirementError::CleanupIncomplete)
                        ),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    let _ = reached.send((
                        self.source_entry
                            .is_eligible_unrevoked_before_deadline_for_test(),
                        Arc::clone(slot),
                    ));
                    let _ = (&mut hook.resume).await;
                }
                state.stage = BashStage::GroupWriterPending;
                // SIGSTOP admission is not reusable SIGKILL authority. A new
                // writer validates the exact original binding after observation
                // and the live gate runs at the actual nonwaiting group syscall.
                let result = self
                    .db
                    .with_native_close_initial_process_first_effect(
                        &self.command,
                        &self.receipt,
                        &self.live_source,
                        expected.scope.clone(),
                        expected.snapshot.clone(),
                        &expected.resource,
                        || {
                            #[cfg(test)]
                            let revoked = self.source_entry.is_revoked_for_test();
                            let admission =
                                self.source_entry.try_first_effect(&self.command, || {
                                    #[cfg(test)]
                                    if let Some(hook) = test_hook.as_ref() {
                                        hook.stats
                                            .group_attempts
                                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    state
                                        .owner
                                        .as_mut()
                                        .expect("record retains Bash owner")
                                        .try_signal_group()
                                });
                            #[cfg(test)]
                            if let Some(hook) = test_hook.as_mut() {
                                let (replacement, _) = tokio::sync::oneshot::channel();
                                let callback = std::mem::replace(&mut hook.callback, replacement);
                                hook.stats.group_signal_succeeded.store(
                                    matches!(&admission, WorkerEntryResult::Ran(Ok(()))),
                                    std::sync::atomic::Ordering::SeqCst,
                                );
                                let _ = callback.send((
                                    revoked,
                                    self.source_entry.original_deadline_unexpired_for_test(),
                                    matches!(&admission, WorkerEntryResult::Refused),
                                ));
                            }
                            admission
                        },
                    )
                    .await;
                // Explicit test fault injection at the real second return,
                // never a claimed naturally occurring SQLite uncertainty.
                #[cfg(test)]
                let result = match (result, test_hook.as_ref()) {
                    (Ok((admission, NativeCloseInitialEntryRelease::Confirmed)), Some(hook)) => {
                        hook.stats
                            .actual_confirmed_release
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        if hook.inject_unknown_release {
                            Ok((
                                admission,
                                NativeCloseInitialEntryRelease::Unconfirmed(
                                    phoenix_db::DbError::CloseFoundationPrecondition(
                                        "injected second Bash writer release uncertainty".into(),
                                    ),
                                ),
                            ))
                        } else {
                            Ok((admission, NativeCloseInitialEntryRelease::Confirmed))
                        }
                    }
                    (result, _) => result,
                };
                match result {
                    Ok((admission, NativeCloseInitialEntryRelease::Confirmed)) => {
                        problem = match admission {
                            WorkerEntryResult::Ran(Ok(())) => None,
                            WorkerEntryResult::Ran(Err(error)) => {
                                Some(format!("Bash group signal failed: {error}"))
                            }
                            WorkerEntryResult::Busy | WorkerEntryResult::Refused => {
                                confirmed_group_refusal = true;
                                Some("live source unavailable, expired or revoked before Bash group signal".into())
                            }
                        };
                    }
                    Err(
                        NativeCloseInitialFirstEffectError::NotStarted(error)
                        | NativeCloseInitialFirstEffectError::Rejected {
                            error,
                            release: NativeCloseInitialEntryRelease::Confirmed,
                        },
                    ) => {
                        confirmed_group_refusal = true;
                        problem = Some(format!("Bash group authorization refused: {error}"));
                    }
                    // Missing/unconfirmed writer release is NOT permission to
                    // resume, drop the state pin, verify, or retry either leaf.
                    _ => {
                        state.stage = BashStage::Quarantined;
                        return Err(NativeInitialStepError::FatalAuthority);
                    }
                }
            }
        }
        // Every writer used above is now definitely absent/released. Compensate
        // our own stop even after deadline/source revocation; never reauthorize
        // a destructive group effect as part of compensation.
        state.stage = BashStage::WritersReleased;
        let owner = state
            .owner
            .as_mut()
            .ok_or(NativeInitialStepError::FatalAuthority)?;
        #[cfg(test)]
        if let Some(hook) = test_hook.as_ref() {
            hook.stats
                .cleanup_attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        if let Err(error) = owner.cleanup_after_writer_release() {
            state.stage = BashStage::CleanupBlocked;
            return Err(failure(
                expected,
                format!("Bash cleanup retains original owner: {error}"),
            ));
        }
        if let Some(problem) = problem {
            if confirmed_group_refusal {
                // The second leaf never entered and every writer is definitely
                // released. Return only this compensated original handle, not
                // the scope permit, and do not reopen spawn or Close admission.
                if let Err(error) = owner.handback_compensated_refusal().await {
                    state.stage = BashStage::CleanupBlocked;
                    return Err(failure(
                        expected,
                        format!("{problem}; Bash handback retains original owner: {error}"),
                    ));
                }
                state.stage = BashStage::CompensatedRefusal;
                drop(state.owner.take());
            } else {
                state.stage = BashStage::CleanupBlocked;
            }
            return Err(failure(expected, problem));
        }
        let outcome = owner.verify_after_cleanup().await;
        match outcome {
            NativeBashRetirementOutcome::Retired(_) => {
                state.stage = BashStage::Verified;
                // Exact waiter + group absence are now proven and every writer
                // released. Release the scope-wide registry writer so another
                // originally captured group can retire under the same deadline.
                drop(state.owner.take());
                Ok(CloseProcessStepOutcome::Retired)
            }
            other => {
                state.stage = BashStage::CleanupBlocked;
                Err(failure(
                    expected,
                    format!("Bash retirement remains unverified: {other:?}"),
                ))
            }
        }
    }
}
