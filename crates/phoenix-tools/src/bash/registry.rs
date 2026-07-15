//! In-memory bash handle registry.
//!
//! REQ-BASH-005 (per-`WorkScope` cap), REQ-BASH-006 (in-memory tombstones,
//! no `SQLite` shadow store), REQ-BASH-014 / REQ-BASH-WS-001 / REQ-BASH-WS-002
//! (per-`WorkScope` registry — a continuation chain on one worktree shares
//! its handle table because it resolves to the same `WorkScope`).
//!
//! Lifetime: registries live in process memory only. A Phoenix restart
//! drops them and any handles they hold; agents see `handle_not_found` on
//! a previously-known handle (matching the spec's "handles do NOT survive
//! Phoenix restart" guarantee).
//!
//! Lock ordering for cap enforcement and spawn (consumed by `BashTool::run`):
//! acquire the registry's `RwLock<HashMap>` for read, then the
//! `WorkScope`'s `RwLock<WorkScopeHandles>` for write. The per-scope lock
//! holds for the duration of cap-check + handle insert to prevent two
//! concurrent spawns from both observing `count == cap - 1` and racing past
//! the cap.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use phoenix_core::work_scope::WorkScope;
use thiserror::Error;
use tokio::sync::RwLock;

use super::handle::{Handle, HandleId};
use super::ring::RING_BUFFER_BYTES;

/// Per-`WorkScope` cap on `running` handles (REQ-BASH-005:
/// `LIVE_HANDLE_CAP`).
pub const LIVE_HANDLE_CAP: usize = 8;

/// Errors surfaced by the registry. `BashTool` translates these into the
/// stable error envelope on the agent's response.
#[derive(Debug, Error)]
pub enum BashHandleError {
    /// REQ-BASH-005: spawn rejected because the work scope has hit
    /// `LIVE_HANDLE_CAP` live handles.
    #[error("this work scope has reached the cap of {cap} live bash handles")]
    HandleCapReached {
        cap: usize,
        live_handles: Vec<LiveHandleSummary>,
    },
}

/// Summary of a live handle for the cap-rejection response (REQ-BASH-005).
#[derive(Debug, Clone)]
pub struct LiveHandleSummary {
    pub handle: HandleId,
    pub cmd: String,
    pub label: Option<String>,
    pub age_seconds: u64,
}

/// Identity needed to attribute one live handle's process group in shared
/// resource-observation projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveHandleProcessGroup {
    pub work_scope: WorkScope,
    pub handle_id: HandleId,
    pub pgid: i32,
}

/// Per-`WorkScope` handle table. Tracks live handles (for cap enforcement
/// and lookup) and tombstones (so peek/wait/kill on an exited handle still
/// resolves until the scope is hard-deleted with no inheritor or Phoenix
/// restarts).
///
/// The unified `handles` map covers both live and tombstoned entries;
/// the discrimination is made by inspecting the handle's `HandleState`.
/// This keeps the lookup path single-source — a handle that transitions
/// from `Live` to `Tombstoned` is the SAME `Arc<Handle>` (its `state`
/// field swaps), and lookup never has to "follow" between two maps.
#[derive(Debug, Default)]
pub struct WorkScopeHandles {
    /// Next sequential handle index for this work scope (`b-1`, `b-2`, ...).
    next_id: u64,
    /// All handles, by id. Live and tombstoned alike.
    handles: HashMap<HandleId, Arc<Handle>>,
}

impl WorkScopeHandles {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the next handle id and increment the counter. Format:
    /// `b-N` where N starts at 1.
    pub fn allocate_handle_id(&mut self) -> HandleId {
        self.next_id += 1;
        HandleId::new(format!("b-{}", self.next_id))
    }

    /// Look up a handle by id (live or tombstoned).
    #[must_use]
    pub fn get(&self, id: &HandleId) -> Option<Arc<Handle>> {
        self.handles.get(id).cloned()
    }

    /// All currently registered handles.
    pub fn all(&self) -> impl Iterator<Item = &Arc<Handle>> {
        self.handles.values()
    }

    /// Number of live handles (status: `running` or `kill_pending_kernel`).
    /// Both share the `Live` representation; tombstoned handles do not count.
    ///
    /// Async because counting requires reading each handle's state lock.
    pub async fn live_count(&self) -> usize {
        let mut n = 0;
        for h in self.handles.values() {
            if h.state().await.is_live() {
                n += 1;
            }
        }
        n
    }

    /// Compute the live-handle summary used for cap-rejection responses.
    pub async fn live_summary(&self) -> Vec<LiveHandleSummary> {
        let mut out = Vec::new();
        let now = SystemTime::now();
        for h in self.handles.values() {
            if h.state().await.is_live() {
                let age_seconds = now
                    .duration_since(h.started_at)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                out.push(LiveHandleSummary {
                    handle: h.handle_id.clone(),
                    cmd: h.cmd.clone(),
                    label: h.label.clone(),
                    age_seconds,
                });
            }
        }
        out
    }

    /// Insert a freshly constructed live handle. Caller is responsible
    /// for cap enforcement via [`Self::check_cap`] BEFORE constructing
    /// the OS process. Returns the inserted handle for chaining.
    pub fn insert(&mut self, handle: Arc<Handle>) -> Arc<Handle> {
        self.handles
            .insert(handle.handle_id.clone(), handle.clone());
        handle
    }

