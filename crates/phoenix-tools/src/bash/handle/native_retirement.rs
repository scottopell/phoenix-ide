//! Physical primitives, not Close authority. Each signal needs its own caller fence.
//!
//! Retain this owner in process-epoch custody before requesting SIGSTOP. Cancellation
//! or unknown SQLite writer release must not drop that custody or start observation,
//! further signaling, or cleanup. Drop closes the fd only; it NEVER resumes a process.
//! No ordinary signaling wrapper is routed through this API.

#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{io, sync::Arc};
#[cfg(target_os = "linux")]
use tokio::sync::OwnedRwLockReadGuard;
use tokio::time::Instant;

use super::Handle;
#[cfg(target_os = "linux")]
use super::HandleState;

// Linux UAPI include/uapi/linux/pidfd.h, available since Linux 6.9. A retained
// struct pid identifies the original group even after its numeric ID is reused.
// Unsupported kernels must refuse; never fall back to kill(-pgid, ...).
#[cfg(target_os = "linux")]
const PIDFD_SIGNAL_PROCESS_GROUP: libc::c_uint = 1 << 2;

#[derive(Debug)]
pub enum NativeBashSignalError {
    Unsupported,
    Expired,
    WrongPhase,
    NotStopped,
    Io(io::Error),
}

impl std::fmt::Display for NativeBashSignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("exact native Bash signaling is unsupported"),
            Self::Expired => f.write_str("original Bash retirement deadline expired"),
            Self::WrongPhase => {
                f.write_str("Bash physical stage already attempted or not admitted")
            }
            Self::NotStopped => f.write_str("exact Bash leader did not report SIGSTOP"),
            Self::Io(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for NativeBashSignalError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    StopAttempted,
    StopRequested,
    StopObserved,
    GroupAttempted,
    Resumed,
}

/// Nonclone physical owner. This does not own the registry's generation fence;
/// callers must retain that original fence alongside this owner through settlement.
/// Group signaling uses the retained pidfd's original kernel group identity, not
/// a numeric-PGID lookup. The state guard remains a state pin, not a reap lock;
/// process reaping, readers, and writer-release custody still belong to the caller.
#[must_use = "retain in external custody through confirmed release and explicit cleanup"]
pub struct NativeBashSignal {
    #[cfg(target_os = "linux")]
    state_pin: Option<OwnedRwLockReadGuard<Arc<HandleState>>>,
    #[cfg(target_os = "linux")]
    pidfd: OwnedFd,
    #[cfg(target_os = "linux")]
    wait_id: libc::id_t,
    pid: u32,
    pgid: i32,
    deadline: Instant,
    phase: Phase,
    resume_required: bool,
}

impl Handle {
    /// Inert preparation: await the state lock under the original deadline, open
    /// the exact pidfd and probe birth identity before any caller writer is held.
    /// `None` means no matching live leader, NOT verified process-group absence.
    pub async fn prepare_native_leaf(
        self: &Arc<Self>,
        deadline: Instant,
    ) -> Result<Option<NativeBashSignal>, NativeBashSignalError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (self, deadline);
            Err(NativeBashSignalError::Unsupported)
        }
        #[cfg(target_os = "linux")]
        {
            if Instant::now() >= deadline {
                return Err(NativeBashSignalError::Expired);
            }
            let guard = tokio::time::timeout_at(deadline, Arc::clone(&self.state).read_owned())
                .await
                .map_err(|_| NativeBashSignalError::Expired)?;
            if Instant::now() >= deadline {
                return Err(NativeBashSignalError::Expired);
            }
            let HandleState::Live(live) = guard.as_ref() else {
                return Ok(None);
            };
            let pid = live.pid;
            let pgid = live.pgid;
            let numeric_pid = i32::try_from(pid).map_err(|_| {
                NativeBashSignalError::Io(io::Error::from_raw_os_error(libc::EINVAL))
            })?;
            // PIDFD_SIGNAL_PROCESS_GROUP addresses the group whose leader has
            // this pidfd's identity. A nonleader cannot authorize another group.
            if pgid <= 0 || pgid != numeric_pid {
                return Err(NativeBashSignalError::Unsupported);
            }
            let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, numeric_pid, 0) };
            if raw < 0 {
                return Err(NativeBashSignalError::Io(io::Error::last_os_error()));
            }
            let raw = libc::c_int::try_from(raw).map_err(|_| {
                NativeBashSignalError::Io(io::Error::from_raw_os_error(libc::EINVAL))
            })?;
            // pidfd_open returns a newly owned descriptor. OwnedFd's Drop only closes it.
            let pidfd = unsafe { OwnedFd::from_raw_fd(raw) };
            let wait_id = libc::id_t::try_from(raw).map_err(|_| {
                NativeBashSignalError::Io(io::Error::from_raw_os_error(libc::EINVAL))
            })?;
            if phoenix_core::process_identity::current_process_identity(pid)
                != Some(self.launch_identity.process)
            {
                return Ok(None);
            }
            // Signal zero checks support/permissions and original group presence
            // without delivering a signal. Reject old kernels before any SIGSTOP.
            let supported = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    0,
                    std::ptr::null::<libc::siginfo_t>(),
                    PIDFD_SIGNAL_PROCESS_GROUP,
                )
            };
            if supported != 0 {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EINVAL) | Some(libc::ENOSYS)
                ) {
                    return Err(NativeBashSignalError::Unsupported);
                }
                return Err(NativeBashSignalError::Io(error));
            }
            if Instant::now() >= deadline {
                return Err(NativeBashSignalError::Expired);
            }
            Ok(Some(NativeBashSignal {
                state_pin: Some(guard),
                pidfd,
                wait_id,
                pid,
                pgid,
                deadline,
                phase: Phase::Prepared,
                resume_required: false,
            }))
        }
    }
}

