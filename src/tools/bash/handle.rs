//! Bash handle — the durable identity of one spawned command.
//!
//! REQ-BASH-001/002/003/004/006: `Handle`, `HandleState`, `FinalCause`,
//! the single `transition_to_terminal` helper through which all state
//! writes pass, and the `ExitWatchPanicGuard` that publishes a sentinel
//! on waiter-task panic.
//!
//! Some accessors here are surface for the future hard-delete cascade
//! (task 02696) and the wire/UI migration (task 02697); silence the
//! per-method dead-code lint until those land.
#![allow(dead_code)]
//!
//! Lock ordering (per `bash.allium` @guidance on `HandleProcessExited`
//! and `HandleKillPendingKernel`):
//!
//! - All `HandleState` writes go through [`Handle::transition_to_terminal`].
//!   It acquires the handle's `RwLock<Arc<HandleState>>` for write at
//!   exactly one point.
//! - [`Handle::transition_to_terminal`] refuses to regress from a terminal
//!   state (`Tombstoned`) back to live — the late-exit-vs-timer race fix.
//! - The watch-channel `exit_signal` is sent AFTER the state swap, so
//!   any `wait` caller that wakes on `changed()` and then re-reads the
//!   state observes the new (terminal) value.
//! - Kill-attempt bookkeeping lives in a separate `RwLock<Option<KillAttempt>>`
//!   on `Handle` rather than inside `LiveData`, so recording an attempt
//!   doesn't require swapping the `Arc<HandleState>` (which would
//!   invalidate readers' ring snapshots).

use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::watch;
use tokio::sync::{Mutex, RwLock};

use super::ring::{RingBuffer, RingLine};

/// Default tombstone tail size (REQ-BASH-006: `TOMBSTONE_TAIL_LINES`).
pub const TOMBSTONE_TAIL_LINES: usize = 2000;

/// Stable handle identifier within a conversation.
///
/// Format is implementation detail (sequential `b-1`, `b-2`, ...).
/// The Allium contract is only that the pair `(conversation, handle_id)`
/// is unique.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HandleId(pub String);

impl HandleId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HandleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Kill signal the agent may request. Sent EXACTLY ONCE per kill call —
/// no auto-escalation (REQ-BASH-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSignal {
    Term,
    Kill,
}

impl KillSignal {
    /// Stable string identifier used in tool responses.
    pub fn as_str(self) -> &'static str {
        match self {
            KillSignal::Term => "TERM",
            KillSignal::Kill => "KILL",
        }
    }

    #[cfg(unix)]
    pub fn as_libc(self) -> i32 {
        match self {
            KillSignal::Term => libc::SIGTERM,
            KillSignal::Kill => libc::SIGKILL,
        }
    }
}

/// Cause of a handle's transition to a terminal state.
///
/// Aligned with the `HandleStatus` enum in `bash.allium` — `Exited` and
/// `Killed` are the two terminal causes that `transition_to_terminal`
/// accepts. `KillPendingKernel` is a separate non-terminal status modeled
/// via [`KillAttempt`] on the live handle, NOT a `FinalCause`.
#[derive(Debug, Clone)]
pub enum FinalCause {
    /// Process exited with a kernel-supplied status code.
    Exited { exit_code: Option<i32> },
    /// Process terminated by signal — Phoenix-sent or external (oom-killer,
    /// external kill). `signal_number` is `None` when the kernel did not
    /// report one.
    Killed {
        exit_code: Option<i32>,
        signal_number: Option<i32>,
    },
}

/// Live data carried while the process is running (or in
/// `kill_pending_kernel` — the process is still alive in that state per
/// `KillPendingKernelImpliesAlive`).
#[derive(Debug)]
pub struct LiveData {
    /// Process group id. Used for signaling the entire group via `kill(-pgid, sig)`.
    pub pgid: i32,
    /// Native pid (informational; matching `Handle.pid` in the Allium spec).
    pub pid: u32,
    /// Output ring. `Mutex` rather than `RwLock` because reader tasks
    /// always need exclusive write access on append; peek readers hold
    /// it briefly to snapshot a window.
    pub ring: Mutex<RingBuffer>,
}

/// Phoenix-side bookkeeping for a kill that is still pending in the kernel.
#[derive(Debug, Clone, Copy)]
pub struct KillAttempt {
    pub signal_sent: KillSignal,
    pub attempted_at: SystemTime,
}

