//! Terminal session handle and active-session registry.

use nix::unistd::Pid;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::{Arc, Mutex};
use tokio::sync::{watch, Semaphore};

use super::command_tracker::CommandTracker;
use phoenix_core::{process_identity::ProcessIdentity, work_scope::ResourceScopeKey};

/// One outer liveness budget for terminal retirement during Close.
///
/// Relay authority release and child exit share the same absolute deadline, so
/// time spent waiting for the relay cannot reset the child-exit budget.
pub const TERMINAL_CLOSE_RETIREMENT_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(10);

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
    /// PTY master file descriptor. Closing this is the sole teardown trigger.
    ///
    /// The option lets retirement close the descriptor while retaining the
    /// exact handle in the registry until child exit is authoritatively
    /// observed. That retained handle is the retry owner if exit does not
    /// settle within the bounded wait.
    pub master_fd: Mutex<Option<OwnedFd>>,
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

impl TerminalHandle {
    /// Return the currently owned PTY master descriptor, if retirement has not
    /// already closed it.
    ///
    /// # Panics
    /// Panics if the descriptor mutex is poisoned.
    #[must_use]
    pub fn master_fd_raw(&self) -> Option<RawFd> {
        self.master_fd
            .lock()
            .expect("terminal master fd poisoned")
            .as_ref()
            .map(AsRawFd::as_raw_fd)
    }

