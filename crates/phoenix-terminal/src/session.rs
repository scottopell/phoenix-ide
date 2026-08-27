//! Terminal session handle and active-session registry.

use nix::unistd::Pid;
use std::collections::HashMap;
use std::os::unix::io::OwnedFd;
use std::sync::{Arc, Mutex};
use tokio::sync::{watch, Semaphore};

use super::command_tracker::CommandTracker;
use phoenix_core::{process_identity::ProcessIdentity, work_scope::ResourceScopeKey};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunchIdentity {
    pub process: ProcessIdentity,
    pub launch_uuid: String,
}

impl TerminalLaunchIdentity {
    #[must_use]
    pub fn stable_identity(&self) -> String {
        format!(
            "pid:{}:start:{}:launch:{}",
            self.process.pid, self.process.start_time, self.launch_uuid
        )
    }
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
    /// Durable launch identity captured at spawn time.
    pub launch_identity: TerminalLaunchIdentity,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalRetirementGeneration(u64);

impl TerminalRetirementGeneration {
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInstanceIdentity {
    durable: TerminalLaunchIdentity,
    handle_addr: usize,
}

impl TerminalInstanceIdentity {
    #[must_use]
    pub fn from_handle(handle: &Arc<TerminalHandle>) -> Self {
        Self {
            durable: handle.launch_identity.clone(),
            handle_addr: Arc::as_ptr(handle) as usize,
        }
    }

    #[must_use]
    pub fn stable_identity(&self) -> String {
        self.durable.stable_identity()
    }

    #[must_use]
    pub fn process_identity(&self) -> ProcessIdentity {
        self.durable.process
    }

    #[must_use]
    pub fn matches_handle(&self, handle: &Arc<TerminalHandle>) -> bool {
        self.handle_addr == Arc::as_ptr(handle) as usize && self.durable == handle.launch_identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRetirementPermit {
    pub work_scope: ResourceScopeKey,
    pub instance: Option<TerminalInstanceIdentity>,
    generation: TerminalRetirementGeneration,
    had_entry: bool,
}

impl TerminalRetirementPermit {
    #[must_use]
    pub fn generation(&self) -> TerminalRetirementGeneration {
        self.generation
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalRetirementOutcome {
    Retired,
    AbsenceVerified,
    Residual { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTerminalInsertError {
    Occupied,
    RetirementFenced,
}

#[derive(Debug, Default)]
struct ActiveTerminalRegistryState {
    handles: HashMap<ResourceScopeKey, Arc<TerminalHandle>>,
    retirements: HashMap<ResourceScopeKey, ScopeRetirementState>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ScopeRetirementState {
    generation: u64,
    fenced: bool,
}

/// Shared registry of active terminal sessions (REQ-TERM-003, REQ-TERM-WS-001).
///
/// Conversation terminals are keyed by durable work-scope identity;
/// `ResourceScopeKey::GlobalTerminal` is structurally separate for `/new`.
/// Continuation conversations in the same work scope therefore
/// share a single entry rather than colliding — matching the ownership
/// pattern of `TmuxRegistry` (REQ-TMUX-WS-001) and `BrowserSessionManager`
/// (REQ-BROWSER-WS-001).
///
/// `Arc`-wrapped so it can be cloned into `AppState` and into handlers.
/// `Mutex` provides the atomic check-and-insert needed for the race guard
/// on the fresh-session path.
#[derive(Clone, Default)]
pub struct ActiveTerminals(Arc<Mutex<ActiveTerminalRegistryState>>);

impl ActiveTerminals {
    #[cfg(test)]
    pub(crate) fn active_count_for_scope(&self, scope: &ResourceScopeKey) -> usize {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.handles
            .iter()
            .filter(|(candidate, _)| *candidate == scope)
            .count()
    }

    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(ActiveTerminalRegistryState::default())))
    }

    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn snapshot_shell_session_ids(&self) -> Vec<i32> {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.handles
            .values()
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
    pub fn is_active(&self, scope: &ResourceScopeKey) -> bool {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.handles.contains_key(scope)
    }

    /// Attempt to register a new terminal for `scope`.
    ///
    /// Returns a typed error when the scope is already occupied or fenced for
    /// retirement. The legacy [`Self::try_insert`] wrapper preserves the older
    /// `Option`-based API by collapsing both rejections to `None`.
    ///
    /// # Errors
    /// Returns [`ActiveTerminalInsertError`] when the scope is occupied or fenced.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub fn try_insert_exact(
        &self,
        scope: ResourceScopeKey,
        handle: TerminalHandle,
    ) -> Result<Arc<TerminalHandle>, ActiveTerminalInsertError> {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if map
            .retirements
            .get(&scope)
            .is_some_and(|retirement| retirement.fenced)
        {
            return Err(ActiveTerminalInsertError::RetirementFenced);
        }
        if map.handles.contains_key(&scope) {
            return Err(ActiveTerminalInsertError::Occupied);
        }
        let arc = Arc::new(handle);
        map.handles.insert(scope, Arc::clone(&arc));
        Ok(arc)
    }

    /// Attempt to register a new terminal for `scope`.
    ///
    /// Returns `None` if a terminal is already active for the scope or if the
    /// scope is fenced for retirement.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn try_insert(
        &self,
        scope: ResourceScopeKey,
        handle: TerminalHandle,
    ) -> Option<Arc<TerminalHandle>> {
        self.try_insert_exact(scope, handle).ok()
    }

    /// Remove one relay-owned handle without clearing a Close fence.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub fn remove_from_relay(&self, scope: &ResourceScopeKey) {
        self.0
            .lock()
            .expect("terminal registry poisoned")
            .handles
            .remove(scope);
    }

    /// Remove the terminal for `scope` and explicitly reopen normal admission.
    ///
    /// This is reserved for non-Close cleanup that owns both effects.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub fn remove_and_reopen(&self, scope: &ResourceScopeKey) {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        map.handles.remove(scope);
        map.retirements.remove(scope);
    }

    /// Look up an active terminal.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn get(&self, scope: &ResourceScopeKey) -> Option<Arc<TerminalHandle>> {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.handles.get(scope).cloned()
    }

