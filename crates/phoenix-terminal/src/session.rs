//! Terminal session handle and active-session registry.

use nix::unistd::Pid;
use std::collections::HashMap;
use std::os::unix::io::OwnedFd;
use std::sync::{Arc, Mutex};
use tokio::sync::{watch, Semaphore};

use super::command_tracker::CommandTracker;
use phoenix_core::work_scope::WorkScope;

/// Why the current relay should stop.
///
/// `Running` is the initial value on a fresh session. The relay watches for
/// transitions away from it:
/// - `Detach`: drop this relay so a reclaiming connection can take over. The
///   shell and `TerminalHandle` must survive.
/// - `TearDown`: the conversation reached a terminal state (REQ-TERM-012) or
///   another hard-stop path — the shell must die.
///
/// Branching on this value in the relay's exit handler lets us split "WS
/// close" (no shell kill) from "conversation end" (kill shell) without a
/// separate out-of-band flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Running,
    Detach,
    TearDown,
}

/// Shell integration detection state (REQ-TERM-015).
///
/// Transitions are one-shot per session: `Unknown` → `Detected` OR `Unknown` → `Absent`.
/// See `ShellIntegrationStatusMonotonic` invariant in `terminal.allium`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellIntegrationStatus {
    /// Initial state — within the detection window.
    Unknown,
    /// OSC 133;C marker observed within the detection window.
    Detected,
    /// Detection window elapsed without a C marker (REQ-TERM-015).
    /// Set by the frontend 5-second timeout; transitions are one-shot.
    #[allow(dead_code)]
    Absent,
}

/// Dimensions of a terminal (columns × rows).
///
/// Invariant: `cols >= 2 && rows >= 1`. Use `try_new` to construct;
/// the relay and WebSocket handler both enforce this at the boundary.
/// See `ResizeFrameRejected` in `terminal.allium`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dims {
    pub cols: u16,
    pub rows: u16,
}

impl Dims {
    /// Returns `Some(Dims)` iff `cols >= 2` and `rows >= 1`, else `None`.
    ///
    /// All construction sites must go through here so the invariant is
    /// structurally enforced rather than replicated in prose comments.
    #[must_use]
    pub fn try_new(cols: u16, rows: u16) -> Option<Self> {
        if cols >= 2 && rows >= 1 {
            Some(Self { cols, rows })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalChildKind {
    Shell,
    TmuxClient,
}

/// Owns the PTY master fd and child process.
///
/// `Drop` closes `master_fd`, which causes the kernel to deliver `SIGHUP`
/// to the shell's process group — the correct teardown chain.
pub struct TerminalHandle {
    /// PTY master file descriptor.  Closing this is the sole teardown trigger.
    pub master_fd: OwnedFd,
    /// Child shell PID.  Reaped by the reader task on EIO.
    pub child_pid: Pid,
    pub child_kind: TerminalChildKind,
    /// Command tracker — fed with every PTY output byte (REQ-TERM-010, REQ-TERM-021).
    pub tracker: Arc<Mutex<CommandTracker>>,
    /// Shell integration detection state.
    pub shell_integration_status: Arc<Mutex<ShellIntegrationStatus>>,
    /// Signal the currently-attached relay to stop.
    ///
    /// Kept on the handle (not the relay) so a reclaiming connection can
    /// drive the sitting relay to exit without touching its local state.
    /// Reset to `Running` before each new relay starts.
    pub stop_tx: watch::Sender<StopReason>,
    /// Single-occupant slot for the attached relay (exactly-one-winner guarantee).
    ///
    /// Initialized with 1 permit. The attached relay holds an
    /// `OwnedSemaphorePermit` for its entire lifetime; releasing the permit
    /// (by dropping it on detach / teardown) is the authoritative signal that
    /// the slot is free for the next reclaimer to take.
    ///
    /// Two concurrent reclaimers cannot both acquire this permit — the
    /// semaphore structurally enforces "exactly one relay attached at a time",
    /// so neither reclaimer can proceed into the relay's acquire path while
    /// another relay is still running. See task 24691 follow-up.
    pub attach_permit: Arc<Semaphore>,
}

impl std::fmt::Debug for TerminalHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalHandle")
            .field("child_pid", &self.child_pid)
            .finish_non_exhaustive()
    }
}

/// Shared registry of active terminal sessions (REQ-TERM-003, REQ-TERM-WS-001).
///
/// Keyed by `WorkScope`: `WorkScope::Worktree(path)` for managed/branch
/// worktrees, `WorkScope::Conversation(id)` for Direct conversations, and
/// `WorkScope::Global` for the singleton scope surfaced on the /new page.
/// Continuation conversations resolving to the same `WorkScope` therefore
/// share a single entry rather than colliding — matching the ownership
/// pattern of `TmuxRegistry` (REQ-TMUX-WS-001) and `BrowserSessionManager`
/// (REQ-BROWSER-WS-001).
///
/// `Arc`-wrapped so it can be cloned into `AppState` and into handlers.
/// `Mutex` provides the atomic check-and-insert needed for the race guard
/// on the fresh-session path.
#[derive(Clone, Default)]
pub struct ActiveTerminals(pub Arc<Mutex<HashMap<WorkScope, Arc<TerminalHandle>>>>);

impl ActiveTerminals {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn snapshot_shell_session_ids(&self) -> Vec<i32> {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.values()
            .filter(|handle| handle.child_kind == TerminalChildKind::Shell)
            .map(|handle| handle.child_pid.as_raw())
            .collect()
    }