/// Tombstone — written exactly once when the handle transitions to a
/// fully terminal state (`Exited` or `Killed`). Replaces the live ring.
#[derive(Debug)]
pub struct Tombstone {
    pub final_cause: FinalCause,
    pub exit_code: Option<i32>,
    /// The signal that terminated the process, when known. Optional;
    /// matches `Handle.signal_number` in the Allium spec.
    pub signal_number: Option<i32>,
    pub duration_ms: u64,
    pub finished_at: SystemTime,
    pub final_tail: Vec<RingLine>,
    /// `next_offset` at the moment of demotion. Lets `truncated_before`
    /// be computed for tombstone reads.
    pub next_offset_at_exit: u64,
    /// If a kill was attempted before this terminal state was reached,
    /// these record the last attempt that landed.
    pub kill_signal_sent: Option<KillSignal>,
    pub kill_attempted_at: Option<SystemTime>,
}

/// Enum that makes invalid handle states unrepresentable: a live handle
/// has a ring and no exit code; a tombstoned handle has neither.
#[derive(Debug)]
pub enum HandleState {
    Live(LiveData),
    Tombstoned(Tombstone),
}

impl HandleState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, HandleState::Tombstoned(_))
    }

    /// True when the handle's process is still alive — i.e. status is
    /// `running` or `kill_pending_kernel`. Both share `Live` representation;
    /// the `kill_pending_kernel` distinction is made by inspecting the
    /// handle's `kill_attempt` field.
    pub fn is_live(&self) -> bool {
        matches!(self, HandleState::Live(_))
    }
}

/// Sentinel value published on the exit watch channel.
///
/// `Exited` covers both `FinalCause::Exited` and `FinalCause::Killed` —
/// the watch channel is a "the handle reached a terminal state" signal,
/// not a cause discriminator. Callers re-read `handle.state()` for the
/// authoritative cause.
///
/// `WaiterPanicked` is the panic-guard sentinel — see [`ExitWatchPanicGuard`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitState {
    Exited,
    WaiterPanicked,
}

/// A bash handle: identity + lifecycle state + watch-channel for exit notification.
///
/// The handle is owned by exactly one `ConversationHandles` registry entry.
/// `state` is `RwLock<Arc<HandleState>>` (per design.md) — peek and wait
/// readers clone the `Arc` without holding the outer lock while they
/// shape responses. Writers hold the outer lock for write to swap the
/// `Arc`.
// `handle_id` and `conversation_id` mirror the Allium `Handle` entity's
// field names; renaming them to satisfy clippy's struct-name-prefix lint
// would diverge from the spec.
#[allow(clippy::struct_field_names)]
pub struct Handle {
    pub conversation_id: String,
    pub handle_id: HandleId,
    pub cmd: String,
    pub started_at: SystemTime,
    /// The current handle state. Always written through
    /// [`Self::transition_to_terminal`].
    state: RwLock<Arc<HandleState>>,
    /// Kill-attempt bookkeeping for `kill_pending_kernel` semantics.
    /// Separate from `LiveData` so recording an attempt does not require
    /// swapping the `Arc<HandleState>` (and therefore does not invalidate
    /// reader snapshots of the ring).
    kill_attempt: RwLock<Option<KillAttempt>>,
    /// Watch channel: `None` until a terminal transition publishes
    /// `Some(ExitState::*)`. Late subscribers see the most recent value
    /// (per `tokio::sync::watch` semantics).
    exit_signal: watch::Sender<Option<ExitState>>,
    exit_observer: watch::Receiver<Option<ExitState>>,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("conversation_id", &self.conversation_id)
            .field("handle_id", &self.handle_id)
            .field("cmd", &self.cmd)
            .field("started_at", &self.started_at)
            .finish_non_exhaustive()
    }
}

impl Handle {
    /// Construct a fresh live handle for a freshly spawned child.
    ///
    /// `pgid` and `pid` are recorded; the ring is created at the
    /// configured `RING_BUFFER_BYTES` cap.
    // pgid/pid mirror the `Handle` entity field names from `bash.allium`;
    // renaming for clippy's similar-names lint would diverge from the spec.
    #[allow(clippy::similar_names)]
    pub fn new_live(
        conversation_id: String,
        handle_id: HandleId,
        cmd: String,
        pgid: i32,
        pid: u32,
        ring_bytes_cap: usize,
    ) -> Arc<Self> {
        let live = LiveData {
            pgid,
            pid,
            ring: Mutex::new(RingBuffer::new(ring_bytes_cap)),
        };
        let (tx, rx) = watch::channel::<Option<ExitState>>(None);
        Arc::new(Self {
            conversation_id,
            handle_id,
            cmd,
            started_at: SystemTime::now(),
            state: RwLock::new(Arc::new(HandleState::Live(live))),
            kill_attempt: RwLock::new(None),
            exit_signal: tx,
            exit_observer: rx,
        })
    }

