//! Original registry-generation custody for separately admitted native Bash leaves.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::OwnedRwLockWriteGuard;
use tokio::time::Instant;

use super::{
    record_handle_in_report, retirement_target_has_live_member, BashRetirementTarget,
    BashTeardownGeneration, CascadeBashReport, ResourceScopeKeyHandles,
};
use crate::bash::handle::{Handle, NativeBashSignal, NativeBashSignalError, WaiterSettlement};

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
    _registry_writer: OwnedRwLockWriteGuard<ResourceScopeKeyHandles>,
    _handle: Arc<Handle>,
    generation: BashTeardownGeneration,
    target: BashRetirementTarget,
    deadline: Instant,
    signal: NativeBashSignal,
    report: CascadeBashReport,
    cleanup_complete: bool,
    group_signal_succeeded: bool,
}

impl super::PreparedBashRetirement {
    /// Convert inert registry custody to native leaf custody under the original deadline.
    /// No signal is sent; the returned owner keeps the original registry writer held.
    pub async fn prepare_native(self) -> Result<NativeBashRetirement, NativeBashRetirementError> {
        let Self {
            entry,
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
                other => NativeBashRetirementError::Native(other),
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
            _registry_writer: writer,
            _handle: handle,
            generation,
            target,
            deadline,
            signal,
            report,
            cleanup_complete: false,
            group_signal_succeeded: false,
        })
    }
}

impl NativeBashRetirement {
    pub fn try_request_stop(&mut self) -> Result<(), NativeBashSignalError> {
        self.signal.try_request_stop()
    }

    pub async fn observe_stopped(&mut self) -> Result<(), NativeBashSignalError> {
        self.signal.observe_stopped().await
    }

    pub fn try_signal_group(&mut self) -> Result<(), NativeBashSignalError> {
        let result = self.signal.try_signal_group();
        if result.is_ok() {
            self.group_signal_succeeded = true;
        }
        result
    }

    /// Invoke only after confirmed release of every external writer. Resumption
    /// must succeed before releasing the handle-state pin.
    pub fn cleanup_after_writer_release(&mut self) -> Result<(), NativeBashSignalError> {
        self.signal.resume_after_writer_release()?;
        self.signal.release_state_pin_after_writer_release()?;
        self.cleanup_complete = true;
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
        if !self._registry_writer.teardown_started
            || self._registry_writer.teardown_generation != self.generation.0
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
            await_waiter_settlement(self._handle.subscribe_waiter_settlement(), self.deadline).await
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
