//! Terminal session handle and active-session registry.

use nix::unistd::Pid;
use std::collections::HashMap;
use std::os::unix::io::OwnedFd;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use vt100::Parser;

/// Dimensions of a terminal (columns × rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dims {
    pub cols: u16,
    pub rows: u16,
}

/// Signals quiescence (no output for 300ms) to waiting `read_terminal` callers.
pub type QuiescenceTx = watch::Sender<u64>;
#[allow(dead_code)] // Used by read_terminal tool (Task 6)
pub type QuiescenceRx = watch::Receiver<u64>;

/// Owns the PTY master fd and child process.
///
/// `Drop` closes `master_fd`, which causes the kernel to deliver `SIGHUP`
/// to the shell's process group — the correct teardown chain.
pub struct TerminalHandle {
    /// PTY master file descriptor.  Closing this is the sole teardown trigger.
    pub master_fd: OwnedFd,
    /// Child shell PID.  Reaped by the reader task on EIO.
    pub child_pid: Pid,
    /// vt100 parser — updated by the reader task on every byte (REQ-TERM-010).
    pub parser: Arc<Mutex<Parser>>,
    /// Quiescence notification — incremented after 300ms silence.
    pub quiescence_tx: QuiescenceTx,
}

impl std::fmt::Debug for TerminalHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalHandle")
            .field("child_pid", &self.child_pid)
            .finish_non_exhaustive()
    }
}

/// Shared registry of active terminal sessions (REQ-TERM-003).
///
/// `Arc`-wrapped so it can be cloned into `AppState` and into handlers.
/// `Mutex` provides the atomic check-and-insert needed for the 409 guard.
#[derive(Clone, Default)]
pub struct ActiveTerminals(pub Arc<Mutex<HashMap<String, Arc<TerminalHandle>>>>);

impl ActiveTerminals {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Returns `true` if a terminal is currently active for `conversation_id`.
    ///
    /// Used as a fast pre-spawn check (REQ-TERM-003) to avoid wasting a
    /// fork+exec on duplicate connections. `try_insert` is still the
    /// authoritative atomic guard against races.
    pub fn is_active(&self, conversation_id: &str) -> bool {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.contains_key(conversation_id)
    }

    /// Attempt to register a new terminal for `conversation_id`.
    ///
    /// Returns `None` if a terminal is already active (409 case).
    pub fn try_insert(
        &self,
        conversation_id: String,
        handle: TerminalHandle,
    ) -> Option<Arc<TerminalHandle>> {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if map.contains_key(&conversation_id) {
            return None; // 409 — already active
        }
        let arc = Arc::new(handle);
        map.insert(conversation_id, Arc::clone(&arc));
        Some(arc)
    }

    /// Remove the terminal for `conversation_id`, if present.
    pub fn remove(&self, conversation_id: &str) {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        map.remove(conversation_id);
    }

    /// Look up an active terminal.
    #[allow(dead_code)] // Used by read_terminal tool (Task 6)
    pub fn get(&self, conversation_id: &str) -> Option<Arc<TerminalHandle>> {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.get(conversation_id).cloned()
    }
}