    /// Read-only inspection of whether retirement admission is currently fenced.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[cfg(test)]
    #[must_use]
    pub fn is_retirement_fenced(&self, scope: &ResourceScopeKey) -> bool {
        let map = self.0.lock().expect("terminal registry poisoned");
        map.retirements
            .get(scope)
            .is_some_and(|retirement| retirement.fenced)
    }

    fn build_retirement_permit(
        scope: &ResourceScopeKey,
        map: &mut ActiveTerminalRegistryState,
    ) -> TerminalRetirementPermit {
        let instance = map
            .handles
            .get(scope)
            .map(TerminalInstanceIdentity::from_handle);
        let had_entry = instance.is_some();
        let generation = {
            let retirement = map.retirements.entry(scope.clone()).or_default();
            retirement.generation = retirement.generation.wrapping_add(1);
            retirement.fenced = true;
            retirement.generation
        };
        TerminalRetirementPermit {
            work_scope: scope.clone(),
            instance,
            generation: TerminalRetirementGeneration(generation),
            had_entry,
        }
    }

    /// Fence `scope` for exact retirement and return the permit authorizing one
    /// teardown attempt against the instance that was current at fence time.
    ///
    /// Admission stays closed until [`Self::reopen_after_repair`] clears the
    /// fence, even if `complete_retirement` later verifies exact absence.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn begin_retirement(&self, scope: &ResourceScopeKey) -> TerminalRetirementPermit {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        Self::build_retirement_permit(scope, &mut map)
    }

    fn matches_exact_instance(
        map: &ActiveTerminalRegistryState,
        permit: &TerminalRetirementPermit,
    ) -> bool {
        let Some(retirement) = map.retirements.get(&permit.work_scope) else {
            return false;
        };
        if !retirement.fenced || retirement.generation != permit.generation.0 {
            return false;
        }
        match (
            map.handles.get(&permit.work_scope),
            permit.instance.as_ref(),
        ) {
            (Some(current), Some(expected)) => expected.matches_handle(current),
            (None, None) => true,
            _ => false,
        }
    }