    /// Remove a handle entirely (live or tombstoned). Granular complement
    /// to `BashHandleRegistry::remove`; not currently used by the
    /// hard-delete cascade (which removes the whole `WorkScope` table) but
    /// kept on the API surface for surgical removal flows.
    #[allow(dead_code)]
    pub fn remove(&mut self, id: &HandleId) -> Option<Arc<Handle>> {
        self.handles.remove(id)
    }

    /// REQ-BASH-005: enforce the cap before allocating a new handle id /
    /// spawning a process.
    ///
    /// # Errors
    /// Returns [`BashHandleError::HandleCapReached`] (populated with the
    /// current live-handle summary so the agent can decide what to kill or
    /// wait on) when the cap is reached.
    pub async fn check_cap(&self, cap: usize) -> Result<(), BashHandleError> {
        if self.live_count().await >= cap {
            Err(BashHandleError::HandleCapReached {
                cap,
                live_handles: self.live_summary().await,
            })
        } else {
            Ok(())
        }
    }
}

/// Signal published when a bash handle in a `WorkScope` changes state
/// (spawned, transitioned to terminal, or killed). Mirrors the browser
/// `BrowserSessionLifecycleEvent` shape: it carries only the affected
/// `WorkScope`, leaving inventory assembly and conversation routing to the
/// runtime's work-scope bridge. State transitions only — NOT per output line
/// (REQ-WSUI-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashLifecyclePhase {
    Spawned,
    KillPendingKernel,
    Terminal,
}

impl BashLifecyclePhase {
    #[must_use]
    pub fn schedules_reconciliation(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

#[derive(Debug, Clone)]
pub struct BashLifecycleEvent {
    pub work_scope: WorkScope,
    pub phase: BashLifecyclePhase,
}

/// Sink the registry publishes [`BashLifecycleEvent`]s into. A bounded `mpsc`
/// keeps the registry decoupled from per-conversation routing (the runtime
/// owns that). `None` for tool-level tests that don't care about the push
/// path.
pub type BashLifecycleSink = tokio::sync::mpsc::UnboundedSender<BashLifecycleEvent>;

/// Top-level registry: maps `WorkScope` -> per-`WorkScope` handle table.
///
/// One registry instance per Phoenix process. Owned by the runtime layer
/// and reached by tools through `ToolContext::bash_handles()`. Keying by
/// `WorkScope` (rather than `conversation_id`) is what lets a continuation
/// chain on one worktree share its handle table — both members resolve to
/// the same `WorkScope::Worktree(path)` (REQ-BASH-WS-001).
#[derive(Debug, Default)]
pub struct BashHandleRegistry {
    inner: RwLock<HashMap<WorkScope, Arc<RwLock<WorkScopeHandles>>>>,
    /// Per-handle ring byte cap. Defaults to [`RING_BUFFER_BYTES`]; tests
    /// override to small values to exercise eviction.
    ring_bytes_cap: usize,
    /// Per-`WorkScope` live-handle cap. Defaults to [`LIVE_HANDLE_CAP`];
    /// tests override to small values to exercise rejection.
    live_handle_cap: usize,
    /// Optional sink for bash state-transition signals (spawn / terminal /
    /// kill). Populated by `RuntimeManager::new` so transitions flow into the
    /// work-scope push bridge; `None` for tool-level tests. Mirrors
    /// `BrowserSessionManager::lifecycle_sink`.
    lifecycle_sink: Option<BashLifecycleSink>,
}

impl BashHandleRegistry {
    /// Create a registry with default caps and no lifecycle sink.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            ring_bytes_cap: RING_BUFFER_BYTES,
            live_handle_cap: LIVE_HANDLE_CAP,
            lifecycle_sink: None,
        }
    }

