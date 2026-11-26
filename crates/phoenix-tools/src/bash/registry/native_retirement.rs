//! Original registry-generation custody for separately admitted native Bash leaves.

use std::collections::HashMap;
use std::sync::Arc;

use phoenix_core::work_scope::ResourceScopeKey;
use thiserror::Error;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use tokio::time::Instant;

use super::{
    record_handle_in_report, retirement_target_has_live_member, BashRetirementTarget,
    BashTeardownGeneration, CascadeBashReport, RegisteredHandle, ResourceScopeKeyHandles,
};
use crate::bash::handle::{
    Handle, HandleId, NativeBashSignal, NativeBashSignalError, WaiterSettlement,
};

#[derive(Debug, Error)]
pub enum NativeBashRetirementError {
    #[error("original Bash retirement deadline expired during native preparation")]
    DeadlineExpired,
    #[error("original Bash teardown generation is stale")]
    StaleGeneration,
    #[error("exact native Bash retirement is unsupported on this host")]
    Unsupported,
    #[error("original Bash target has no matching live native leader")]
    NoLiveLeader,
    #[error("native Bash leader PID/PGID does not match the original retirement target")]
    TargetMismatch,
    #[error("native Bash leaf preparation failed: {0}")]
    Native(#[source] NativeBashSignalError),
    #[error("compensated refusal handback requires successful native cleanup")]
    CleanupIncomplete,
    #[error("compensated refusal handback is forbidden after any process-group signal attempt")]
    GroupSignalAttempted,
    #[error("original Bash teardown generation is stale")]
    StaleHandbackGeneration,
    #[error("the exact original handle is not uniquely held by this native owner")]
    OriginalMembershipMismatch,
    #[error("the original handle id is already occupied in the work table or global index")]
    HandbackCollision,
}

#[derive(Debug)]
pub enum NativeBashRetirementOutcome {
    Retired(CascadeBashReport),
    Unverified(CascadeBashReport),
    StaleGeneration(CascadeBashReport),
}

/// Affine owner retaining the registry writer across both native signal stages
/// and verification. It deliberately has no Drop cleanup or signal behavior.
#[must_use = "retain through confirmed external writer release and explicit cleanup"]
pub struct NativeBashRetirement {
    registry_writer: OwnedRwLockWriteGuard<ResourceScopeKeyHandles>,
    global_index: Arc<RwLock<HashMap<HandleId, RegisteredHandle>>>,
    work_scope: ResourceScopeKey,
    handle: Arc<Handle>,
    generation: BashTeardownGeneration,
    target: BashRetirementTarget,
    deadline: Instant,
    signal: NativeBashSignal,
    report: CascadeBashReport,
    cleanup_complete: bool,
    group_signal_attempted: bool,
    group_signal_succeeded: bool,
}

impl super::PreparedBashRetirement {
    /// Convert inert registry custody to native leaf custody under the original deadline.
    /// No signal is sent; the returned owner keeps the original registry writer held.
    ///
    /// # Errors
    /// Returns an error when the original deadline or generation is invalid, the exact live
    /// leader cannot be prepared, or native signaling is unsupported.
    pub async fn prepare_native(self) -> Result<NativeBashRetirement, NativeBashRetirementError> {
        let Self {
            entry,
            global_index,
            work_scope,
            handle,
            generation,
            target,
            deadline,
        } = self;
        if Instant::now() >= deadline {
            return Err(NativeBashRetirementError::DeadlineExpired);
        }
        let writer = tokio::time::timeout_at(deadline, Arc::clone(&entry).write_owned())
            .await
            .map_err(|_| NativeBashRetirementError::DeadlineExpired)?;
        if Instant::now() >= deadline {
            return Err(NativeBashRetirementError::DeadlineExpired);
        }
        if !writer.teardown_started || writer.teardown_generation != generation.0 {
            return Err(NativeBashRetirementError::StaleGeneration);
        }
        let uncertainty = writer
            .admission
            .unverified_launch
            .load(std::sync::atomic::Ordering::Acquire)
            .then_some(super::BashScopeUncertainty::UnregisteredLaunch);
        let signal = tokio::time::timeout_at(deadline, handle.prepare_native_leaf(deadline))
            .await
            .map_err(|_| NativeBashRetirementError::DeadlineExpired)?
            .map_err(|error| match error {
                NativeBashSignalError::Unsupported => NativeBashRetirementError::Unsupported,
                NativeBashSignalError::Expired => NativeBashRetirementError::DeadlineExpired,
                error @ (NativeBashSignalError::WrongPhase
                | NativeBashSignalError::NotStopped
                | NativeBashSignalError::Io(_)) => NativeBashRetirementError::Native(error),
            })?
            .ok_or(NativeBashRetirementError::NoLiveLeader)?;
        if Instant::now() >= deadline {
            return Err(NativeBashRetirementError::DeadlineExpired);
        }
        if signal.pid() != target.pid.unwrap_or_default() || signal.pgid() != target.pgid {
            return Err(NativeBashRetirementError::TargetMismatch);
        }
        let mut report = CascadeBashReport {
            teardown_generation: Some(generation),
            scope_uncertainty: uncertainty,
            ..CascadeBashReport::default()
        };
        record_handle_in_report(
            &mut report,
            target.pgid,
            target.pid,
            target.kill_pending_kernel,
        );
        Ok(NativeBashRetirement {
            registry_writer: writer,
            global_index,
            work_scope,
            handle,
            generation,
            target,
            deadline,
            signal,
            report,
            cleanup_complete: false,
            group_signal_attempted: false,
            group_signal_succeeded: false,
        })
    }
}

impl NativeBashRetirement {
    /// Request the first native stop signal for the exact retained leader.
    ///
    /// # Errors
    /// Returns the native signal error if the original deadline or signal phase refuses the
    /// stop request, or if the kernel call fails.
    pub fn try_request_stop(&mut self) -> Result<(), NativeBashSignalError> {
        self.signal.try_request_stop()
    }