    fn verify_exact_absence(
        map: &ActiveTerminalRegistryState,
        permit: &TerminalRetirementPermit,
    ) -> TerminalRetirementOutcome {
        match (
            map.handles.get(&permit.work_scope),
            permit.instance.as_ref(),
        ) {
            (None, _) => TerminalRetirementOutcome::AbsenceVerified,
            (Some(current), Some(expected)) if !expected.matches_handle(current) => {
                TerminalRetirementOutcome::AbsenceVerified
            }
            (Some(_), Some(expected)) => TerminalRetirementOutcome::Residual {
                reason: format!(
                    "exact terminal instance {} remained current after teardown",
                    expected.stable_identity()
                ),
            },
            (Some(current), None) => TerminalRetirementOutcome::Residual {
                reason: format!(
                    "terminal {} appeared after retirement fenced an absent scope",
                    current.launch_identity.stable_identity()
                ),
            },
        }
    }

    /// Retire only the exact terminal instance authorized by `permit`.
    ///
    /// A stale permit must not remove a replacement terminal. On an exact match,
    /// this removes the registry-owned handle, signals relay teardown, and reaps
    /// the shell when no relay is attached. The scope remains fenced until
    /// [`Self::reopen_after_repair`] is called.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub async fn complete_retirement(
        &self,
        permit: &TerminalRetirementPermit,
    ) -> TerminalRetirementOutcome {
        let handle = {
            let map = self.0.lock().expect("terminal registry poisoned");
            if !Self::matches_exact_instance(&map, permit) {
                return Self::verify_exact_absence(&map, permit);
            }
            let Some(handle) = map.handles.get(&permit.work_scope) else {
                return Self::verify_exact_absence(&map, permit);
            };
            let handle = Arc::clone(handle);
            let _ = handle.stop_tx.send(StopReason::TearDown);
            handle
        };

        if handle.attach_permit.available_permits() == 0
            && tokio::time::timeout(
                std::time::Duration::from_secs(2),
                Arc::clone(&handle.attach_permit).acquire_owned(),
            )
            .await
            .is_err()
        {
            return TerminalRetirementOutcome::Residual {
                reason: "terminal relay did not release after teardown request".to_string(),
            };
        }
        let handle = {
            let mut map = self.0.lock().expect("terminal registry poisoned");
            if !Self::matches_exact_instance(&map, permit) {
                return Self::verify_exact_absence(&map, permit);
            }
            map.handles.remove(&permit.work_scope)
        };

        let Some(handle) = handle else {
            let map = self.0.lock().expect("terminal registry poisoned");
            return Self::verify_exact_absence(&map, permit);
        };

        let child_pid = handle.child_pid;
        let _ = handle.stop_tx.send(StopReason::TearDown);
        drop(handle);

        let wait_outcome =
            tokio::task::spawn_blocking(move || nix::sys::wait::waitpid(child_pid, None)).await;
        if wait_outcome.is_err() {
            return TerminalRetirementOutcome::Residual {
                reason: "terminal child exit observation failed".to_string(),
            };
        }
        let map = self.0.lock().expect("terminal registry poisoned");
        match Self::verify_exact_absence(&map, permit) {
            TerminalRetirementOutcome::AbsenceVerified if permit.had_entry => {
                TerminalRetirementOutcome::Retired
            }
            outcome @ (TerminalRetirementOutcome::Retired
            | TerminalRetirementOutcome::AbsenceVerified
            | TerminalRetirementOutcome::Residual { .. }) => outcome,
        }
    }

    /// Clear a retirement fence after repair authorizes the scope to admit a
    /// fresh terminal again.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub fn reopen_after_repair(&self, scope: &ResourceScopeKey) {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        map.retirements.remove(scope);
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
        work_scope: &ResourceScopeKey,
        inheritor_scope: Option<&ResourceScopeKey>,
    ) {
        if inheritor_scope == Some(work_scope) {
            tracing::debug!(
                work_scope = %work_scope,
                "terminal: skipping teardown — scope inherited by continuation"
            );
            return;
        }

        let permit = self.begin_retirement(work_scope);
        let outcome = self.complete_retirement(&permit).await;
        self.reopen_after_repair(work_scope);

        tracing::info!(
            work_scope = %work_scope,
            outcome = ?outcome,
            "terminal: cascade teardown complete"
        );
    }
}

/// Convenience function for the cleanup-cascade orchestrator. Mirrors
/// `cascade_tmux_on_delete` and `cascade_browser_on_delete`.
pub async fn cascade_terminal_on_delete(
    terminals: &ActiveTerminals,
    work_scope: &ResourceScopeKey,
    inheritor_scope: Option<&ResourceScopeKey>,
) {
    terminals
        .cascade_on_delete(work_scope, inheritor_scope)
        .await;
}