// The same attempt-once/deadline boundary is used by BOTH physical leaves.
// An OS error consumes this attempt, but expiry before entry performs no effect.
fn attempt_once<T>(
    phase: &mut Phase,
    expected: Phase,
    attempted: Phase,
    deadline: Instant,
    effect: impl FnOnce() -> io::Result<T>,
) -> Result<T, NativeBashSignalError> {
    if *phase != expected {
        return Err(NativeBashSignalError::WrongPhase);
    }
    if Instant::now() >= deadline {
        return Err(NativeBashSignalError::Expired);
    }
    *phase = attempted;
    effect().map_err(NativeBashSignalError::Io)
}

impl NativeBashSignal {
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn pgid(&self) -> i32 {
        self.pgid
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Synchronous first physical effect. Invoke INSIDE the exact DB/live-source
    /// callback, after retaining this owner externally. No waits, probes or cleanup.
    pub fn try_request_stop(&mut self) -> Result<(), NativeBashSignalError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(NativeBashSignalError::Unsupported)
        }
        #[cfg(target_os = "linux")]
        {
            let fd = self.pidfd.as_raw_fd();
            attempt_once(
                &mut self.phase,
                Phase::Prepared,
                Phase::StopAttempted,
                self.deadline,
                || {
                    let rc = unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd,
                            libc::SIGSTOP,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        )
                    };
                    if rc == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                },
            )?;
            self.resume_required = true;
            self.phase = Phase::StopRequested;
            Ok(())
        }
    }

    /// Only after confirmed writer release. Borrowing preserves the externally
    /// retained owner if this observation future is cancelled. No signal is sent.
    pub async fn observe_stopped(&mut self) -> Result<(), NativeBashSignalError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(NativeBashSignalError::Unsupported)
        }
        #[cfg(target_os = "linux")]
        {
            if self.phase != Phase::StopRequested {
                return Err(NativeBashSignalError::WrongPhase);
            }
            loop {
                if Instant::now() >= self.deadline {
                    return Err(NativeBashSignalError::Expired);
                }
                if self.exact_stop_observed()? {
                    if Instant::now() >= self.deadline {
                        return Err(NativeBashSignalError::Expired);
                    }
                    self.phase = Phase::StopObserved;
                    return Ok(());
                }
                tokio::time::sleep_until(std::cmp::min(
                    self.deadline,
                    Instant::now() + std::time::Duration::from_millis(10),
                ))
                .await;
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn exact_stop_observed(&self) -> Result<bool, NativeBashSignalError> {
        let mut state = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        let result = unsafe {
            libc::waitid(
                libc::P_PIDFD,
                self.wait_id,
                state.as_mut_ptr(),
                libc::WSTOPPED | libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        };
        if result != 0 {
            return Err(NativeBashSignalError::Io(io::Error::last_os_error()));
        }
        let state = unsafe { state.assume_init() };
        if unsafe { state.si_pid() } == 0 {
            return Ok(false);
        }
        if state.si_code == libc::CLD_STOPPED && unsafe { state.si_status() } == libc::SIGSTOP {
            Ok(true)
        } else {
            Err(NativeBashSignalError::NotStopped)
        }
    }

    /// Synchronous SECOND destructive leaf, requiring a fresh exact caller
    /// DB/live-source callback. First SIGSTOP does not authorize this SIGKILL.
    /// Reobserves the exact leader with one nonblocking waitid AFTER any caller
    /// authorization wait, then checks the original deadline at actual signal entry.
    /// No wait loop or asynchronous work is done here. Failed signals are not retried.
    pub fn try_signal_group(&mut self) -> Result<(), NativeBashSignalError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(NativeBashSignalError::Unsupported)
        }
        #[cfg(target_os = "linux")]
        {
            if self.phase != Phase::StopObserved {
                return Err(NativeBashSignalError::WrongPhase);
            }
            if Instant::now() >= self.deadline {
                return Err(NativeBashSignalError::Expired);
            }
            if !self.exact_stop_observed()? {
                return Err(NativeBashSignalError::NotStopped);
            }
            let fd = self.pidfd.as_raw_fd();
            attempt_once(
                &mut self.phase,
                Phase::StopObserved,
                Phase::GroupAttempted,
                self.deadline,
                || {
                    let rc = unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd,
                            libc::SIGKILL,
                            std::ptr::null::<libc::siginfo_t>(),
                            PIDFD_SIGNAL_PROCESS_GROUP,
                        )
                    };
                    if rc == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                },
            )
        }
    }

    /// Release the state pin AFTER confirmed writer release and explicit stop
    /// cleanup, before any handle-state reads, exit notification or tombstone
    /// verification. Those operations may wait for a state writer blocked by this
    /// pin. The registry generation fence and fd custody remain the caller's.
    /// This permanently prevents new destructive entry through this owner.
    pub fn release_state_pin_after_writer_release(&mut self) -> Result<(), NativeBashSignalError> {
        if self.resume_required {
            return Err(NativeBashSignalError::WrongPhase);
        }
        self.phase = Phase::Resumed;
        #[cfg(target_os = "linux")]
        drop(self.state_pin.take());
        Ok(())
    }

    /// Explicit exact-leader cleanup only after confirmed release of every writer
    /// involved. May resume after deadline to undo this owner's earlier stop, but
    /// never authorizes another group signal. Errors retain the same owner in place.
    pub fn resume_after_writer_release(&mut self) -> Result<(), NativeBashSignalError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(NativeBashSignalError::Unsupported)
        }
        #[cfg(target_os = "linux")]
        {
            if !self.resume_required {
                return Ok(());
            }
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.pidfd.as_raw_fd(),
                    libc::SIGCONT,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            };
            if rc != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(NativeBashSignalError::Io(error));
                }
            }
            self.resume_required = false;
            self.phase = Phase::Resumed;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nonleader_preparation_is_unsupported() {
        let handle = Handle::new_live(
            phoenix_core::work_scope::ResourceScopeKey::Unattached("native-nonleader".into()),
            super::super::HandleId::new("native-nonleader"),
            "synthetic nonleader; never executed".into(),
            None,
            std::path::PathBuf::from("/tmp"),
            2,
            1,
            1024,
        );
        assert!(matches!(
            handle
                .prepare_native_leaf(Instant::now() + std::time::Duration::from_secs(1))
                .await,
            Err(NativeBashSignalError::Unsupported)
        ));
        // This is an admission result, not syscall interception: the Linux path
        // rejects the mismatched leader/group before pidfd_open or signal zero.
        assert!(handle.kill_attempt().await.is_none());
    }

    #[cfg(target_os = "linux")]
    fn child_handle(child: &tokio::process::Child) -> Arc<Handle> {
        use phoenix_core::{
            process_identity::current_process_identity,
            work_scope::{ResourceAuthority, ResourceScopeKey},
        };
        let pid = child.id().expect("spawned child has pid");
        let process =
            current_process_identity(pid).expect("real child has a valid process identity");
        let pgid = i32::try_from(pid).expect("child pid fits pgid");
        Handle::new_live_for_actor_with_owner_and_launch_identity(
            ResourceScopeKey::Unattached("native-retirement-test".into()),
            super::super::HandleId::new(format!("native-retirement-{pid}")),
            super::super::BashLaunchIdentity {
                process,
                launch_uuid: format!("native-retirement-test-{pid}"),
            },
            "native-retirement-test".into(),
            ResourceAuthority::Work,
            "sleep 600".into(),
            None,
            std::path::PathBuf::from("/tmp"),
            pgid,
            pid,
            1024,
        )
    }

    #[cfg(target_os = "linux")]
    async fn stopped_sleep_child() -> tokio::process::Child {
        tokio::process::Command::new("sleep")
            .arg("600")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .expect("spawn disposable child")
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropping_native_owner_does_not_resume_observed_child() {
        use std::time::Duration;

        let mut child = stopped_sleep_child().await;
        let pid = child.id().unwrap();
        let handle = child_handle(&child);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut owner = handle.prepare_native_leaf(deadline).await.unwrap().unwrap();
        owner.try_request_stop().unwrap();
        tokio::time::timeout(Duration::from_secs(5), owner.observe_stopped())
            .await
            .expect("stop observation remains live")
            .unwrap();

        drop(owner);
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let state = status
            .lines()
            .find(|line| line.starts_with("State:"))
            .unwrap();
        assert!(
            state.contains("T (stopped)") || state.contains("t (tracing stop)"),
            "dropping native owner resumed child: {state}"
        );

        // This test owns the exact Child handle; explicitly recover then kill/reap it.
        assert_eq!(
            unsafe { libc::kill(i32::try_from(pid).unwrap(), libc::SIGCONT) },
            0
        );
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn group_kill_requires_explicit_resume_before_state_pin_release() {
        use super::super::{FinalCause, TOMBSTONE_TAIL_LINES};
        use std::time::{Duration, SystemTime};

        let mut child = stopped_sleep_child().await;
        let handle = child_handle(&child);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut owner = handle.prepare_native_leaf(deadline).await.unwrap().unwrap();
        owner.try_request_stop().unwrap();
        tokio::time::timeout(Duration::from_secs(5), owner.observe_stopped())
            .await
            .expect("stop observation remains live")
            .unwrap();
        owner.try_signal_group().unwrap();

        assert!(matches!(
            owner.release_state_pin_after_writer_release(),
            Err(NativeBashSignalError::WrongPhase)
        ));
        let transition = handle.transition_to_terminal(
            FinalCause::Killed {
                exit_code: None,
                signal_number: Some(libc::SIGKILL),
            },
            Duration::ZERO,
            SystemTime::now(),
            TOMBSTONE_TAIL_LINES,
        );
        let mut transition = std::pin::pin!(transition);
        let first = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(transition.as_mut(), cx))
        })
        .await;
        assert!(
            first.is_pending(),
            "terminal transition must wait on the retained state pin"
        );

        // Group SIGKILL can make exact-leader SIGCONT return ESRCH; that is accepted.
        owner.resume_after_writer_release().unwrap();
        owner.release_state_pin_after_writer_release().unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), transition)
            .await
            .expect("terminal transition unblocked by state-pin release"));
        // Direct-child wait is the test's reap boundary; no runtime/source authority is implied.
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("group-killed child reaps")
            .unwrap();
    }

    #[test]
    fn expired_stop_and_group_stages_perform_zero_effects() {
        for (expected, attempted) in [
            (Phase::Prepared, Phase::StopAttempted),
            (Phase::StopObserved, Phase::GroupAttempted),
        ] {
            let mut phase = expected;
            let mut calls = 0;
            let result = attempt_once(&mut phase, expected, attempted, Instant::now(), || {
                calls += 1;
                Ok(())
            });
            assert!(matches!(result, Err(NativeBashSignalError::Expired)));
            assert_eq!(calls, 0);
            assert!(phase == expected);
        }
    }

    #[test]
    fn each_physical_stage_is_attempted_only_once_even_after_os_error() {
        for (expected, attempted) in [
            (Phase::Prepared, Phase::StopAttempted),
            (Phase::StopObserved, Phase::GroupAttempted),
        ] {
            let mut phase = expected;
            let mut calls = 0;
            let deadline = Instant::now() + std::time::Duration::from_secs(30);
            let first = attempt_once(&mut phase, expected, attempted, deadline, || {
                calls += 1;
                Err::<(), _>(io::Error::from_raw_os_error(libc::EACCES))
            });
            assert!(
                matches!(first, Err(NativeBashSignalError::Io(error)) if error.raw_os_error() == Some(libc::EACCES))
            );
            let second = attempt_once(&mut phase, expected, attempted, deadline, || {
                calls += 1;
                Ok(())
            });
            assert!(matches!(second, Err(NativeBashSignalError::WrongPhase)));
            assert_eq!(calls, 1);
        }
    }
}