    /// Returns `true` if a terminal is currently active for `scope`.
    ///
    /// Retained for tests; the WebSocket handler goes directly to `get`
    /// (reclaim path) or `try_insert` (fresh path) without a separate
    /// pre-check, since a pre-check can't avoid the reclaim race.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[cfg(test)]
    #[allow(dead_code)]
    #[must_use]
    pub fn is_active(&self, scope: &WorkScope) -> bool {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.contains_key(scope)
    }

    /// Attempt to register a new terminal for `scope`.
    ///
    /// Returns `None` if a terminal is already active for the scope.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn try_insert(
        &self,
        scope: WorkScope,
        handle: TerminalHandle,
    ) -> Option<Arc<TerminalHandle>> {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if map.contains_key(&scope) {
            return None;
        }
        let arc = Arc::new(handle);
        map.insert(scope, Arc::clone(&arc));
        Some(arc)
    }

    /// Remove the terminal for `scope`, if present.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub fn remove(&self, scope: &WorkScope) {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        map.remove(scope);
    }

    /// Look up an active terminal.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn get(&self, scope: &WorkScope) -> Option<Arc<TerminalHandle>> {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.get(scope).cloned()
    }

    /// Cascade-cleanup entry for `run_resource_cleanup_cascade`
    /// (REQ-TERM-WS-001, REQ-TERM-012). Mirrors `TmuxRegistry::cascade_on_delete`
    /// and `BrowserSessionManager::cascade_on_delete`:
    ///
    ///   - If `inheritor_scope == Some(work_scope)`, the continuation
    ///     conversation resolves to the same scope and still owns the
    ///     terminal. Skip teardown.
    ///   - Otherwise, remove the registry entry, signal any attached relay
    ///     to tear down via `StopReason::TearDown`, and reap the shell
    ///     when no relay is attached (when a relay is attached, its own
    ///     teardown branch calls `waitpid`).
    ///
    /// Best-effort. Returns silently if no terminal is registered for the
    /// scope — that is the common case during cascade for scopes that
    /// never spawned a user terminal (sub-agent conversations, etc.).
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub async fn cascade_on_delete(
        &self,
        work_scope: &WorkScope,
        inheritor_scope: Option<&WorkScope>,
    ) {
        if inheritor_scope == Some(work_scope) {
            tracing::debug!(
                work_scope = %work_scope,
                "terminal: skipping teardown — scope inherited by continuation"
            );
            return;
        }

        let handle = {
            let mut map = self.0.lock().expect("terminal registry poisoned");
            map.remove(work_scope)
        };
        let Some(handle) = handle else {
            return;
        };

        let child_pid = handle.child_pid;
        // attach_permit starts with 1 permit; an attached relay holds it
        // for its lifetime, so `available_permits() == 0` means a relay is
        // currently attached.
        let relay_attached = handle.attach_permit.available_permits() == 0;

        // Signal an attached relay to tear down. The relay's exit branch
        // runs `full_teardown` which closes the master_fd, reaps the
        // child, and exits. `send` returns Err if no receivers are alive
        // (no relay attached) — we ignore.
        let _ = handle.stop_tx.send(StopReason::TearDown);

        // Drop our registry-owned Arc clone. If a relay is attached, it
        // still has its own clone + master_fd dup, so master_fd stays
        // open until the relay's clean-up. If no relay is attached, our
        // clone was the last reference: Drop fires now, master_fd closes,
        // SIGHUP delivers to the shell's process group.
        drop(handle);

        if !relay_attached {
            // No relay to reap the child for us. Do it here.
            // spawn_blocking because waitpid is sync; ignore errors —
            // ECHILD just means someone else already reaped it.
            let _ = tokio::task::spawn_blocking(move || {
                let _ = nix::sys::wait::waitpid(child_pid, None);
            })
            .await;
        }

        tracing::info!(
            work_scope = %work_scope,
            relay_attached,
            "terminal: cascade teardown complete"
        );
    }
}

/// Convenience function for the cleanup-cascade orchestrator. Mirrors
/// `cascade_tmux_on_delete` and `cascade_browser_on_delete`.
pub async fn cascade_terminal_on_delete(
    terminals: &ActiveTerminals,
    work_scope: &WorkScope,
    inheritor_scope: Option<&WorkScope>,
) {
    terminals
        .cascade_on_delete(work_scope, inheritor_scope)
        .await;
}