    /// Observe the exact retained leader's stop state without sending a signal.
    ///
    /// # Errors
    /// Returns the native signal error if exact stop observation fails, the original deadline
    /// expires, or this owner is in the wrong phase.
    pub async fn observe_stopped(&mut self) -> Result<(), NativeBashSignalError> {
        self.signal.observe_stopped().await
    }

    /// Send the native group signal after exact stop observation.
    ///
    /// # Errors
    /// Returns the native signal error if the original deadline, exact stop observation, or
    /// phase refuses the group signal, or if the kernel call fails.
    pub fn try_signal_group(&mut self) -> Result<(), NativeBashSignalError> {
        self.group_signal_attempted = true;
        let result = self.signal.try_signal_group();
        if result.is_ok() {
            self.group_signal_succeeded = true;
        }
        result
    }

    /// Invoke only after confirmed release of every external writer. Resumption
    /// must succeed before releasing the handle-state pin.
    ///
    /// # Errors
    /// Returns the native signal error if resume or state-pin release fails. On failure this
    /// owner retains its cleanup state and registry custody.
    pub fn cleanup_after_writer_release(&mut self) -> Result<(), NativeBashSignalError> {
        self.signal.resume_after_writer_release()?;
        self.signal.release_state_pin_after_writer_release()?;
        self.cleanup_complete = true;
        Ok(())
    }

    /// Return the exact original live handle to its original table only when refusal is
    /// confirmed before any group-signal attempt and explicit native cleanup succeeded.
    /// The borrowed owner remains intact on every error; this operation never cancels or
    /// refreshes the original teardown fence and does not signal or renew its deadline.
    ///
    /// # Errors
    /// Returns an error unless cleanup is complete, no group signal was attempted, the original
    /// generation and membership remain valid, and the global index has no collision.
    pub async fn handback_compensated_refusal(&mut self) -> Result<(), NativeBashRetirementError> {
        if !self.cleanup_complete {
            return Err(NativeBashRetirementError::CleanupIncomplete);
        }
        if self.group_signal_attempted {
            return Err(NativeBashRetirementError::GroupSignalAttempted);
        }
        if !self.registry_writer.teardown_started
            || self.registry_writer.teardown_generation != self.generation.0
        {
            return Err(NativeBashRetirementError::StaleHandbackGeneration);
        }
        let handle = Arc::clone(&self.handle);
        if handle.handle_id != self.target.handle_id
            || handle.launch_identity != self.target.launch_identity
            || self.registry_writer.handles.contains_key(&handle.handle_id)
        {
            return Err(NativeBashRetirementError::OriginalMembershipMismatch);
        }

        // This is intentionally the only awaited lock acquisition after validation. Do not
        // consult the registry's inner scope map here: teardown may hold its writer while
        // waiting on this table, which would invert lock order.
        let mut global_index = self.global_index.write().await;
        if global_index.contains_key(&handle.handle_id) {
            return Err(NativeBashRetirementError::HandbackCollision);
        }
        self.registry_writer.insert(Arc::clone(&handle));
        global_index.insert(
            handle.handle_id.clone(),
            RegisteredHandle {
                owner: self.work_scope.clone(),
                handle,
            },
        );
        Ok(())
    }