    /// Snapshot the current state. Cloning the `Arc` is cheap; callers
    /// can release the outer lock before doing real work.
    pub async fn state(&self) -> Arc<HandleState> {
        self.state.read().await.clone()
    }

    /// Read the handle's most recent kill attempt, if any. `Some` iff
    /// the handle is in `kill_pending_kernel` (status-wise) — i.e. the
    /// kill response timer has fired but the process has not yet exited.
    /// Cleared back to `None` on terminal transition (see
    /// [`Self::transition_to_terminal`]).
    pub async fn kill_attempt(&self) -> Option<KillAttempt> {
        *self.kill_attempt.read().await
    }

    /// Receiver for the exit watch. Each `wait`/`spawn` call MUST clone
    /// a fresh receiver from this method (see design.md "Watch-channel rule").
    pub fn exit_observer(&self) -> watch::Receiver<Option<ExitState>> {
        self.exit_observer.clone()
    }

    /// Record that a kill response timer fired without observing exit.
    /// Mutates only `kill_attempt`; the `state` remains `Live`. If the
    /// handle is already terminal at the moment of the call (the waiter
    /// beat the timer), this is a no-op and returns `false`.
    ///
    /// Lock ordering: takes the `state` read-lock first to test for the
    /// terminal-state regression case, then the `kill_attempt` write-lock.
    /// Documented order: state-then-attempt.
    pub async fn mark_kill_pending_kernel(
        &self,
        signal: KillSignal,
        attempted_at: SystemTime,
    ) -> bool {
        // First check we're not racing a late terminal transition.
        let state_guard = self.state.read().await;
        if state_guard.is_terminal() {
            tracing::debug!(
                handle_id = %self.handle_id,
                "kill timer fired after handle reached terminal state — \
                 mark_kill_pending_kernel is a no-op"
            );
            return false;
        }
        // We hold the state read-lock while writing the attempt slot —
        // ensures no concurrent transition_to_terminal writer can demote
        // us in between (transition_to_terminal takes state.write()).
        let mut attempt = self.kill_attempt.write().await;
        *attempt = Some(KillAttempt {
            signal_sent: signal,
            attempted_at,
        });
        drop(attempt);
        drop(state_guard);
        true
    }

    /// THE single helper through which every Live -> Tombstoned transition
    /// passes. Holds the outer `state` write-lock; refuses to regress from
    /// a terminal state.
    ///
    /// Returns `true` iff the handle was newly transitioned to terminal
    /// (i.e. it was previously Live). Returns `false` if the handle was
    /// already terminal — in that case the caller's late event (typically
    /// the kill response timer firing AFTER the waiter task already
    /// demoted) is dropped on the floor, which is the correct behavior
    /// per `bash.allium`'s @guidance on `HandleProcessExited`.
    ///
    /// Caller responsibility: after this returns `true`, call
    /// [`Self::publish_exit`] with `ExitState::Exited`. The two-step
    /// (state swap, then signal) is intentional: late subscribers wake
    /// on the signal and re-read state, observing the new value.
    pub async fn transition_to_terminal(
        &self,
        cause: FinalCause,
        duration: std::time::Duration,
        tombstone_tail_lines: usize,
    ) -> bool {
        let mut guard = self.state.write().await;
        match guard.as_ref() {
            HandleState::Tombstoned(_) => {
                tracing::debug!(
                    handle_id = %self.handle_id,
                    "transition_to_terminal: handle already terminal — ignoring late event"
                );
                false
            }
            HandleState::Live(live) => {
                // Snapshot the ring under its mutex. We hold the outer
                // state write-lock, so no other writer can swap the Arc;
                // readers may hold older Arc clones but their ring
                // snapshots are independent of ours.
                let ring = live.ring.lock().await;
                let final_tail = ring.snapshot_tail(tombstone_tail_lines);
                let next_offset_at_exit = ring.next_offset();
                drop(ring);

                // Pull the most recent kill attempt (if any) — Copy type,
                // so deref-the-guard is sufficient.
                let attempt = *self.kill_attempt.read().await;
                let kill_signal_sent = attempt.map(|k| k.signal_sent);
                let kill_attempted_at = attempt.map(|k| k.attempted_at);

                let (exit_code, signal_number) = match &cause {
                    FinalCause::Exited { exit_code } => (*exit_code, None),
                    FinalCause::Killed {
                        exit_code,
                        signal_number,
                    } => (*exit_code, *signal_number),
                };
                let tomb = Tombstone {
                    final_cause: cause,
                    exit_code,
                    signal_number,
                    duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    finished_at: SystemTime::now(),
                    final_tail,
                    next_offset_at_exit,
                    kill_signal_sent,
                    kill_attempted_at,
                };
                *guard = Arc::new(HandleState::Tombstoned(tomb));
                true
            }
        }
    }

