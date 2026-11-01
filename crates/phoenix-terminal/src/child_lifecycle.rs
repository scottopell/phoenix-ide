//! Single child-exit observer and retained, in-memory wait receipt.

use nix::{sys::signal::Signal, sys::wait::WaitStatus, unistd::Pid};
use std::sync::Mutex;
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalExit {
    Exited(i32),
    Signaled(Signal, bool),
}

type ExitResult = nix::Result<TerminalExit>;

#[derive(Clone, Debug)]
pub(crate) struct ChildExitReceipt(watch::Receiver<Option<ExitResult>>);

impl ChildExitReceipt {
    pub(crate) fn exited(&self) -> bool {
        matches!(*self.0.borrow(), Some(Ok(_)))
    }

    pub(crate) async fn wait(&self) -> ExitResult {
        let mut receiver = self.0.clone();
        loop {
            if let Some(result) = *receiver.borrow_and_update() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Err(nix::errno::Errno::ECHILD);
            }
        }
    }
}

#[derive(Debug)]
struct ChildExitPublisher(watch::Sender<Option<ExitResult>>);

impl ChildExitPublisher {
    fn publish(self, result: ExitResult) {
        self.0.send_replace(Some(result));
    }
}

/// The observer is consumed on successful registry publication, never by a relay.
#[derive(Debug)]
pub struct TerminalChildLifecycle {
    observer: Mutex<Option<ChildExitObserver>>,
    receipt: ChildExitReceipt,
}

#[derive(Debug)]
pub(crate) struct ChildExitObserver {
    source: ChildExitSource,
    publisher: ChildExitPublisher,
}

#[derive(Debug)]
enum ChildExitSource {
    Process,
    #[cfg(any(test, feature = "test-support"))]
    Passive,
    #[cfg(test)]
    Controlled(tokio::sync::oneshot::Receiver<ExitResult>),
}

impl TerminalChildLifecycle {
    #[must_use]
    pub fn new() -> Self {
        Self::with_observer(ChildExitSource::Process)
    }

    fn with_observer(source: ChildExitSource) -> Self {
        let (sender, receiver) = watch::channel(None);
        Self {
            observer: Mutex::new(Some(ChildExitObserver {
                source,
                publisher: ChildExitPublisher(sender),
            })),
            receipt: ChildExitReceipt(receiver),
        }
    }

    /// Fake-PID registry fixtures never call waitpid or start a runtime task.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn passive() -> Self {
        Self::with_observer(ChildExitSource::Passive)
    }

    #[cfg(test)]
    pub(crate) fn controlled() -> (Self, tokio::sync::oneshot::Sender<ExitResult>) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        (
            Self::with_observer(ChildExitSource::Controlled(receiver)),
            sender,
        )
    }

    #[cfg(test)]
    pub(crate) fn publish_for_test(&self, result: ExitResult) {
        let observer = self
            .observer
            .lock()
            .expect("child observer poisoned")
            .take()
            .expect("test publisher already consumed");
        assert!(matches!(observer.source, ChildExitSource::Passive));
        observer.publisher.publish(result);
    }

    pub(crate) fn receipt(&self) -> ChildExitReceipt {
        self.receipt.clone()
    }

    pub(crate) fn take_observer(&self) -> Option<ChildExitObserver> {
        let mut observer = self.observer.lock().expect("child observer poisoned");
        #[cfg(any(test, feature = "test-support"))]
        if matches!(observer.as_ref()?.source, ChildExitSource::Passive) {
            return None;
        }
        observer.take()
    }
}

impl Default for TerminalChildLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildExitObserver {
    pub(crate) async fn wait_and_publish(self, pid: Pid) -> ExitResult {
        let result = match self.source {
            ChildExitSource::Process => wait_for_child_exit(pid).await,
            #[cfg(any(test, feature = "test-support"))]
            ChildExitSource::Passive => unreachable!("passive observer cannot be installed"),
            #[cfg(test)]
            ChildExitSource::Controlled(receiver) => {
                receiver.await.unwrap_or(Err(nix::errno::Errno::ECHILD))
            }
        };
        self.publisher.publish(result);
        result
    }
}