    /// Verify while borrowing the owner, retaining its generation guard. Does not
    /// signal and refuses until explicit cleanup has completed.
    pub async fn verify_after_cleanup(&mut self) -> NativeBashRetirementOutcome {
        if !self.cleanup_complete {
            self.report.kill_failures.push((
                self.target.pgid,
                "native retirement verification refused before explicit writer-release cleanup"
                    .into(),
            ));
            return NativeBashRetirementOutcome::Unverified(self.report.clone());
        }
        if !self.registry_writer.teardown_started
            || self.registry_writer.teardown_generation != self.generation.0
        {
            self.report.kill_failures.push((
                self.target.pgid,
                "original Bash teardown generation is stale".into(),
            ));
            return NativeBashRetirementOutcome::StaleGeneration(self.report.clone());
        }
        if Instant::now() >= self.deadline {
            self.report.kill_failures.push((
                self.target.pgid,
                "original Bash retirement deadline expired; absence unverified".into(),
            ));
            return NativeBashRetirementOutcome::Unverified(self.report.clone());
        }
        if let Err(reason) =
            await_waiter_settlement(self.handle.subscribe_waiter_settlement(), self.deadline).await
        {
            self.report.kill_failures.push((self.target.pgid, reason));
            return NativeBashRetirementOutcome::Unverified(self.report.clone());
        }
        let observed = tokio::time::timeout_at(self.deadline, async {
            retirement_target_has_live_member(&self.target)
        })
        .await;
        if Instant::now() >= self.deadline {
            self.report.kill_failures.push((
                self.target.pgid,
                "original Bash retirement deadline expired; absence unverified".into(),
            ));
            return NativeBashRetirementOutcome::Unverified(self.report.clone());
        }
        match observed {
            Ok(Ok(false)) if self.group_signal_succeeded && !self.report.has_failures() => {
                NativeBashRetirementOutcome::Retired(self.report.clone())
            }
            Ok(Ok(false)) => {
                if !self.group_signal_succeeded {
                    self.report.kill_failures.push((self.target.pgid, "process group absence observed, but no native group signal succeeded; retirement is unverified".into()));
                }
                // Observed absence cannot erase prior failures or scope uncertainty.
                NativeBashRetirementOutcome::Unverified(self.report.clone())
            }
            Ok(Ok(true)) => {
                self.report.kill_failures.push((
                    self.target.pgid,
                    "exact process group remains live after native retirement".into(),
                ));
                NativeBashRetirementOutcome::Unverified(self.report.clone())
            }
            Ok(Err(error)) => {
                self.report.kill_failures.push((
                    self.target.pgid,
                    format!("native process-group absence observation failed: {error}"),
                ));
                NativeBashRetirementOutcome::Unverified(self.report.clone())
            }
            Err(_) => {
                self.report.kill_failures.push((
                    self.target.pgid,
                    "original Bash retirement deadline expired; absence unverified".into(),
                ));
                NativeBashRetirementOutcome::Unverified(self.report.clone())
            }
        }
    }
}

/// The exact waiter's proof is independent of a tombstone or group absence.
/// Observation cancellation drops only this subscription, never the actual waiter.
async fn await_waiter_settlement(
    mut observer: tokio::sync::watch::Receiver<WaiterSettlement>,
    deadline: Instant,
) -> Result<(), String> {
    if Instant::now() >= deadline {
        return Err("original Bash retirement deadline expired before waiter settlement".into());
    }
    let result = tokio::time::timeout_at(deadline, async {
        loop {
            let state = observer.borrow_and_update().clone();
            match state {
                WaiterSettlement::Settled => return Ok(()),
                WaiterSettlement::Unverified(reason) => {
                    return Err(format!("exact Bash waiter settlement unverified: {reason}"));
                }
                WaiterSettlement::Pending => {}
            }
            observer
                .changed()
                .await
                .map_err(|_| "exact Bash waiter settlement observer closed".to_string())?;
        }
    })
    .await;
    if Instant::now() >= deadline {
        return Err("original Bash retirement deadline expired during waiter settlement".into());
    }
    result.map_err(|_| {
        "original Bash retirement deadline expired during waiter settlement".to_string()
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn waiter_settlement_uses_original_remaining_deadline() {
        let (_sender, observer) = tokio::sync::watch::channel(WaiterSettlement::Pending);
        let deadline = Instant::now() + Duration::from_secs(10);
        tokio::time::advance(Duration::from_secs(9)).await;
        let wait = await_waiter_settlement(observer, deadline);
        let mut wait = std::pin::pin!(wait);
        let first = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(wait.as_mut(), cx))
        })
        .await;
        assert!(first.is_pending());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(wait.await.unwrap_err().contains("deadline expired"));
        assert_eq!(Instant::now(), deadline);
    }

    #[tokio::test(start_paused = true)]
    async fn waiter_settlement_failure_or_late_success_cannot_verify() {
        let (sender, observer) = tokio::sync::watch::channel(WaiterSettlement::Pending);
        sender.send_replace(WaiterSettlement::Unverified(
            "reader drain timed out".into(),
        ));
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(await_waiter_settlement(observer.clone(), deadline)
            .await
            .unwrap_err()
            .contains("reader drain timed out"));
        sender.send_replace(WaiterSettlement::Settled);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(await_waiter_settlement(observer, deadline)
            .await
            .unwrap_err()
            .contains("deadline expired"));
    }

    #[tokio::test]
    async fn waiter_settlement_accepts_retained_success_but_not_closed_pending() {
        let (sender, observer) = tokio::sync::watch::channel(WaiterSettlement::Pending);
        let deadline = Instant::now() + Duration::from_secs(10);
        sender.send_replace(WaiterSettlement::Settled);
        assert!(await_waiter_settlement(observer, deadline).await.is_ok());
        let (sender, observer) = tokio::sync::watch::channel(WaiterSettlement::Pending);
        drop(sender);
        assert!(await_waiter_settlement(observer, deadline)
            .await
            .unwrap_err()
            .contains("observer closed"));
    }
}