    /// Publish the exit sentinel on the watch channel. Call AFTER
    /// `transition_to_terminal` so the state is observable when waiters
    /// wake. Idempotent — a second send overwrites the first, but
    /// `Option<ExitState>` only has terminal variants past `None` so
    /// the practical effect is no different.
    pub fn publish_exit(&self, state: ExitState) {
        let _ = self.exit_signal.send(Some(state));
    }

    /// Return the live `LiveData`'s pgid if the handle is currently live
    /// (running or `kill_pending_kernel`). Used by the shutdown kill-tree
    /// pass and the hard-delete cascade.
    pub async fn live_pgid(&self) -> Option<i32> {
        match self.state.read().await.as_ref() {
            HandleState::Live(live) => Some(live.pgid),
            HandleState::Tombstoned(_) => None,
        }
    }
}

/// Drop guard that publishes `ExitState::WaiterPanicked` if the waiter
/// task panics before disarming. Without this, a panic in the waiter would
/// leave the watch channel at `None` forever and any `wait` caller would
/// hang on `changed().await`.
///
/// Use:
/// ```ignore
/// let guard = ExitWatchPanicGuard::new(handle.clone());
/// // ... do work that might panic ...
/// guard.disarm();
/// ```
#[must_use = "ExitWatchPanicGuard publishes a sentinel on drop unless disarmed"]
pub struct ExitWatchPanicGuard {
    handle: Option<Arc<Handle>>,
}

impl ExitWatchPanicGuard {
    pub fn new(handle: Arc<Handle>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    /// Disarm the guard — call after the waiter task has successfully
    /// published its terminal state. After disarm, `Drop` is a no-op.
    pub fn disarm(mut self) {
        self.handle = None;
    }
}

impl Drop for ExitWatchPanicGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            tracing::error!(
                handle_id = %handle.handle_id,
                "bash waiter task dropped without disarm — publishing WaiterPanicked"
            );
            let _ = handle.exit_signal.send(Some(ExitState::WaiterPanicked));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn live_handle() -> Arc<Handle> {
        Handle::new_live(
            "conv-1".into(),
            HandleId::new("b-1"),
            "echo hi".into(),
            12345,
            12345,
            super::super::ring::RING_BUFFER_BYTES,
        )
    }