    fn close_master_fd(&self) {
        self.master_fd
            .lock()
            .expect("terminal master fd poisoned")
            .take();
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

#[derive(Debug, PartialEq, Eq)]
pub struct TerminalRetirementPermit {
    pub work_scope: ResourceScopeKey,
    pub instance: Option<TerminalInstanceIdentity>,
    generation: TerminalRetirementGeneration,
    deadline: tokio::time::Instant,
    owner: TerminalRetirementOwner,
    had_entry: bool,
}

impl TerminalRetirementPermit {
    #[must_use]
    pub fn generation(&self) -> TerminalRetirementGeneration {
        self.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalRetirementOwner {
    Close,
    Relay,
}

/// Affine proof that a relay completed the retirement generation it claimed.
/// Consuming this outcome may reopen only that exact relay-owned fence.
#[derive(Debug, PartialEq, Eq)]
pub struct RelayRetirementCompletion {
    work_scope: ResourceScopeKey,
    generation: TerminalRetirementGeneration,
    owner: TerminalRetirementOwner,
    pub outcome: TerminalRetirementOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalRetirementOutcome {
    Retired,
    AbsenceVerified,
    Residual { reason: String },
}

/// Typed ownership decision made when an attached relay reaches a destructive
/// exit. Exactly one retirement owner is allowed to remove and reap the handle.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayTeardownOwnership {
    /// Close already fenced this exact instance and owns completion.
    ExistingRetirementOwner,
    /// No Close fence existed, so the relay initiated a bounded retirement that
    /// it may complete only after releasing its attach permit.
    RelayInitiated(TerminalRetirementPermit),
    /// The relay no longer corresponds to the registry's current instance.
    StaleRelay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTerminalInsertError {
    Occupied,
    RetirementFenced,
}

#[derive(Debug, Default)]
struct ActiveTerminalRegistryState {
    handles: HashMap<ResourceScopeKey, Arc<TerminalHandle>>,
    spawn_reservations: std::collections::HashSet<ResourceScopeKey>,
    retirements: HashMap<ResourceScopeKey, ScopeRetirementState>,
}

#[derive(Debug, Clone, Copy)]
struct ScopeRetirementState {
    generation: u64,
    owner: Option<TerminalRetirementOwner>,
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

    /// Reserve fresh-terminal admission before any PTY or tmux side effect.
    ///
    /// # Panics
    /// Panics if the terminal registry mutex is poisoned.
    ///
    /// # Errors
    /// Returns a typed rejection when the scope is occupied, already reserved,
    /// or fenced for retirement.
    pub fn reserve_spawn(&self, scope: &ResourceScopeKey) -> Result<(), ActiveTerminalInsertError> {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if map
            .retirements
            .get(scope)
            .is_some_and(|retirement| retirement.owner.is_some())
        {
            return Err(ActiveTerminalInsertError::RetirementFenced);
        }
        if map.handles.contains_key(scope) || !map.spawn_reservations.insert(scope.clone()) {
            return Err(ActiveTerminalInsertError::Occupied);
        }
        Ok(())
    }

    /// Publish a PTY under its previously reserved admission slot.
    ///
    /// # Panics
    /// Panics if the terminal registry mutex is poisoned.
    ///
    /// # Errors
    /// Returns a typed rejection if the reservation was revoked by retirement.
    pub fn insert_reserved(
        &self,
        scope: ResourceScopeKey,
        handle: TerminalHandle,
    ) -> Result<Arc<TerminalHandle>, ActiveTerminalInsertError> {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        let reserved = map.spawn_reservations.remove(&scope);
        if map
            .retirements
            .get(&scope)
            .is_some_and(|retirement| retirement.owner.is_some())
        {
            return Err(ActiveTerminalInsertError::RetirementFenced);
        }
        if !reserved || map.handles.contains_key(&scope) {
            return Err(ActiveTerminalInsertError::Occupied);
        }
        let handle = Arc::new(handle);
        map.handles.insert(scope, Arc::clone(&handle));
        Ok(handle)
    }

    /// Release a fresh-terminal reservation after planning or spawn failure.
    ///
    /// # Panics
    /// Panics if the terminal registry mutex is poisoned.
    pub fn release_spawn(&self, scope: &ResourceScopeKey) {
        self.0
            .lock()
            .expect("terminal registry poisoned")
            .spawn_reservations
            .remove(scope);
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
            .is_some_and(|retirement| retirement.owner.is_some())
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

    /// Transfer destructive relay cleanup to the registry's single retirement
    /// owner. The relay must release its handle and attach permit before
    /// completing a returned relay-initiated permit.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn claim_relay_teardown(
        &self,
        scope: &ResourceScopeKey,
        handle: &Arc<TerminalHandle>,
    ) -> RelayTeardownOwnership {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if !map
            .handles
            .get(scope)
            .is_some_and(|current| Arc::ptr_eq(current, handle))
        {
            return RelayTeardownOwnership::StaleRelay;
        }
        if map
            .retirements
            .get(scope)
            .is_some_and(|retirement| retirement.owner.is_some())
        {
            return RelayTeardownOwnership::ExistingRetirementOwner;
        }
        RelayTeardownOwnership::RelayInitiated(Self::build_retirement_permit(
            scope,
            &mut map,
            tokio::time::Instant::now() + TERMINAL_CLOSE_RETIREMENT_BUDGET,
            TerminalRetirementOwner::Relay,
        ))
    }

    /// Remove a terminal only when no retirement owner fences the scope.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[cfg(test)]
    pub(crate) fn remove_unfenced(&self, scope: &ResourceScopeKey) {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if map
            .retirements
            .get(scope)
            .is_none_or(|retirement| retirement.owner.is_none())
        {
            map.handles.remove(scope);
        }
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

    /// Returns the current handle only while relay attachment admission is open.
    ///
    /// # Panics
    /// Panics if the terminal registry mutex is poisoned.
    #[must_use]
    pub fn get_for_attach(&self, scope: &ResourceScopeKey) -> Option<Arc<TerminalHandle>> {
        let map = self.0.lock().expect("terminal registry poisoned");
        if map
            .retirements
            .get(scope)
            .is_some_and(|retirement| retirement.owner.is_some())
        {
            return None;
        }
        map.handles.get(scope).cloned()
    }

    /// Reset a reclaimed handle's stop channel only if it is still the current
    /// attachable instance. This makes a Close fence + `TearDown` atomic against
    /// relay setup, so setup cannot overwrite an already-issued teardown.
    ///
    /// # Panics
    /// Panics if the terminal registry mutex is poisoned.
    #[must_use]
    pub fn prepare_relay(&self, scope: &ResourceScopeKey, handle: &Arc<TerminalHandle>) -> bool {
        let map = self.0.lock().expect("terminal registry poisoned");
        if map
            .retirements
            .get(scope)
            .is_some_and(|retirement| retirement.owner.is_some())
            || !map
                .handles
                .get(scope)
                .is_some_and(|current| Arc::ptr_eq(current, handle))
        {
            return false;
        }
        let _ = handle.stop_tx.send(StopReason::Running);
        true
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
            .is_some_and(|retirement| retirement.owner.is_some())
    }

    fn build_retirement_permit(
        scope: &ResourceScopeKey,
        map: &mut ActiveTerminalRegistryState,
        deadline: tokio::time::Instant,
        owner: TerminalRetirementOwner,
    ) -> TerminalRetirementPermit {
        map.spawn_reservations.remove(scope);
        let instance = map
            .handles
            .get(scope)
            .map(TerminalInstanceIdentity::from_handle);
        let had_entry = instance.is_some();
        let generation = map
            .retirements
            .get(scope)
            .map_or(1, |retirement| retirement.generation.wrapping_add(1));
        map.retirements.insert(
            scope.clone(),
            ScopeRetirementState {
                generation,
                owner: Some(owner),
            },
        );
        TerminalRetirementPermit {
            work_scope: scope.clone(),
            instance,
            generation: TerminalRetirementGeneration(generation),
            deadline,
            owner,
            had_entry,
        }
    }

    /// Fence `scope` for exact retirement and return the permit authorizing one
    /// teardown attempt against the instance that was current at fence time.
    ///
    /// Admission stays closed until the exact permit is cancelled, even if
    /// `complete_retirement` later verifies exact absence.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn begin_retirement(&self, scope: &ResourceScopeKey) -> TerminalRetirementPermit {
        self.begin_retirement_by(
            scope,
            tokio::time::Instant::now() + TERMINAL_CLOSE_RETIREMENT_BUDGET,
        )
    }

    /// Fence retirement with an already-established absolute deadline.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn begin_retirement_by(
        &self,
        scope: &ResourceScopeKey,
        deadline: tokio::time::Instant,
    ) -> TerminalRetirementPermit {
        let mut map = self.0.lock().expect("terminal registry poisoned");
        Self::build_retirement_permit(scope, &mut map, deadline, TerminalRetirementOwner::Close)
    }

    fn matches_exact_instance(
        map: &ActiveTerminalRegistryState,
        permit: &TerminalRetirementPermit,
    ) -> bool {
        let Some(retirement) = map.retirements.get(&permit.work_scope) else {
            return false;
        };
        if retirement.generation != permit.generation.0 || retirement.owner != Some(permit.owner) {
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

    fn verify_exact_absence_by(
        map: &ActiveTerminalRegistryState,
        permit: &TerminalRetirementPermit,
        observe_process_identity: impl Fn(u32) -> Option<ProcessIdentity>,
    ) -> TerminalRetirementOutcome {
        match (
            map.handles.get(&permit.work_scope),
            permit.instance.as_ref(),
        ) {
            (None, Some(expected)) => {
                match observe_process_identity(expected.process_identity().pid) {
                    Some(current) if current == expected.process_identity() => {
                        TerminalRetirementOutcome::Residual {
                            reason: format!(
                                "exact terminal child {} remains live outside the registry",
                                expected.stable_identity()
                            ),
                        }
                    }
                    Some(_) => TerminalRetirementOutcome::AbsenceVerified,
                    None => TerminalRetirementOutcome::Residual {
                        reason: format!(
                            "exact terminal child {} absence could not be authoritatively observed",
                            expected.stable_identity()
                        ),
                    },
                }
            }
            (None, None) => TerminalRetirementOutcome::AbsenceVerified,
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
    /// the shell when no relay is attached. The scope remains fenced until the
    /// exact owning permit or relay completion is consumed.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub async fn complete_retirement(
        &self,
        permit: &TerminalRetirementPermit,
    ) -> TerminalRetirementOutcome {
        self.complete_retirement_by(permit, permit.deadline).await
    }

    pub(crate) async fn complete_retirement_by(
        &self,
        permit: &TerminalRetirementPermit,
        deadline: tokio::time::Instant,
    ) -> TerminalRetirementOutcome {
        self.complete_retirement_by_observing(
            permit,
            deadline,
            wait_for_child_exit,
            phoenix_core::process_identity::current_process_identity,
        )
        .await
    }

    pub(crate) async fn complete_retirement_by_observing<Wait, WaitFuture, Observe>(
        &self,
        permit: &TerminalRetirementPermit,
        deadline: tokio::time::Instant,
        wait_for_exit: Wait,
        observe_process_identity: Observe,
    ) -> TerminalRetirementOutcome
    where
        Wait: FnOnce(Pid) -> WaitFuture,
        WaitFuture: std::future::Future<Output = nix::Result<nix::sys::wait::WaitStatus>>,
        Observe: Fn(u32) -> Option<ProcessIdentity> + Copy,
    {
        let attach_permit = {
            let map = self.0.lock().expect("terminal registry poisoned");
            if !Self::matches_exact_instance(&map, permit) {
                return Self::verify_exact_absence_by(&map, permit, observe_process_identity);
            }
            let Some(handle) = map.handles.get(&permit.work_scope) else {
                return Self::verify_exact_absence_by(&map, permit, observe_process_identity);
            };
            handle.stop_tx.send_replace(StopReason::TearDown);
            Arc::clone(&handle.attach_permit)
        };

        // An attached relay remains the teardown authority: do not retain an
        // Arc that would keep its PTY master open while it waits and reaps. A
        // detached terminal has no relay, so retirement acquires this authority
        // immediately and actively closes the exact current handle's master.
        let Ok(Ok(relay_authority)) =
            tokio::time::timeout_at(deadline, attach_permit.acquire_owned()).await
        else {
            return TerminalRetirementOutcome::Residual {
                reason: "terminal relay did not release after teardown request".to_string(),
            };
        };
        let handle = {
            let map = self.0.lock().expect("terminal registry poisoned");
            if !Self::matches_exact_instance(&map, permit) {
                let outcome = Self::verify_exact_absence_by(&map, permit, observe_process_identity);
                drop(relay_authority);
                return match outcome {
                    TerminalRetirementOutcome::AbsenceVerified if permit.had_entry => {
                        TerminalRetirementOutcome::Retired
                    }
                    outcome @ (TerminalRetirementOutcome::Retired
                    | TerminalRetirementOutcome::AbsenceVerified
                    | TerminalRetirementOutcome::Residual { .. }) => outcome,
                };
            }
            let Some(handle) = map.handles.get(&permit.work_scope) else {
                unreachable!("exact terminal match without a current handle")
            };
            Arc::clone(handle)
        };
        let child_pid = handle.child_pid;
        handle.close_master_fd();

        // SIGCHLD is the causal child-exit signal. The timeout is only the
        // outer Close liveness bound, and it shares the relay deadline above.
        let wait_outcome = tokio::time::timeout_at(deadline, wait_for_exit(child_pid)).await;
        if !matches!(wait_outcome, Ok(Ok(_))) {
            let expected = permit
                .instance
                .as_ref()
                .expect("exact terminal match must carry process identity")
                .process_identity();
            match observe_process_identity(expected.pid) {
                Some(current) if current != expected => {}
                Some(_) | None => {
                    return TerminalRetirementOutcome::Residual {
                        reason: "terminal child exit was not authoritatively observed; exact handle retained for retry"
                            .to_string(),
                    };
                }
            }
        }
        drop(relay_authority);
        {
            let mut map = self.0.lock().expect("terminal registry poisoned");
            if !Self::matches_exact_instance(&map, permit) {
                return Self::verify_exact_absence_by(&map, permit, observe_process_identity);
            }
            map.handles.remove(&permit.work_scope);
        }
        drop(handle);
        if permit.had_entry {
            TerminalRetirementOutcome::Retired
        } else {
            TerminalRetirementOutcome::AbsenceVerified
        }
    }

    /// Complete a relay-owned retirement and return an affine reopening outcome.
    pub async fn complete_relay_retirement(
        &self,
        permit: TerminalRetirementPermit,
    ) -> RelayRetirementCompletion {
        let outcome = self.complete_retirement(&permit).await;
        RelayRetirementCompletion {
            work_scope: permit.work_scope,
            generation: permit.generation,
            owner: permit.owner,
            outcome,
        }
    }

    /// Consume a relay completion and reopen only its exact current relay fence.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    #[must_use]
    pub fn reopen_after_relay_completion(
        &self,
        completion: RelayRetirementCompletion,
    ) -> TerminalRetirementOutcome {
        let RelayRetirementCompletion {
            work_scope,
            generation,
            owner,
            outcome,
        } = completion;
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if let Some(retirement) = map.retirements.get_mut(&work_scope) {
            if retirement.generation == generation.0
                && retirement.owner == Some(TerminalRetirementOwner::Relay)
                && owner == TerminalRetirementOwner::Relay
            {
                retirement.owner = None;
            }
        }
        outcome
    }

    /// Consume an uncommitted Close permit and cancel only its exact fence.
    ///
    /// # Panics
    /// Panics if the registry mutex is poisoned.
    pub fn cancel_retirement(&self, permit: TerminalRetirementPermit) {
        let TerminalRetirementPermit {
            work_scope,
            generation,
            owner,
            ..
        } = permit;
        let mut map = self.0.lock().expect("terminal registry poisoned");
        if let Some(retirement) = map.retirements.get_mut(&work_scope) {
            if retirement.generation == generation.0
                && retirement.owner == Some(TerminalRetirementOwner::Close)
                && owner == TerminalRetirementOwner::Close
            {
                retirement.owner = None;
            }
        }
    }

    /// Cascade-cleanup entry for `run_resource_cleanup_cascade`
    /// (REQ-TERM-WS-001, REQ-TERM-012). Mirrors `TmuxRegistry::cascade_on_delete`
    /// and `BrowserSessionManager::cascade_on_delete`:
    ///
    ///   - If `inheritor_scope == Some(work_scope)`, the continuation
    ///     conversation resolves to the same scope and still owns the
    ///     terminal. Skip teardown.
    ///   - Otherwise, fence the registry entry, signal any attached relay
    ///     to tear down via `StopReason::TearDown`, acquire teardown authority
    ///     after the relay releases, and reap the exact child within the shared
    ///     retirement deadline.
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
        self.cancel_retirement(permit);

        tracing::info!(
            work_scope = %work_scope,
            outcome = ?outcome,
            "terminal: cascade teardown complete"
        );
    }
}

#[cfg(unix)]
fn terminal_child_exit(status: nix::sys::wait::WaitStatus) -> Option<nix::sys::wait::WaitStatus> {
    use nix::sys::wait::WaitStatus;

    match status {
        outcome @ (WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => Some(outcome),
        WaitStatus::Stopped(..) | WaitStatus::Continued(_) | WaitStatus::StillAlive => None,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        WaitStatus::PtraceEvent(..) | WaitStatus::PtraceSyscall(_) => None,
    }
}

#[cfg(unix)]
async fn wait_for_child_exit(child_pid: Pid) -> nix::Result<nix::sys::wait::WaitStatus> {
    use nix::sys::wait::{waitpid, WaitPidFlag};

    // Register for SIGCHLD before the first observation so an exit between the
    // check and wait remains visible either as a pending signal or a zombie.
    let mut child_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())
        .map_err(|error| nix::Error::from_raw(error.raw_os_error().unwrap_or(libc::EIO)))?;
    loop {
        let status = waitpid(child_pid, Some(WaitPidFlag::WNOHANG))?;
        if let Some(outcome) = terminal_child_exit(status) {
            return Ok(outcome);
        }
        let _ = child_signal.recv().await;
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_wait_status_tests {
    use super::terminal_child_exit;
    use nix::{
        sys::{signal::Signal, wait::WaitStatus},
        unistd::Pid,
    };

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