    /// Create a registry that publishes bash state-transition signals into
    /// `sink`. The runtime wires this to the work-scope push bridge, which
    /// resolves the scope's conversation and broadcasts a `WorkScopeUpdate`.
    /// Mirrors `BrowserSessionManager::with_lifecycle_sink`.
    #[must_use]
    pub fn with_lifecycle_sink(sink: Option<BashLifecycleSink>) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            ring_bytes_cap: RING_BUFFER_BYTES,
            live_handle_cap: LIVE_HANDLE_CAP,
            lifecycle_sink: sink,
        }
    }

    /// Test-only: build a registry with custom caps.
    #[cfg(test)]
    #[must_use]
    pub fn with_caps(ring_bytes_cap: usize, live_handle_cap: usize) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            ring_bytes_cap,
            live_handle_cap,
            lifecycle_sink: None,
        }
    }

    /// Publish a bash state-transition signal for `work_scope` if a sink is
    /// wired. Best-effort: a dropped receiver / closed channel is logged at
    /// `debug` (capability gap) and does not affect handle correctness.
    /// Mirrors `BrowserSessionManager::emit_lifecycle`.
    pub fn emit_lifecycle(&self, work_scope: &WorkScope, phase: BashLifecyclePhase) {
        let Some(sink) = self.lifecycle_sink.as_ref() else {
            return;
        };
        let event = BashLifecycleEvent {
            work_scope: work_scope.clone(),
            phase,
        };
        if let Err(e) = sink.send(event) {
            tracing::debug!(
                work_scope = %work_scope,
                error = %e,
                "dropping bash lifecycle event — sink closed"
            );
        }
    }

    /// Clone the lifecycle sink, if any. Used to hand the sink to the
    /// detached waiter task so the transition-to-terminal edge (which fires
    /// off-thread, long after the spawning tool call returns) can emit.
    #[must_use]
    pub fn lifecycle_sink(&self) -> Option<BashLifecycleSink> {
        self.lifecycle_sink.clone()
    }

    /// Configured ring byte cap for live handles in this registry.
    pub fn ring_bytes_cap(&self) -> usize {
        self.ring_bytes_cap
    }

    /// Configured live-handle cap.
    pub fn live_handle_cap(&self) -> usize {
        self.live_handle_cap
    }

    /// Get-or-create the per-`WorkScope` handle table. Matches the
    /// `BrowserSessionManager::get_session` pattern — returns the same
    /// `Arc<RwLock<WorkScopeHandles>>` for repeated calls with the
    /// same `WorkScope`, so a continuation chain on one worktree shares
    /// one table (REQ-BASH-WS-001).
    pub async fn get_or_create(&self, work_scope: &WorkScope) -> Arc<RwLock<WorkScopeHandles>> {
        // Fast path: read-lock and return existing entry.
        {
            let map = self.inner.read().await;
            if let Some(entry) = map.get(work_scope) {
                return entry.clone();
            }
        }
        // Slow path: write-lock to create. Re-check under the lock to
        // avoid clobbering a concurrent creator.
        let mut map = self.inner.write().await;
        if let Some(entry) = map.get(work_scope) {
            return entry.clone();
        }
        let entry = Arc::new(RwLock::new(WorkScopeHandles::new()));
        map.insert(work_scope.clone(), entry.clone());
        entry
    }

    /// Move a `WorkScope`'s handle table from `old` to `new`.
    ///
    /// Used at an Explore→Work approval, where the conversation's scope flips
    /// from `WorkScope::Conversation(id)` to `WorkScope::Worktree(path)`: bash
    /// handles opened pre-approval are stored under `old` and must follow the
    /// scope so the inventory and idle/cleanup paths resolve them under `new`.
    /// The underlying processes are untouched — only the lookup key moves.
    ///
    /// Returns `true` if an entry was moved. No-ops (returns `false`) when:
    /// - `old == new` (nothing to do), or
    /// - there is no entry under `old`, or
    /// - `new` is already occupied — in that case the pre-existing `new` entry
    ///   is preserved and the `old` entry is left in place (NOT clobbered), and
    ///   the collision is logged at WARN. At approval `new` is a freshly created
    ///   worktree scope, so occupancy is not expected.
    pub async fn rekey_scope(&self, old: &WorkScope, new: &WorkScope) -> bool {
        if old == new {
            return false;
        }
        let mut map = self.inner.write().await;
        if map.contains_key(new) {
            if map.contains_key(old) {
                tracing::warn!(
                    old = %old,
                    new = %new,
                    "bash: refusing to rekey handle table — destination scope already occupied; leaving both entries in place"
                );
            }
            return false;
        }
        let Some(entry) = map.remove(old) else {
            return false;
        };
        map.insert(new.clone(), entry);
        true
    }

    /// Look up a `WorkScope`'s handle table **without creating one**.
    ///
    /// Read-only counterpart to [`Self::get_or_create`], for observability
    /// surfaces (the work-scope inventory endpoint) that must reflect the
    /// registry as-is and must not allocate a table for a scope that has
    /// never spawned a bash handle.
    pub async fn get_existing(
        &self,
        work_scope: &WorkScope,
    ) -> Option<Arc<RwLock<WorkScopeHandles>>> {
        self.inner.read().await.get(work_scope).cloned()
    }

    /// Snapshot live process-group ids across ALL work scopes, for the
    /// shutdown kill-tree pass. Acquires read locks; callers must NOT
    /// hold any per-scope lock while invoking this.
    ///
    /// REQ-BASH-007: walks live handles for the `shutdown_kill_tree` pass.
    pub async fn snapshot_live_pgids(&self) -> Vec<i32> {
        let mut out = Vec::new();
        let map = self.inner.read().await;
        for entry in map.values() {
            let scope_handles = entry.read().await;
            for h in scope_handles.all() {
                if let Some(pgid) = h.live_pgid().await {
                    out.push(pgid);
                }
            }
        }
        out
    }

    /// Snapshot live process groups together with their authoritative scope and
    /// handle identity. Read-only and non-creating.
    pub async fn snapshot_live_process_groups(&self) -> Vec<LiveHandleProcessGroup> {
        let mut out = Vec::new();
        let map = self.inner.read().await;
        for (work_scope, entry) in map.iter() {
            let scope_handles = entry.read().await;
            for handle in scope_handles.all() {
                if let Some(pgid) = handle.live_pgid().await {
                    out.push(LiveHandleProcessGroup {
                        work_scope: work_scope.clone(),
                        handle_id: handle.handle_id.clone(),
                        pgid,
                    });
                }
            }
        }
        out
    }

    /// Remove a `WorkScope`'s handle table outright. Used by the
    /// hard-delete cascade (REQ-BASH-006). Returns the removed entry so
    /// the caller can SIGKILL its live process groups synchronously.
    pub async fn remove(&self, work_scope: &WorkScope) -> Option<Arc<RwLock<WorkScopeHandles>>> {
        let mut map = self.inner.write().await;
        map.remove(work_scope)
    }

    /// Number of work scopes currently tracked. Test/diagnostic only.
    #[cfg(test)]
    pub async fn scope_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