    #[tokio::test]
    async fn transition_to_terminal_sets_tombstone_and_returns_true() {
        let h = live_handle();
        let did_transition = h
            .transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                Duration::from_millis(42),
                TOMBSTONE_TAIL_LINES,
            )
            .await;
        assert!(did_transition);
        let state = h.state().await;
        match state.as_ref() {
            HandleState::Tombstoned(t) => {
                assert_eq!(t.exit_code, Some(0));
                assert_eq!(t.duration_ms, 42);
                assert!(matches!(t.final_cause, FinalCause::Exited { .. }));
            }
            HandleState::Live(_) => panic!("expected tombstone"),
        }
    }

    #[tokio::test]
    async fn transition_to_terminal_refuses_regression_from_terminal() {
        let h = live_handle();
        // First transition to Exited.
        assert!(
            h.transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                Duration::from_millis(10),
                TOMBSTONE_TAIL_LINES,
            )
            .await
        );

        // A "late kill response timer" race would attempt to write
        // FinalCause::Killed back over the terminal state. The helper
        // must refuse.
        let did_overwrite = h
            .transition_to_terminal(
                FinalCause::Killed {
                    exit_code: None,
                    signal_number: Some(15),
                },
                Duration::from_millis(99),
                TOMBSTONE_TAIL_LINES,
            )
            .await;
        assert!(
            !did_overwrite,
            "transition_to_terminal must not overwrite an already-terminal state"
        );
        let state = h.state().await;
        match state.as_ref() {
            HandleState::Tombstoned(t) => {
                // Original cause preserved.
                assert!(matches!(t.final_cause, FinalCause::Exited { .. }));
                assert_eq!(t.exit_code, Some(0));
                assert_eq!(t.duration_ms, 10);
            }
            HandleState::Live(_) => panic!("expected tombstone"),
        }
    }

    #[tokio::test]
    async fn transition_to_terminal_refuses_regression_to_kill_pending_kernel() {
        // The exact race the spec @guidance calls out: waiter task already
        // wrote Tombstone(Exited); the kill response timer expires AFTER
        // and tries to mark kill_pending_kernel via mark_kill_pending_kernel.
        // The helper must refuse — the terminal state must not regress.
        let h = live_handle();
        assert!(
            h.transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                Duration::from_millis(10),
                TOMBSTONE_TAIL_LINES,
            )
            .await
        );
        let did_mark = h
            .mark_kill_pending_kernel(KillSignal::Term, SystemTime::now())
            .await;
        assert!(
            !did_mark,
            "mark_kill_pending_kernel must not regress a terminal handle"
        );
        // No KillAttempt was recorded.
        assert!(h.kill_attempt().await.is_none());
    }

    #[tokio::test]
    async fn transition_to_terminal_killed_records_signal_number() {
        let h = live_handle();
        let did_transition = h
            .transition_to_terminal(
                FinalCause::Killed {
                    exit_code: None,
                    signal_number: Some(9),
                },
                Duration::from_millis(5),
                TOMBSTONE_TAIL_LINES,
            )
            .await;
        assert!(did_transition);
        let state = h.state().await;
        match state.as_ref() {
            HandleState::Tombstoned(t) => {
                assert!(matches!(t.final_cause, FinalCause::Killed { .. }));
                assert_eq!(t.signal_number, Some(9));
                assert_eq!(t.exit_code, None);
            }
            HandleState::Live(_) => panic!("expected tombstone"),
        }
    }

    #[tokio::test]
    async fn mark_kill_pending_kernel_records_attempt_on_live_handle() {
        let h = live_handle();
        let now = SystemTime::now();
        let did_mark = h.mark_kill_pending_kernel(KillSignal::Term, now).await;
        assert!(did_mark);
        let attempt = h.kill_attempt().await.expect("attempt recorded");
        assert_eq!(attempt.signal_sent, KillSignal::Term);
        assert_eq!(attempt.attempted_at, now);
        // Handle is still Live (status: kill_pending_kernel).
        assert!(h.state().await.is_live());
    }

    #[tokio::test]
    async fn transition_after_kill_pending_kernel_carries_kill_metadata_into_tombstone() {
        let h = live_handle();
        let attempted = SystemTime::now();
        assert!(
            h.mark_kill_pending_kernel(KillSignal::Term, attempted)
                .await
        );
        // Now the late-arriving exit fires.
        assert!(
            h.transition_to_terminal(
                FinalCause::Killed {
                    exit_code: None,
                    signal_number: Some(15),
                },
                Duration::from_millis(50),
                TOMBSTONE_TAIL_LINES,
            )
            .await
        );
        let state = h.state().await;
        match state.as_ref() {
            HandleState::Tombstoned(t) => {
                assert_eq!(t.kill_signal_sent, Some(KillSignal::Term));
                assert_eq!(t.kill_attempted_at, Some(attempted));
                assert_eq!(t.signal_number, Some(15));
            }
            HandleState::Live(_) => panic!("expected tombstone"),
        }
    }

    #[tokio::test]
    async fn panic_guard_publishes_waiter_panicked_on_drop_without_disarm() {
        let h = live_handle();
        let mut rx = h.exit_observer();
        // Initial value is None.
        assert_eq!(*rx.borrow(), None);

        {
            // Construct and immediately drop without disarm — simulates
            // a waiter task panicking mid-flight.
            let _guard = ExitWatchPanicGuard::new(h.clone());
        }

        // The drop must have published WaiterPanicked.
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), Some(ExitState::WaiterPanicked));
    }

    #[tokio::test]
    async fn panic_guard_disarm_does_not_publish() {
        let h = live_handle();
        let mut rx = h.exit_observer();
        {
            let guard = ExitWatchPanicGuard::new(h.clone());
            guard.disarm();
        }
        // changed() with a tiny timeout should NOT see anything.
        let did_change = tokio::time::timeout(Duration::from_millis(50), rx.changed())
            .await
            .is_ok();
        assert!(
            !did_change,
            "panic guard disarm must not publish a value on drop"
        );
        assert_eq!(*rx.borrow(), None);
    }

    #[tokio::test]
    async fn publish_exit_after_transition_wakes_waiters() {
        let h = live_handle();
        let mut rx = h.exit_observer();

        let waiter = tokio::spawn(async move {
            rx.changed().await.unwrap();
            *rx.borrow()
        });

        // Real flow: transition, then publish.
        assert!(
            h.transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                Duration::from_millis(1),
                TOMBSTONE_TAIL_LINES,
            )
            .await
        );
        h.publish_exit(ExitState::Exited);

        let observed = waiter.await.unwrap();
        assert_eq!(observed, Some(ExitState::Exited));
    }
}