#[cfg(unix)]
fn terminal_child_exit(status: WaitStatus) -> Option<TerminalExit> {
    match status {
        WaitStatus::Exited(_, code) => Some(TerminalExit::Exited(code)),
        WaitStatus::Signaled(_, signal, core_dumped) => {
            Some(TerminalExit::Signaled(signal, core_dumped))
        }
        WaitStatus::Stopped(..) | WaitStatus::Continued(_) | WaitStatus::StillAlive => None,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        WaitStatus::PtraceEvent(..) | WaitStatus::PtraceSyscall(_) => None,
    }
}

fn wait_uninterrupted(
    mut wait: impl FnMut() -> nix::Result<WaitStatus>,
) -> nix::Result<WaitStatus> {
    loop {
        match wait() {
            Err(nix::errno::Errno::EINTR) => {}
            result => return result,
        }
    }
}

#[cfg(unix)]
async fn wait_for_child_exit(child_pid: Pid) -> ExitResult {
    use nix::sys::wait::{waitpid, WaitPidFlag};

    // Register for SIGCHLD before the first observation so an exit between the
    // check and wait remains visible either as a pending signal or a zombie.
    let mut child_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())
        .map_err(|error| nix::Error::from_raw(error.raw_os_error().unwrap_or(libc::EIO)))?;
    loop {
        let status = wait_uninterrupted(|| waitpid(child_pid, Some(WaitPidFlag::WNOHANG)))?;
        if let Some(outcome) = terminal_child_exit(status) {
            return Ok(outcome);
        }
        let _ = child_signal.recv().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_terminal_wait_statuses_prove_exit() {
        let pid = Pid::from_raw(42);
        assert_eq!(
            terminal_child_exit(WaitStatus::Exited(pid, 23)),
            Some(TerminalExit::Exited(23))
        );
        assert_eq!(
            terminal_child_exit(WaitStatus::Signaled(pid, Signal::SIGTERM, false)),
            Some(TerminalExit::Signaled(Signal::SIGTERM, false))
        );
        for status in [
            WaitStatus::StillAlive,
            WaitStatus::Stopped(pid, Signal::SIGSTOP),
            WaitStatus::Continued(pid),
        ] {
            assert_eq!(terminal_child_exit(status), None);
        }
    }

    #[test]
    fn interrupted_wait_retries_but_other_errors_are_retained() {
        let pid = Pid::from_raw(42);
        let mut outcomes = [
            Err(nix::errno::Errno::EINTR),
            Err(nix::errno::Errno::EINTR),
            Ok(WaitStatus::Exited(pid, 0)),
        ]
        .into_iter();
        assert_eq!(
            wait_uninterrupted(|| outcomes.next().expect("wait call")),
            Ok(WaitStatus::Exited(pid, 0))
        );
        assert!(outcomes.next().is_none());
        assert_eq!(
            wait_uninterrupted(|| Err(nix::errno::Errno::ECHILD)),
            Err(nix::errno::Errno::ECHILD)
        );
    }

    #[tokio::test]
    async fn published_receipt_survives_its_unique_publisher_and_is_repeatable() {
        let (lifecycle, exit) = TerminalChildLifecycle::controlled();
        let receipt = lifecycle.receipt();
        let observer = lifecycle.take_observer().expect("unique observer");
        assert!(lifecycle.take_observer().is_none());
        exit.send(Ok(TerminalExit::Signaled(Signal::SIGHUP, false)))
            .expect("controlled observer");
        let expected = Ok(TerminalExit::Signaled(Signal::SIGHUP, false));
        assert_eq!(observer.wait_and_publish(Pid::from_raw(42)).await, expected);
        drop(lifecycle);
        assert!(receipt.exited());
        assert_eq!(receipt.wait().await, expected);
        assert_eq!(receipt.clone().wait().await, expected);
    }

    #[tokio::test]
    async fn missing_publisher_is_not_exit_proof() {
        let lifecycle = TerminalChildLifecycle::new();
        let receipt = lifecycle.receipt();
        drop(lifecycle);
        assert!(!receipt.exited());
        assert_eq!(receipt.wait().await, Err(nix::errno::Errno::ECHILD));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ptrace_observations_are_not_terminal_child_exits() {
        let pid = Pid::from_raw(42);
        assert_eq!(
            terminal_child_exit(WaitStatus::PtraceEvent(
                pid,
                Signal::SIGTRAP,
                libc::PTRACE_EVENT_EXIT
            )),
            None
        );
        assert_eq!(terminal_child_exit(WaitStatus::PtraceSyscall(pid)), None);
    }
}