/// Best-effort cascade outcome for the hard-delete orchestrator
/// (REQ-BASH-006). Failure surfaces as a structured record the
/// orchestrator logs at WARN; nothing here is fatal — the conversation
/// row is removed regardless. The orchestrator already knows the
/// `WorkScope` (it's an argument), so it is not duplicated here.
#[derive(Debug, Clone, Default)]
pub struct CascadeBashReport {
    /// pids that were live at snapshot time (informational; kills target
    /// the pgid). One per live handle.
    pub live_handle_pids: Vec<i32>,
    /// pgids that were live at snapshot time and received a SIGKILL.
    pub live_handle_pgids: Vec<i32>,
    /// Subset of live pgids whose handle was in `kill_pending_kernel`
    /// status when the cascade ran (a prior kill had not been observed
    /// to land yet). Surfaced separately because these are the most
    /// likely D-state offenders for an operator chasing orphans.
    pub kill_pending_kernel_pids: Vec<i32>,
    /// Per-pgid kill failures (`kill(2)` returned non-zero). Successful
    /// kills and `ESRCH` (process already gone) do not appear here.
    pub kill_failures: Vec<(i32, String)>,
}

/// Run the bash side of the hard-delete cascade for `work_scope`
/// (REQ-BASH-006, REQ-BASH-WS-002). Mirrors the tmux/browser cascades'
/// `(work_scope, inheritor_scope)` signature.
///
/// A continuation that inherits the same `WorkScope` keeps the live
/// processes and tombstones — they belong to the `WorkScope`, not the
/// deleted conversation. When `inheritor_scope == Some(work_scope)` the
/// teardown is skipped entirely and the handle table is left in place
/// (early return, empty report).
///
/// Otherwise, atomically:
///
///   1. Removes the scope's handle table from the registry — any
///      subsequent tool call for this `WorkScope` will see "no handle
///      table" and produce `handle_not_found`, which is the correct
///      behaviour once no inheritor survives.
///   2. Snapshots live pgid / pid / `kill_pending_kernel` state across the
///      removed handles.
///   3. Sends `SIGKILL` to each live process group via
///      `kill(-pgid, SIGKILL)` (catches immediate descendants per
///      REQ-BASH-007's setpgid contract).
///
/// Step 1 alone drops the live handles with the registry entry. Step 3
/// satisfies the `KillSignalSentForAllLiveHandles` ensures clause; failures
/// populate `kill_failures` for the orchestrator's WARN log but are not
/// fatal — the spec's policy is "log and continue" (REQ-BED-032).
///
/// SIGKILL rather than SIGTERM: hard-delete deletes the conversation
/// outright, so no agent is left to observe a graceful close. Same
/// rationale as `shutdown_kill_tree` in [`super::reaper`].
pub async fn cascade_bash_on_delete(
    registry: &Arc<BashHandleRegistry>,
    work_scope: &WorkScope,
    inheritor_scope: Option<&WorkScope>,
) -> CascadeBashReport {
    let mut report = CascadeBashReport::default();

    // A continuation inheriting the same scope keeps the live processes and
    // tombstones — they belong to the WorkScope, not the deleted conversation.
    if inheritor_scope == Some(work_scope) {
        tracing::debug!(
            work_scope = %work_scope,
            "bash: skipping handle teardown — scope inherited by continuation"
        );
        return report;
    }

    let Some(entry) = registry.remove(work_scope).await else {
        return report;
    };

    {
        let scope_handles = entry.read().await;
        for h in scope_handles.all() {
            let Some(group_id) = h.live_pgid().await else {
                continue;
            };
            let process_id = h.live_pid().await;
            let kill_pending = h.is_kill_pending_kernel().await;
            record_handle_in_report(&mut report, group_id, process_id, kill_pending);

            #[cfg(unix)]
            {
                // SAFETY: kill(2) with negative pid signals the process group;
                // no memory implications. ESRCH (group already gone) is
                // expected and is not surfaced as an error.
                let rc = unsafe { libc::kill(-group_id, libc::SIGKILL) };
                if rc != 0 {
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() != Some(libc::ESRCH) {
                        report.kill_failures.push((group_id, err.to_string()));
                    }
                }
            }
        }
    }

    // The handle table was actually removed (and any live process groups
    // SIGKILL'd), so the scope's inventory is now empty. Publish a lifecycle
    // edge so the work-scope bridge re-broadcasts the refreshed (empty)
    // inventory — without it, a scope with no tmux/browser change to drive
    // the bridge would leave the collapsed work-scope badge showing the
    // killed handles (REQ-WSUI-007). NOT emitted on the preserved early
    // return above, where nothing changed.
    registry.emit_lifecycle(work_scope, BashLifecyclePhase::Terminal);

    report
}

/// Record one handle's pgid/pid/kill-pending state into the cascade
/// report. Factored out of [`cascade_bash_on_delete`] so the cast-width
/// allow attributes don't pollute the loop body. pgid/pid are spec
/// names from `bash.allium`'s `Handle` entity.
#[allow(clippy::cast_possible_wrap, clippy::similar_names)]
fn record_handle_in_report(
    report: &mut CascadeBashReport,
    pgid: i32,
    pid: Option<u32>,
    kill_pending: bool,
) {
    report.live_handle_pgids.push(pgid);
    if let Some(p) = pid {
        report.live_handle_pids.push(p as i32);
    }
    if kill_pending {
        if let Some(p) = pid {
            report.kill_pending_kernel_pids.push(p as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::handle::{FinalCause, Handle};

    fn scope(name: &str) -> WorkScope {
        WorkScope::Conversation(name.to_string())
    }

    fn make_handle(scope_name: &str, id: &str, ring_bytes_cap: usize) -> Arc<Handle> {
        Handle::new_live(
            scope(scope_name),
            HandleId::new(id),
            format!("cmd for {id}"),
            None,
            12345,
            12345,
            ring_bytes_cap,
        )
    }

    /// REQ-WSUI-007: `emit_lifecycle` publishes a `BashLifecycleEvent`
    /// carrying the affected scope when a sink is wired, and is a no-op
    /// (no panic, no phantom event) when it is not. Mirrors the browser
    /// manager's `emit_lifecycle_round_trips_through_sink`.
    #[tokio::test]
    async fn emit_lifecycle_round_trips_through_sink() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = BashHandleRegistry::with_lifecycle_sink(Some(tx));
        let a = scope("conv-A");
        let b = scope("conv-B");
        registry.emit_lifecycle(&a, BashLifecyclePhase::Spawned);
        registry.emit_lifecycle(&b, BashLifecyclePhase::Terminal);

        let e1 = rx.try_recv().expect("first event missing");
        assert_eq!(e1.work_scope, a);
        assert_eq!(e1.phase, BashLifecyclePhase::Spawned);
        let e2 = rx.try_recv().expect("second event missing");
        assert_eq!(e2.work_scope, b);
        assert_eq!(e2.phase, BashLifecyclePhase::Terminal);
        assert!(rx.try_recv().is_err(), "no more events expected");
    }

    #[tokio::test]
    async fn emit_lifecycle_without_sink_is_no_op() {
        let registry = BashHandleRegistry::new();
        registry.emit_lifecycle(&scope("conv-X"), BashLifecyclePhase::Spawned);
        assert!(registry.lifecycle_sink().is_none());
    }

    #[tokio::test]
    async fn allocate_handle_id_is_sequential_per_scope() {
        let mut handles = WorkScopeHandles::new();
        assert_eq!(handles.allocate_handle_id().as_str(), "b-1");
        assert_eq!(handles.allocate_handle_id().as_str(), "b-2");
        assert_eq!(handles.allocate_handle_id().as_str(), "b-3");
    }

    #[tokio::test]
    async fn allocate_handle_id_independent_across_scopes() {
        let registry = BashHandleRegistry::new();
        let scope_a = registry.get_or_create(&scope("conv-a")).await;
        let scope_b = registry.get_or_create(&scope("conv-b")).await;
        assert_eq!(scope_a.write().await.allocate_handle_id().as_str(), "b-1");
        assert_eq!(scope_b.write().await.allocate_handle_id().as_str(), "b-1");
        assert_eq!(scope_a.write().await.allocate_handle_id().as_str(), "b-2");
    }

    #[tokio::test]
    async fn cap_rejects_when_live_count_at_cap() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 2);
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let mut guard = handles_arc.write().await;
        // Insert two live handles — at the cap.
        guard.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));
        guard.insert(make_handle("conv-1", "b-2", RING_BUFFER_BYTES));
        // Now check_cap must reject.
        let err = guard.check_cap(2).await.unwrap_err();
        match err {
            BashHandleError::HandleCapReached { cap, live_handles } => {
                assert_eq!(cap, 2);
                assert_eq!(live_handles.len(), 2);
                let ids: Vec<&str> = live_handles
                    .iter()
                    .map(|s| s.handle.as_str())
                    .collect::<Vec<_>>();
                assert!(ids.contains(&"b-1") && ids.contains(&"b-2"));
            }
        }
    }

    #[tokio::test]
    async fn cap_rejection_includes_cmd_and_age() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 1);
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let mut guard = handles_arc.write().await;
        guard.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));
        let err = guard.check_cap(1).await.unwrap_err();
        let BashHandleError::HandleCapReached { live_handles, .. } = err;
        assert_eq!(live_handles[0].cmd, "cmd for b-1");
        // age is recent; just assert it's a u64 (>= 0).
        let _ = live_handles[0].age_seconds;
    }

    #[tokio::test]
    async fn tombstoned_handle_does_not_count_against_cap() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 1);
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let mut guard = handles_arc.write().await;
        let h = guard.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));

        // Cap is 1; live_count == 1 right now → reject.
        assert!(guard.check_cap(1).await.is_err());

        // Demote the handle to tombstoned (process exited).
        let did_transition = h
            .transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                std::time::Duration::from_millis(1),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
        assert!(did_transition);

        // Live count is now 0; cap allows another spawn.
        assert!(guard.check_cap(1).await.is_ok());
        assert_eq!(guard.live_count().await, 0);
        // The tombstoned handle is still resolvable.
        assert!(guard.get(&HandleId::new("b-1")).is_some());
    }

    #[tokio::test]
    async fn check_cap_passes_below_cap() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 8);
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let guard = handles_arc.read().await;
        // Empty scope: live_count = 0; cap = 8 → ok.
        assert!(guard.check_cap(8).await.is_ok());
    }

    #[tokio::test]
    async fn get_or_create_returns_same_arc_for_same_scope() {
        let registry = BashHandleRegistry::new();
        let a = registry.get_or_create(&scope("conv-1")).await;
        let b = registry.get_or_create(&scope("conv-1")).await;
        assert!(Arc::ptr_eq(&a, &b));
    }

    /// A worktree-backed continuation chain shares one handle table: both
    /// members resolve to the same `WorkScope::Worktree(path)`, so
    /// `get_or_create` hands back the same `Arc` (REQ-BASH-WS-001).
    #[tokio::test]
    async fn get_or_create_shares_table_across_worktree_scope() {
        let registry = BashHandleRegistry::new();
        let wt = WorkScope::Worktree("/tmp/wt-shared".to_string());
        let a = registry.get_or_create(&wt).await;
        let b = registry.get_or_create(&wt).await;
        assert!(Arc::ptr_eq(&a, &b));
        // A different scope (Conversation with same inner string) is a
        // disjoint table — no leakage across the namespace boundary.
        let conv = WorkScope::Conversation("/tmp/wt-shared".to_string());
        let c = registry.get_or_create(&conv).await;
        assert!(!Arc::ptr_eq(&a, &c));
    }

    /// Approval scope-flip: a handle table opened under the conversation
    /// scope is reachable under the worktree scope after a rekey, and the old
    /// scope is empty. The moved table is the SAME `Arc` (the live process is
    /// untouched — only the lookup key changed).
    #[tokio::test]
    async fn rekey_scope_moves_table_from_conversation_to_worktree() {
        let registry = BashHandleRegistry::new();
        let old = WorkScope::Conversation("conv-explore".to_string());
        let new = WorkScope::Worktree("/tmp/wt-approved".to_string());

        let before = registry.get_or_create(&old).await;
        {
            let mut g = before.write().await;
            g.insert(make_handle("conv-explore", "b-1", RING_BUFFER_BYTES));
        }

        assert!(
            registry.rekey_scope(&old, &new).await,
            "rekey must report a move"
        );

        // Old scope is now empty; new scope holds the same Arc and the handle.
        assert!(registry.get_existing(&old).await.is_none());
        let after = registry
            .get_existing(&new)
            .await
            .expect("handle table must be reachable under the new scope");
        assert!(
            Arc::ptr_eq(&before, &after),
            "rekey must move the Arc, not clone"
        );
        assert!(after.read().await.get(&HandleId::new("b-1")).is_some());
    }

    #[tokio::test]
    async fn rekey_scope_no_entry_is_noop() {
        let registry = BashHandleRegistry::new();
        let old = WorkScope::Conversation("never".to_string());
        let new = WorkScope::Worktree("/tmp/wt".to_string());
        assert!(!registry.rekey_scope(&old, &new).await);
        assert!(registry.get_existing(&new).await.is_none());
    }

    /// Occupied destination: the pre-existing `new` entry is preserved and the
    /// `old` entry is left in place — neither is clobbered.
    #[tokio::test]
    async fn rekey_scope_occupied_destination_does_not_clobber() {
        let registry = BashHandleRegistry::new();
        let old = WorkScope::Conversation("conv".to_string());
        let new = WorkScope::Worktree("/tmp/wt".to_string());
        let old_table = registry.get_or_create(&old).await;
        let new_table = registry.get_or_create(&new).await;

        assert!(
            !registry.rekey_scope(&old, &new).await,
            "occupied dest must not move"
        );

        let old_after = registry
            .get_existing(&old)
            .await
            .expect("old entry preserved");
        let new_after = registry
            .get_existing(&new)
            .await
            .expect("new entry preserved");
        assert!(Arc::ptr_eq(&old_table, &old_after));
        assert!(Arc::ptr_eq(&new_table, &new_after));
    }

    #[tokio::test]
    async fn rekey_scope_same_scope_is_noop() {
        let registry = BashHandleRegistry::new();
        let s = WorkScope::Worktree("/tmp/wt".to_string());
        let _ = registry.get_or_create(&s).await;
        assert!(!registry.rekey_scope(&s, &s).await);
        assert!(registry.get_existing(&s).await.is_some());
    }

    #[tokio::test]
    async fn snapshot_live_pgids_collects_across_scopes() {
        let registry = BashHandleRegistry::new();
        let scope_a = registry.get_or_create(&scope("conv-a")).await;
        let scope_b = registry.get_or_create(&scope("conv-b")).await;
        {
            let mut g = scope_a.write().await;
            let mut h = make_handle("conv-a", "b-1", RING_BUFFER_BYTES);
            // Override pgid via construction — we built it with 12345 in
            // make_handle. Just use that.
            let _ = Arc::get_mut(&mut h); // ensure no aliasing for the assertion below
            g.insert(h);
        }
        {
            let mut g = scope_b.write().await;
            g.insert(make_handle("conv-b", "b-1", RING_BUFFER_BYTES));
        }
        let pgids = registry.snapshot_live_pgids().await;
        // Both handles share pgid 12345 (test fixture).
        assert_eq!(pgids.len(), 2);
        assert!(pgids.iter().all(|&p| p == 12345));
    }

    #[tokio::test]
    async fn snapshot_live_pgids_skips_tombstoned() {
        let registry = BashHandleRegistry::new();
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let h = {
            let mut g = handles_arc.write().await;
            g.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES))
        };
        h.transition_to_terminal(
            FinalCause::Exited { exit_code: Some(0) },
            std::time::Duration::from_millis(1),
            crate::bash::handle::TOMBSTONE_TAIL_LINES,
        )
        .await;
        let pgids = registry.snapshot_live_pgids().await;
        assert!(
            pgids.is_empty(),
            "tombstoned handles must not appear in live pgid snapshot"
        );
    }

    #[tokio::test]
    async fn remove_returns_entry() {
        let registry = BashHandleRegistry::new();
        let _ = registry.get_or_create(&scope("conv-1")).await;
        assert_eq!(registry.scope_count().await, 1);
        assert!(registry.remove(&scope("conv-1")).await.is_some());
        assert_eq!(registry.scope_count().await, 0);
        assert!(registry.remove(&scope("conv-1")).await.is_none());
    }

    #[tokio::test]
    async fn cascade_bash_on_delete_no_entry_is_clean() {
        let registry = Arc::new(BashHandleRegistry::new());
        let report = cascade_bash_on_delete(&registry, &scope("never-existed"), None).await;
        assert!(report.kill_failures.is_empty());
        assert!(report.live_handle_pgids.is_empty());
        assert!(report.live_handle_pids.is_empty());
        assert!(report.kill_pending_kernel_pids.is_empty());
    }

    #[tokio::test]
    async fn cascade_bash_on_delete_tombstoned_only_is_clean() {
        let registry = Arc::new(BashHandleRegistry::new());
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let h = {
            let mut g = handles_arc.write().await;
            g.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES))
        };
        // Demote so there are no live handles to kill.
        h.transition_to_terminal(
            FinalCause::Exited { exit_code: Some(0) },
            std::time::Duration::from_millis(1),
            crate::bash::handle::TOMBSTONE_TAIL_LINES,
        )
        .await;

        let report = cascade_bash_on_delete(&registry, &scope("conv-1"), None).await;
        assert!(report.kill_failures.is_empty());
        assert!(report.live_handle_pgids.is_empty());
        // Registry entry is gone after cascade.
        assert_eq!(registry.scope_count().await, 0);
    }

    /// REQ-BASH-WS-002: when the continuation inherits the SAME `WorkScope`,
    /// the cascade is a no-op — the handle table and any live processes
    /// survive for the inheritor to peek/wait/kill.
    #[tokio::test]
    async fn cascade_bash_on_delete_skips_when_inheritor_shares_scope() {
        let registry = Arc::new(BashHandleRegistry::new());
        let wt = WorkScope::Worktree("/tmp/wt-inherit".to_string());
        let handles_arc = registry.get_or_create(&wt).await;
        {
            let mut g = handles_arc.write().await;
            g.insert(Handle::new_live(
                wt.clone(),
                HandleId::new("b-1"),
                "cmd for b-1".to_string(),
                None,
                12345,
                12345,
                RING_BUFFER_BYTES,
            ));
        }

        // Inheritor resolves to the same scope → skip teardown.
        let report = cascade_bash_on_delete(&registry, &wt, Some(&wt)).await;
        assert!(report.live_handle_pgids.is_empty());
        assert!(report.kill_failures.is_empty());
        // The table is preserved and the handle is still reachable.
        assert_eq!(registry.scope_count().await, 1);
        let preserved = registry.get_or_create(&wt).await;
        assert!(preserved.read().await.get(&HandleId::new("b-1")).is_some());
    }

    /// A continuation that resolves to a DIFFERENT scope does not preserve;
    /// teardown proceeds exactly as the no-inheritor case.
    #[tokio::test]
    async fn cascade_bash_on_delete_tears_down_when_inheritor_differs() {
        let registry = Arc::new(BashHandleRegistry::new());
        let wt = WorkScope::Worktree("/tmp/wt-deleted".to_string());
        let other = WorkScope::Conversation("conv-other".to_string());
        let _ = registry.get_or_create(&wt).await;

        let report = cascade_bash_on_delete(&registry, &wt, Some(&other)).await;
        assert!(report.kill_failures.is_empty());
        assert_eq!(registry.scope_count().await, 0);
    }

    /// REQ-WSUI-007: the cascade teardown path (handle table actually
    /// removed) must publish a `BashLifecycleEvent` for the scope so the
    /// work-scope bridge re-broadcasts the now-empty inventory. Without
    /// this, a scope with no concurrent tmux/browser change leaves the
    /// collapsed work-scope badge showing the killed handles.
    #[tokio::test]
    async fn cascade_bash_on_delete_emits_lifecycle_on_teardown() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = Arc::new(BashHandleRegistry::with_lifecycle_sink(Some(tx)));
        let s = scope("conv-teardown");
        let handles_arc = registry.get_or_create(&s).await;
        {
            let mut g = handles_arc.write().await;
            g.insert(make_handle("conv-teardown", "b-1", RING_BUFFER_BYTES));
        }

        let _ = cascade_bash_on_delete(&registry, &s, None).await;

        let evt = rx.try_recv().expect("teardown must emit a lifecycle edge");
        assert_eq!(evt.work_scope, s);
        assert!(rx.try_recv().is_err(), "exactly one edge per teardown");
    }

    /// The preserved early-return path (inheritor shares the scope) must
    /// NOT emit — nothing changed, so a refreshed broadcast would be a
    /// phantom edge.
    #[tokio::test]
    async fn cascade_bash_on_delete_no_lifecycle_on_preserved_path() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = Arc::new(BashHandleRegistry::with_lifecycle_sink(Some(tx)));
        let wt = WorkScope::Worktree("/tmp/wt-preserve-no-emit".to_string());
        let _ = registry.get_or_create(&wt).await;

        let _ = cascade_bash_on_delete(&registry, &wt, Some(&wt)).await;

        assert!(
            rx.try_recv().is_err(),
            "preserved path must not emit a lifecycle edge"
        );
    }

    /// A cascade against a scope with no handle table (nothing removed)
    /// must not emit — there is no stale inventory to refresh.
    #[tokio::test]
    async fn cascade_bash_on_delete_no_lifecycle_when_no_entry() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = Arc::new(BashHandleRegistry::with_lifecycle_sink(Some(tx)));

        let _ = cascade_bash_on_delete(&registry, &scope("never-existed"), None).await;

        assert!(
            rx.try_recv().is_err(),
            "no-entry cascade must not emit a lifecycle edge"
        );
    }

    #[tokio::test]
    async fn cascade_bash_on_delete_records_live_pgids_and_drops_entry() {
        // The fake handle uses pgid 12345 (a process group that almost
        // certainly does not exist on the test host). `kill(-12345, …)`
        // will return ESRCH, which the cascade swallows — so this test
        // verifies the bookkeeping side: the pgid is recorded in the
        // report and the registry entry is removed.
        let registry = Arc::new(BashHandleRegistry::new());
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        {
            let mut g = handles_arc.write().await;
            g.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));
            g.insert(make_handle("conv-1", "b-2", RING_BUFFER_BYTES));
        }

        let report = cascade_bash_on_delete(&registry, &scope("conv-1"), None).await;
        assert_eq!(report.live_handle_pgids.len(), 2);
        assert!(report.live_handle_pgids.iter().all(|&p| p == 12345));
        assert!(report.kill_failures.is_empty(), "ESRCH must be swallowed");
        assert_eq!(registry.scope_count().await, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::similar_names)] // pgid/pid mirror spec field names
    async fn cascade_bash_on_delete_actually_kills_a_real_subprocess() {
        // Spawn a real `sleep` in its own process group, register a
        // matching Handle, and verify the cascade SIGKILLs it. We then
        // `wait()` on the child (which reaps the zombie) and assert the
        // exit status reflects a SIGKILL termination — the process
        // outliving the cascade would have it still in `Running` state
        // and `try_wait()` would return Ok(None).
        use std::os::unix::process::CommandExt as _;
        use std::os::unix::process::ExitStatusExt as _;
        use std::process::Stdio;
        use tokio::time::{sleep, Duration};

        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("60");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                // Become own process group leader so kill(-pgid, …) hits
                // exactly this child (REQ-BASH-007 setpgid contract).
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("spawn sleep");
        let pid = child.id();
        // The child is its own process group leader, so the group id
        // equals the pid. Cast width is a `u32` -> `i32` conversion;
        // pids small enough to be valid here never overflow `i32`.
        #[allow(clippy::cast_possible_wrap, clippy::similar_names)]
        let pgid = pid as i32;

        // Verify the process is alive before the cascade runs.
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "subprocess must be running before cascade"
        );

        let registry = Arc::new(BashHandleRegistry::new());
        let real_scope = scope("conv-real");
        let handles_arc = registry.get_or_create(&real_scope).await;
        {
            let mut g = handles_arc.write().await;
            let h = Handle::new_live(
                real_scope.clone(),
                HandleId::new("b-1"),
                "sleep 60".to_string(),
                None,
                pgid,
                pid,
                RING_BUFFER_BYTES,
            );
            g.insert(h);
        }

        let report = cascade_bash_on_delete(&registry, &real_scope, None).await;
        assert!(report.live_handle_pgids.contains(&pgid));
        assert!(report.kill_failures.is_empty());
        assert_eq!(registry.scope_count().await, 0);

        // Wait briefly for the kernel to deliver SIGKILL, then reap the
        // child. The exit status's `signal()` should be `Some(SIGKILL)`.
        for _ in 0..20 {
            if let Some(status) = child.try_wait().expect("try_wait") {
                assert_eq!(
                    status.signal(),
                    Some(libc::SIGKILL),
                    "subprocess must have been terminated by SIGKILL"
                );
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
        // Best-effort cleanup if the kernel never delivered.
        unsafe {
            let _ = libc::kill(pgid, libc::SIGKILL);
        }
        let _ = child.wait();
        panic!("subprocess survived cascade SIGKILL within 1s");
    }
}
