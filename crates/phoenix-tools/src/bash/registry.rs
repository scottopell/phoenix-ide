//! In-memory bash handle registry.
//!
//! REQ-BASH-005 (per-`ResourceScopeKey` cap), REQ-BASH-006 (in-memory tombstones,
//! no `SQLite` shadow store), REQ-BASH-014 / REQ-BASH-WS-001 / REQ-BASH-WS-002
//! (per-`ResourceScopeKey` registry — a continuation chain on one worktree shares
//! its handle table because it resolves to the same `ResourceScopeKey`).
//!
//! Lifetime: registries live in process memory only. A Phoenix restart
//! drops them and any handles they hold; agents see `handle_not_found` on
//! a previously-known handle (matching the spec's "handles do NOT survive
//! Phoenix restart" guarantee).
//!
//! Lock ordering for cap enforcement and spawn (consumed by `BashTool::run`):
//! acquire the registry's `RwLock<HashMap>` for read, then the
//! `ResourceScopeKey`'s `RwLock<ResourceScopeKeyHandles>` for write. The per-scope lock
//! holds for the duration of cap-check + handle insert to prevent two
//! concurrent spawns from both observing `count == cap - 1` and racing past
//! the cap.

use phoenix_core::work_scope::EffectiveResourceAccess;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::SystemTime;

use phoenix_core::work_scope::ResourceScopeKey;
use thiserror::Error;
use tokio::sync::{Notify, RwLock};

use super::handle::{Handle, HandleId};
use super::ring::RING_BUFFER_BYTES;

/// Per-`ResourceScopeKey` cap on `running` handles (REQ-BASH-005:
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
    #[error("this work scope is being torn down and no longer accepts bash spawns")]
    SpawnFenced,
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
    pub work_scope: ResourceScopeKey,
    pub control_scope: ResourceScopeKey,
    pub handle_id: HandleId,
    pub pgid: i32,
}

#[derive(Debug, Clone)]
pub struct RegisteredHandle {
    pub owner: ResourceScopeKey,
    pub handle: Arc<Handle>,
}

#[derive(Debug, Default)]
struct SpawnAdmission {
    pending: AtomicUsize,
    settled: Arc<Notify>,
}

#[derive(Debug)]
pub struct SpawnReservation {
    owner: ResourceScopeKey,
    handle_id: HandleId,
    committed: bool,
    admission: Arc<SpawnAdmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BashTeardownGeneration(u64);

#[derive(Debug)]
pub struct BashTeardownFence {
    entry: Arc<RwLock<ResourceScopeKeyHandles>>,
    generation: BashTeardownGeneration,
}

impl BashTeardownFence {
    #[must_use]
    pub fn generation(&self) -> BashTeardownGeneration {
        self.generation
    }
}

impl SpawnReservation {
    #[must_use]
    pub fn handle_id(&self) -> &HandleId {
        &self.handle_id
    }
}

impl SpawnReservation {
    fn settle(&mut self) {
        if !self.committed {
            self.committed = true;
            self.admission.pending.fetch_sub(1, Ordering::AcqRel);
            self.admission.settled.notify_waiters();
        }
    }
}

impl Drop for SpawnReservation {
    fn drop(&mut self) {
        self.settle();
    }
}

/// Per-owner handle table. Tracks live handles (for cap enforcement and owner-local
/// inventory) and tombstones (so peek/wait/kill on an exited handle still resolves
/// until the owner is hard-deleted with no inheritor or Phoenix restarts).
///
/// The unified `handles` map covers both live and tombstoned entries; the
/// discrimination is made by inspecting the handle's `HandleState`. This keeps the
/// lookup path single-source — a handle that transitions from `Live` to
/// `Tombstoned` is the SAME `Arc<Handle>` (its `state` field swaps), and lookup
/// never has to "follow" between two maps.
#[derive(Debug, Default)]
pub struct ResourceScopeKeyHandles {
    /// All handles owned by this work scope, by id. Live and tombstoned alike.
    handles: HashMap<HandleId, Arc<Handle>>,
    /// Once teardown begins, no new spawn reservation may commit for this owner.
    teardown_started: bool,
    /// Cancellation-safe admission count and teardown notification.
    admission: Arc<SpawnAdmission>,
    /// Monotonic identity of the lifecycle request that most recently fenced admission.
    teardown_generation: u64,
}

impl ResourceScopeKeyHandles {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    pub async fn live_count_for_actor(&self, actor: &EffectiveResourceAccess) -> usize {
        let mut n = 0;
        for h in self.handles.values() {
            if actor.can_control(&h.creator_conversation_id, h.authority)
                && h.state().await.is_live()
            {
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
                let age_seconds = now.duration_since(h.started_at).map_or(0, |d| d.as_secs());
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

    pub async fn live_summary_for_actor(
        &self,
        actor: &EffectiveResourceAccess,
    ) -> Vec<LiveHandleSummary> {
        let mut out = Vec::new();
        let now = SystemTime::now();
        for h in self.handles.values() {
            if actor.can_control(&h.creator_conversation_id, h.authority)
                && h.state().await.is_live()
            {
                out.push(LiveHandleSummary {
                    handle: h.handle_id.clone(),
                    cmd: h.cmd.clone(),
                    label: h.label.clone(),
                    age_seconds: now
                        .duration_since(h.started_at)
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0),
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
    /// hard-delete cascade (which removes the whole `ResourceScopeKey` table) but
    /// kept on the API surface for surgical removal flows.
    #[allow(dead_code)]
    pub fn remove(&mut self, id: &HandleId) -> Option<Arc<Handle>> {
        self.handles.remove(id)
    }

    pub fn remove_for_actor(&mut self, actor: &EffectiveResourceAccess) -> Vec<Arc<Handle>> {
        let ids: Vec<_> = self
            .handles
            .iter()
            .filter(|(_, handle)| {
                actor.can_control(&handle.creator_conversation_id, handle.authority)
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| self.handles.remove(&id))
            .collect()
    }

    pub fn remove_restricted_created_by(&mut self, conversation_id: &str) -> Vec<Arc<Handle>> {
        let ids: Vec<_> = self
            .handles
            .iter()
            .filter(|(_, handle)| {
                handle.creator_conversation_id == conversation_id
                    && handle.authority == phoenix_core::work_scope::ResourceAuthority::Restricted
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| self.handles.remove(&id))
            .collect()
    }

    fn take_all(&mut self) -> Vec<Arc<Handle>> {
        self.handles.drain().map(|(_, handle)| handle).collect()
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

/// Signal published when a bash handle in a `ResourceScopeKey` changes state
/// (spawned, transitioned to terminal, or killed). Mirrors the browser
/// `BrowserSessionLifecycleEvent` shape: it carries only the affected
/// `ResourceScopeKey`, leaving inventory assembly and conversation routing to the
/// runtime's work-scope bridge. State transitions only — NOT per output line
/// (REQ-WSUI-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashLifecyclePhase {
    Spawned,
    KillPendingKernel,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashTerminalEffect {
    InventoryOnly,
    InventoryAndBranchReconcile,
}

#[derive(Debug, Clone)]
pub struct BashLifecycleEvent {
    pub owner: ResourceScopeKey,
    pub handle_id: Option<HandleId>,
    pub phase: BashLifecyclePhase,
    pub cause: Option<crate::bash::handle::FinalCause>,
    pub terminal_effect: Option<BashTerminalEffect>,
}

/// Sink the registry publishes [`BashLifecycleEvent`]s into. A bounded `mpsc`
/// keeps the registry decoupled from per-conversation routing (the runtime
/// owns that). `None` for tool-level tests that don't care about the push
/// path.
pub type BashLifecycleSink = tokio::sync::mpsc::UnboundedSender<BashLifecycleEvent>;

/// Top-level registry: maps `ResourceScopeKey` -> per-`ResourceScopeKey` handle table.
///
/// One registry instance per Phoenix process. Owned by the runtime layer
/// and reached by tools through `ToolContext::bash_handles()`. Keying by
/// durable work-scope identity (rather than `conversation_id`) is what lets a
/// continuation chain share its handle table (REQ-BASH-WS-001).
#[derive(Debug, Default)]
pub struct BashHandleRegistry {
    inner: RwLock<HashMap<ResourceScopeKey, Arc<RwLock<ResourceScopeKeyHandles>>>>,
    handles_by_id: RwLock<HashMap<HandleId, RegisteredHandle>>,
    /// Per-handle ring byte cap. Defaults to [`RING_BUFFER_BYTES`]; tests
    /// override to small values to exercise eviction.
    ring_bytes_cap: usize,
    /// Per-`ResourceScopeKey` live-handle cap. Defaults to [`LIVE_HANDLE_CAP`];
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
            handles_by_id: RwLock::new(HashMap::new()),
            ring_bytes_cap: RING_BUFFER_BYTES,
            live_handle_cap: LIVE_HANDLE_CAP,
            lifecycle_sink: None,
        }
    }

    /// Create a registry that publishes bash state-transition signals into
    /// `sink`. The runtime wires this to the work-scope push bridge, which
    /// resolves the scope's conversation and broadcasts a `ResourceScopeKeyUpdate`.
    /// Mirrors `BrowserSessionManager::with_lifecycle_sink`.
    #[must_use]
    pub fn with_lifecycle_sink(sink: Option<BashLifecycleSink>) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            handles_by_id: RwLock::new(HashMap::new()),
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
            handles_by_id: RwLock::new(HashMap::new()),
            ring_bytes_cap,
            live_handle_cap,
            lifecycle_sink: None,
        }
    }

    /// Publish a bash state-transition signal for `work_scope` if a sink is
    /// wired. Best-effort: a dropped receiver / closed channel is logged at
    /// `debug` (capability gap) and does not affect handle correctness.
    /// Mirrors `BrowserSessionManager::emit_lifecycle`.
    pub fn emit_lifecycle(
        &self,
        owner: &ResourceScopeKey,
        handle_id: Option<HandleId>,
        phase: BashLifecyclePhase,
        cause: Option<&crate::bash::handle::FinalCause>,
        terminal_effect: Option<BashTerminalEffect>,
    ) {
        let Some(sink) = self.lifecycle_sink.as_ref() else {
            return;
        };
        let handle_id_for_log = handle_id.as_ref().map(|id| id.as_str().to_string());
        let event = BashLifecycleEvent {
            owner: owner.clone(),
            handle_id,
            phase,
            cause: cause.cloned(),
            terminal_effect,
        };
        if let Err(e) = sink.send(event) {
            tracing::debug!(
                owner = %owner,
                phase = ?phase,
                cause = ?cause,
                handle_id = handle_id_for_log.as_deref(),
                terminal_effect = ?terminal_effect,
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

    fn allocate_handle_id() -> HandleId {
        HandleId::new(format!("b-{}", uuid::Uuid::new_v4()))
    }

    /// Get-or-create the per-`ResourceScopeKey` handle table. Matches the
    /// `BrowserSessionManager::get_session` pattern — returns the same
    /// `Arc<RwLock<ResourceScopeKeyHandles>>` for repeated calls with the
    /// same `ResourceScopeKey`, so a continuation chain on one worktree shares
    /// one table (REQ-BASH-WS-001).
    pub async fn get_or_create(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Arc<RwLock<ResourceScopeKeyHandles>> {
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
        let entry = Arc::new(RwLock::new(ResourceScopeKeyHandles::new()));
        map.insert(work_scope.clone(), entry.clone());
        entry
    }
    /// Look up a `ResourceScopeKey`'s handle table **without creating one**.
    ///
    /// Read-only counterpart to [`Self::get_or_create`], for observability
    /// surfaces (the work-scope inventory endpoint) that must reflect the
    /// registry as-is and must not allocate a table for a scope that has
    /// never spawned a bash handle.
    pub async fn get_existing(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Option<Arc<RwLock<ResourceScopeKeyHandles>>> {
        self.inner.read().await.get(work_scope).cloned()
    }

    /// Reserves one owner-local live-handle slot and an opaque global handle id.
    ///
    /// # Errors
    /// Returns [`BashHandleError::SpawnFenced`] after owner teardown begins, or
    /// [`BashHandleError::HandleCapReached`] when live and pending spawns fill the cap.
    pub async fn reserve_spawn(
        &self,
        owner: &ResourceScopeKey,
    ) -> Result<SpawnReservation, BashHandleError> {
        let entry = self.get_or_create(owner).await;
        let handles = entry.write().await;
        if handles.teardown_started {
            return Err(BashHandleError::SpawnFenced);
        }
        if handles.live_count().await + handles.admission.pending.load(Ordering::Acquire)
            >= self.live_handle_cap
        {
            return Err(BashHandleError::HandleCapReached {
                cap: self.live_handle_cap,
                live_handles: handles.live_summary().await,
            });
        }
        handles.admission.pending.fetch_add(1, Ordering::AcqRel);
        Ok(SpawnReservation {
            owner: owner.clone(),
            handle_id: Self::allocate_handle_id(),
            committed: false,
            admission: handles.admission.clone(),
        })
    }

    /// Commits an admitted spawn into its owner table and global lookup index.
    ///
    /// # Errors
    /// Returns [`BashHandleError::SpawnFenced`] if the owner table disappeared.
    pub async fn commit_spawn(
        &self,
        reservation: &mut SpawnReservation,
        handle: Arc<Handle>,
    ) -> Result<Arc<Handle>, BashHandleError> {
        let entry = self
            .get_existing(&reservation.owner)
            .await
            .ok_or(BashHandleError::SpawnFenced)?;
        let mut by_id = self.handles_by_id.write().await;
        let mut table = entry.write().await;
        let inserted = table.insert(handle);
        by_id.insert(
            inserted.handle_id.clone(),
            RegisteredHandle {
                owner: reservation.owner.clone(),
                handle: inserted.clone(),
            },
        );
        reservation.settle();
        Ok(inserted)
    }

    pub fn abort_spawn(&self, mut reservation: SpawnReservation) {
        reservation.settle();
    }

    /// Reopen admission when the authoritative lifecycle mutation following a
    /// completed cleanup fails and the `WorkScope` therefore remains active.
    pub async fn reopen_spawn_admission(
        &self,
        owner: &ResourceScopeKey,
        generation: BashTeardownGeneration,
    ) -> bool {
        let Some(entry) = self.get_existing(owner).await else {
            return false;
        };
        let mut table = entry.write().await;
        if table.teardown_started && table.teardown_generation == generation.0 {
            table.teardown_started = false;
            true
        } else {
            false
        }
    }

    pub async fn get_by_id(&self, handle_id: &HandleId) -> Option<RegisteredHandle> {
        self.handles_by_id.read().await.get(handle_id).cloned()
    }

    #[doc(hidden)]
    pub async fn register_existing_handle(&self, owner: &ResourceScopeKey, handle: Arc<Handle>) {
        let entry = self.get_or_create(owner).await;
        entry.write().await.insert(handle.clone());
        self.handles_by_id.write().await.insert(
            handle.handle_id.clone(),
            RegisteredHandle {
                owner: owner.clone(),
                handle,
            },
        );
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
        for (owner, entry) in map.iter() {
            let scope_handles = entry.read().await;
            for handle in scope_handles.all() {
                if let Some(pgid) = handle.live_pgid().await {
                    out.push(LiveHandleProcessGroup {
                        work_scope: owner.clone(),
                        control_scope: handle.controller_scope.clone(),
                        handle_id: handle.handle_id.clone(),
                        pgid,
                    });
                }
            }
        }
        out
    }

    pub async fn owner_handles(&self, owner: &ResourceScopeKey) -> Vec<RegisteredHandle> {
        let Some(entry) = self.get_existing(owner).await else {
            return Vec::new();
        };
        let registered = entry
            .read()
            .await
            .all()
            .cloned()
            .map(|handle| RegisteredHandle {
                owner: owner.clone(),
                handle,
            })
            .collect();
        registered
    }

    /// Remove a `ResourceScopeKey`'s handle table outright. Used by the
    /// hard-delete cascade (REQ-BASH-006). Returns the removed entry so
    /// the caller can SIGKILL its live process groups synchronously.
    pub async fn begin_teardown(&self, work_scope: &ResourceScopeKey) -> Option<BashTeardownFence> {
        let (entry, generation) = {
            let mut tables = self.inner.write().await;
            let entry = tables
                .entry(work_scope.clone())
                .or_insert_with(|| Arc::new(RwLock::new(ResourceScopeKeyHandles::new())))
                .clone();
            let mut table = entry.write().await;
            table.teardown_generation = table.teardown_generation.wrapping_add(1);
            table.teardown_started = true;
            let generation = BashTeardownGeneration(table.teardown_generation);
            drop(table);
            (entry, generation)
        };
        loop {
            let notified = {
                let mut table = entry.write().await;
                table.teardown_started = true;
                let mut notified = Box::pin(table.admission.settled.clone().notified_owned());
                notified.as_mut().enable();
                if table.admission.pending.load(Ordering::Acquire) == 0 {
                    return Some(BashTeardownFence {
                        entry: entry.clone(),
                        generation,
                    });
                }
                notified
            };
            notified.await;
        }
    }

    async fn drain_owner(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Option<(Arc<RwLock<ResourceScopeKeyHandles>>, BashTeardownGeneration)> {
        let fence = self.begin_teardown(work_scope).await?;
        let removed_handles = fence.entry.write().await.take_all();
        self.remove_from_global_index(&removed_handles).await;

        let removed = Arc::new(RwLock::new(ResourceScopeKeyHandles::new()));
        {
            let mut table = removed.write().await;
            for handle in removed_handles {
                table.insert(handle);
            }
        }
        Some((removed, fence.generation()))
    }

    pub async fn remove(
        &self,
        work_scope: &ResourceScopeKey,
    ) -> Option<Arc<RwLock<ResourceScopeKeyHandles>>> {
        self.drain_owner(work_scope)
            .await
            .map(|(removed, _)| removed)
    }

    async fn remove_from_global_index(&self, handles: &[Arc<Handle>]) {
        let mut by_id = self.handles_by_id.write().await;
        for handle in handles {
            by_id.remove(&handle.handle_id);
        }
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
/// `ResourceScopeKey` (it's an argument), so it is not duplicated here.
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
    /// Fence identity installed by an owner-level teardown. Absent when the
    /// owner was preserved and only actor-restricted handles were removed.
    pub teardown_generation: Option<BashTeardownGeneration>,
}

/// Run the bash side of the hard-delete cascade for `work_scope`
/// (REQ-BASH-006, REQ-BASH-WS-002). Mirrors the tmux/browser cascades'
/// `(work_scope, inheritor_scope)` signature.
///
/// When `inheritor_scope == Some(work_scope)`, the owner table and its spawn
/// admission remain active. Handles with shared Work authority remain owned by
/// the scope; restricted handles created by the deleted conversation are
/// removed from both indexes and their live process groups are killed.
///
/// Without a same-scope inheritor, teardown atomically:
///
///   1. Fences the owner table against new reservations and waits for admitted
///      spawns to settle.
///   2. Drains all handles from the owner table and global lookup index while
///      retaining the empty fenced table so late spawns remain rejected.
///   3. Snapshots live pgid / pid / `kill_pending_kernel` state across the
///      drained handles.
///   4. Sends `SIGKILL` to each live process group via
///      `kill(-pgid, SIGKILL)` (catches immediate descendants per
///      REQ-BASH-007's setpgid contract).
///
/// The final step satisfies the `KillSignalSentForAllLiveHandles` ensures clause; failures
/// populate `kill_failures` for the orchestrator's WARN log but are not
/// fatal — the spec's policy is "log and continue" (REQ-BED-032).
///
/// SIGKILL rather than SIGTERM: hard-delete deletes the conversation
/// outright, so no agent is left to observe a graceful close. Same
/// rationale as `shutdown_kill_tree` in [`super::reaper`].
pub async fn cascade_bash_on_delete(
    registry: &Arc<BashHandleRegistry>,
    work_scope: &ResourceScopeKey,
    actor: &EffectiveResourceAccess,
    inheritor_scope: Option<&ResourceScopeKey>,
) -> CascadeBashReport {
    if inheritor_scope == Some(work_scope) {
        let Some(entry) = registry.get_existing(work_scope).await else {
            return CascadeBashReport::default();
        };
        let handles = entry
            .write()
            .await
            .remove_restricted_created_by(actor.conversation_id());
        registry.remove_from_global_index(&handles).await;
        return kill_selected_handles(registry, work_scope, handles).await;
    }
    teardown_bash_owner(registry, work_scope).await
}

/// Fence an owner against future Bash spawns, wait for admitted spawns to
/// settle, remove all registered handles, and kill every live process group.
pub async fn teardown_bash_owner(
    registry: &Arc<BashHandleRegistry>,
    work_scope: &ResourceScopeKey,
) -> CascadeBashReport {
    let mut report = CascadeBashReport::default();
    let Some((removed, generation)) = registry.drain_owner(work_scope).await else {
        return report;
    };
    report.teardown_generation = Some(generation);
    let handles: Vec<_> = removed.read().await.all().cloned().collect();

    for h in &handles {
        let Some(group_id) = h.live_pgid().await else {
            continue;
        };
        let process_id = h.live_pid().await;
        let kill_pending = h.is_kill_pending_kernel().await;
        record_handle_in_report(&mut report, group_id, process_id, kill_pending);

        #[cfg(unix)]
        {
            // SAFETY: kill(2) with negative pid signals the process group;
            // no memory implications. ESRCH (group already gone) is expected.
            let rc = unsafe { libc::kill(-group_id, libc::SIGKILL) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    report.kill_failures.push((group_id, err.to_string()));
                }
            }
        }
    }

    if !handles.is_empty() {
        registry.emit_lifecycle(
            work_scope,
            None,
            BashLifecyclePhase::Terminal,
            None,
            Some(BashTerminalEffect::InventoryOnly),
        );
    }

    report
}

async fn kill_selected_handles(
    registry: &BashHandleRegistry,
    work_scope: &ResourceScopeKey,
    handles: Vec<Arc<Handle>>,
) -> CascadeBashReport {
    let mut report = CascadeBashReport::default();
    if handles.is_empty() {
        return report;
    }
    for handle in handles {
        if let Some(group_id) = handle.live_pgid().await {
            record_handle_in_report(
                &mut report,
                group_id,
                handle.live_pid().await,
                handle.is_kill_pending_kernel().await,
            );
            #[cfg(unix)]
            {
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
    registry.emit_lifecycle(
        work_scope,
        None,
        BashLifecyclePhase::Terminal,
        None,
        Some(BashTerminalEffect::InventoryOnly),
    );
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

    fn scope(name: &str) -> ResourceScopeKey {
        ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse(name).unwrap())
    }

    fn work_actor(id: &str) -> EffectiveResourceAccess {
        EffectiveResourceAccess::new(id, phoenix_core::work_scope::ResourceAuthority::Work)
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
        registry.emit_lifecycle(
            &a,
            Some(HandleId::new("b-a")),
            BashLifecyclePhase::Spawned,
            None,
            None,
        );
        registry.emit_lifecycle(
            &b,
            None,
            BashLifecyclePhase::Terminal,
            None,
            Some(BashTerminalEffect::InventoryOnly),
        );

        let e1 = rx.try_recv().expect("first event missing");
        assert_eq!(e1.owner, a);
        assert_eq!(e1.handle_id, Some(HandleId::new("b-a")));
        assert_eq!(e1.phase, BashLifecyclePhase::Spawned);
        assert_eq!(e1.terminal_effect, None);
        let e2 = rx.try_recv().expect("second event missing");
        assert_eq!(e2.owner, b);
        assert_eq!(e2.handle_id, None);
        assert_eq!(e2.phase, BashLifecyclePhase::Terminal);
        assert_eq!(e2.terminal_effect, Some(BashTerminalEffect::InventoryOnly));
        assert!(rx.try_recv().is_err(), "no more events expected");
    }

    #[tokio::test]
    async fn emit_lifecycle_without_sink_is_no_op() {
        let registry = BashHandleRegistry::new();
        let scope = scope("conv-X");
        registry.emit_lifecycle(
            &scope,
            Some(HandleId::new("b-x")),
            BashLifecyclePhase::Spawned,
            None,
            None,
        );
        assert!(registry.lifecycle_sink().is_none());
    }

    #[tokio::test]
    async fn handle_ids_are_globally_unique_across_owners() {
        let registry = BashHandleRegistry::new();
        let a = registry
            .reserve_spawn(&scope("conv-a"))
            .await
            .expect("reserve a");
        let b = registry
            .reserve_spawn(&scope("conv-b"))
            .await
            .expect("reserve b");
        let c = registry
            .reserve_spawn(&scope("conv-a"))
            .await
            .expect("reserve c");
        assert!(a.handle_id().as_str().starts_with("b-"));
        assert_ne!(a.handle_id(), b.handle_id());
        assert_ne!(a.handle_id(), c.handle_id());
        assert_ne!(b.handle_id(), c.handle_id());
    }

    #[tokio::test]
    async fn owner_local_inventory_and_removal_leave_other_owner_intact() {
        let registry = BashHandleRegistry::new();
        let owner_a = scope("conv-a");
        let owner_b = scope("conv-b");
        let table_a = registry.get_or_create(&owner_a).await;
        let table_b = registry.get_or_create(&owner_b).await;
        table_a
            .write()
            .await
            .insert(make_handle("conv-a", "b-1", RING_BUFFER_BYTES));
        table_b
            .write()
            .await
            .insert(make_handle("conv-b", "b-2", RING_BUFFER_BYTES));

        assert_eq!(registry.owner_handles(&owner_a).await.len(), 1);
        assert_eq!(registry.owner_handles(&owner_b).await.len(), 1);
        let _ = registry.remove(&owner_a).await;
        assert!(registry.owner_handles(&owner_a).await.is_empty());
        assert_eq!(registry.owner_handles(&owner_b).await.len(), 1);
    }

    #[tokio::test]
    async fn reservation_abort_and_commit_behave_as_expected() {
        let registry = BashHandleRegistry::new();
        let owner = scope("conv-a");

        let reservation = registry.reserve_spawn(&owner).await.expect("reserve abort");
        let aborted_id = reservation.handle_id().clone();
        registry.abort_spawn(reservation);
        assert!(registry.get_by_id(&aborted_id).await.is_none());

        let mut reservation = registry
            .reserve_spawn(&owner)
            .await
            .expect("reserve commit");
        let committed_id = reservation.handle_id().clone();
        let inserted = registry
            .commit_spawn(
                &mut reservation,
                Handle::new_live(
                    owner.clone(),
                    committed_id.clone(),
                    "pwd".into(),
                    None,
                    12345,
                    12345,
                    RING_BUFFER_BYTES,
                ),
            )
            .await
            .expect("commit spawn");
        assert_eq!(inserted.handle_id, committed_id);
        assert!(registry.get_by_id(&committed_id).await.is_some());
    }

    #[tokio::test]
    async fn teardown_fences_new_spawn_admission() {
        let registry = BashHandleRegistry::new();
        let owner = scope("conv-a");
        registry.get_or_create(&owner).await;
        let _ = registry
            .begin_teardown(&owner)
            .await
            .expect("begin teardown");
        assert!(registry.reserve_spawn(&owner).await.is_err());
    }

    #[tokio::test]
    async fn failed_lifecycle_mutation_can_reopen_spawn_admission() {
        let registry = BashHandleRegistry::new();
        let owner = scope("conv-a");
        let generation = registry
            .begin_teardown(&owner)
            .await
            .expect("teardown")
            .generation();
        assert!(matches!(
            registry.reserve_spawn(&owner).await,
            Err(BashHandleError::SpawnFenced)
        ));

        assert!(registry.reopen_spawn_admission(&owner, generation).await);
        assert!(registry.reserve_spawn(&owner).await.is_ok());
    }

    #[tokio::test]
    async fn stale_lifecycle_failure_cannot_reopen_newer_teardown() {
        let registry = BashHandleRegistry::new();
        let owner = scope("conv-a");
        let stale = registry
            .begin_teardown(&owner)
            .await
            .expect("first teardown")
            .generation();
        let current = registry
            .begin_teardown(&owner)
            .await
            .expect("second teardown")
            .generation();

        assert!(!registry.reopen_spawn_admission(&owner, stale).await);
        assert!(matches!(
            registry.reserve_spawn(&owner).await,
            Err(BashHandleError::SpawnFenced)
        ));
        assert!(registry.reopen_spawn_admission(&owner, current).await);
        assert!(registry.reserve_spawn(&owner).await.is_ok());
    }

    #[tokio::test]
    async fn teardown_of_unseen_owner_installs_spawn_fence() {
        let registry = BashHandleRegistry::new();
        let owner = scope("never-spawned");
        assert!(registry.get_existing(&owner).await.is_none());
        assert!(registry.begin_teardown(&owner).await.is_some());
        assert!(matches!(
            registry.reserve_spawn(&owner).await,
            Err(BashHandleError::SpawnFenced)
        ));
    }

    #[tokio::test]
    async fn pending_reservation_counts_against_cap() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 1);
        let owner = scope("conv-a");
        let _reservation = registry.reserve_spawn(&owner).await.expect("first slot");
        assert!(matches!(
            registry.reserve_spawn(&owner).await,
            Err(BashHandleError::HandleCapReached { cap: 1, .. })
        ));
    }

    #[tokio::test]
    async fn dropped_reservation_releases_teardown_waiter() {
        let registry = Arc::new(BashHandleRegistry::new());
        let owner = scope("conv-a");
        let reservation = registry.reserve_spawn(&owner).await.expect("reservation");
        let teardown = {
            let registry = registry.clone();
            let owner = owner.clone();
            tokio::spawn(async move { registry.begin_teardown(&owner).await })
        };
        tokio::task::yield_now().await;
        assert!(!teardown.is_finished());
        drop(reservation);
        assert!(teardown.await.expect("join").is_some());
    }

    #[tokio::test]
    async fn teardown_waits_for_admitted_spawn_to_settle() {
        let registry = Arc::new(BashHandleRegistry::new());
        let owner = scope("conv-a");
        let reservation = registry.reserve_spawn(&owner).await.expect("reservation");
        let teardown = {
            let registry = registry.clone();
            let owner = owner.clone();
            tokio::spawn(async move { registry.begin_teardown(&owner).await })
        };
        tokio::task::yield_now().await;
        assert!(!teardown.is_finished());
        registry.abort_spawn(reservation);
        assert!(teardown.await.expect("join").is_some());
    }

    #[tokio::test]
    async fn preserved_owner_still_accepts_spawns() {
        let registry = Arc::new(BashHandleRegistry::new());
        let owner = scope("conv-a");
        let actor = work_actor("owner");
        let _ = registry.get_or_create(&owner).await;
        let _ = cascade_bash_on_delete(&registry, &owner, &actor, Some(&owner)).await;
        assert!(registry.reserve_spawn(&owner).await.is_ok());
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
            BashHandleError::SpawnFenced => panic!("expected cap rejection"),
        }
    }

    #[tokio::test]
    async fn cap_rejection_includes_cmd_and_age() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 1);
        let handles_arc = registry.get_or_create(&scope("conv-1")).await;
        let mut guard = handles_arc.write().await;
        guard.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));
        let err = guard.check_cap(1).await.unwrap_err();
        let BashHandleError::HandleCapReached { live_handles, .. } = err else {
            panic!("expected cap rejection");
        };
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
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
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

    /// Conversations assigned the same durable work scope share one handle
    /// table, while a distinct opaque scope remains isolated (REQ-BASH-WS-001).
    #[tokio::test]
    async fn get_or_create_shares_table_across_work_scope() {
        let registry = BashHandleRegistry::new();
        let shared = scope("opaque-shared");
        let a = registry.get_or_create(&shared).await;
        let b = registry.get_or_create(&shared).await;
        assert!(Arc::ptr_eq(&a, &b));

        let distinct = scope("opaque-distinct");
        let c = registry.get_or_create(&distinct).await;
        assert!(!Arc::ptr_eq(&a, &c));
    }

    /// Approval scope-flip: a handle table opened under the conversation
    /// scope is reachable under the worktree scope after a rekey, and the old
    /// scope is empty. The moved table is the SAME `Arc` (the live process is
    /// untouched — only the lookup key changed).
    /// Occupied destination: the pre-existing `new` entry is preserved and the
    /// `old` entry is left in place — neither is clobbered.
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
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
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
    async fn remove_drains_but_preserves_teardown_fence() {
        let registry = BashHandleRegistry::new();
        let owner = scope("conv-1");
        let _ = registry.get_or_create(&owner).await;
        assert!(registry.begin_teardown(&owner).await.is_some());
        assert!(registry.remove(&owner).await.is_some());
        assert_eq!(registry.scope_count().await, 1);
        assert!(matches!(
            registry.reserve_spawn(&owner).await,
            Err(BashHandleError::SpawnFenced)
        ));
    }

    #[tokio::test]
    async fn work_scope_teardown_removes_coordinator_controlled_handle() {
        let registry = Arc::new(BashHandleRegistry::new());
        let lifecycle_scope = scope("inspected-work");
        let handle = Handle::new_live_for_actor_with_owner(
            ResourceScopeKey::Coordinator,
            HandleId::new("b-coordinator"),
            "coordinator".to_string(),
            phoenix_core::work_scope::ResourceAuthority::Work,
            "pwd".to_string(),
            None,
            12345,
            12345,
            RING_BUFFER_BYTES,
        );
        registry
            .register_existing_handle(&lifecycle_scope, handle.clone())
            .await;
        handle
            .transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                std::time::Duration::from_millis(1),
                std::time::SystemTime::now(),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;

        assert_eq!(registry.owner_handles(&lifecycle_scope).await.len(), 1);
        let report =
            cascade_bash_on_delete(&registry, &lifecycle_scope, &work_actor("owner"), None).await;
        assert!(report.kill_failures.is_empty());
        assert!(registry.owner_handles(&lifecycle_scope).await.is_empty());
    }

    #[tokio::test]
    async fn cascade_bash_on_delete_no_entry_is_clean() {
        let registry = Arc::new(BashHandleRegistry::new());
        let report = cascade_bash_on_delete(
            &registry,
            &scope("never-existed"),
            &work_actor("owner"),
            None,
        )
        .await;
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
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
            crate::bash::handle::TOMBSTONE_TAIL_LINES,
        )
        .await;

        let report =
            cascade_bash_on_delete(&registry, &scope("conv-1"), &work_actor("owner"), None).await;
        assert!(report.kill_failures.is_empty());
        assert!(report.live_handle_pgids.is_empty());
        // Registry entry is gone after cascade.
        assert_eq!(registry.scope_count().await, 1);
    }

    /// REQ-BASH-WS-002: when the continuation inherits the SAME `ResourceScopeKey`,
    /// the cascade is a no-op — the handle table and any live processes
    /// survive for the inheritor to peek/wait/kill.
    #[tokio::test]
    async fn cascade_bash_on_delete_skips_when_inheritor_shares_scope() {
        let registry = Arc::new(BashHandleRegistry::new());
        let wt = scope("/tmp/wt-inherit");
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
        let report = cascade_bash_on_delete(&registry, &wt, &work_actor("owner"), Some(&wt)).await;
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
        let wt = scope("/tmp/wt-deleted");
        let other = scope("conv-other");
        let _ = registry.get_or_create(&wt).await;

        let report =
            cascade_bash_on_delete(&registry, &wt, &work_actor("owner"), Some(&other)).await;
        assert!(report.kill_failures.is_empty());
        assert_eq!(registry.scope_count().await, 1);
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

        let _ = cascade_bash_on_delete(&registry, &s, &work_actor("owner"), None).await;

        let evt = rx.try_recv().expect("teardown must emit a lifecycle edge");
        assert_eq!(evt.owner, s);
        assert_eq!(evt.handle_id, None);
        assert_eq!(evt.terminal_effect, Some(BashTerminalEffect::InventoryOnly));
        assert!(rx.try_recv().is_err(), "exactly one edge per teardown");
    }

    /// The preserved early-return path (inheritor shares the scope) must
    /// NOT emit — nothing changed, so a refreshed broadcast would be a
    /// phantom edge.
    #[tokio::test]
    async fn cascade_bash_on_delete_no_lifecycle_on_preserved_path() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = Arc::new(BashHandleRegistry::with_lifecycle_sink(Some(tx)));
        let wt = scope("/tmp/wt-preserve-no-emit");
        let _ = registry.get_or_create(&wt).await;

        let _ = cascade_bash_on_delete(&registry, &wt, &work_actor("owner"), Some(&wt)).await;

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

        let _ = cascade_bash_on_delete(
            &registry,
            &scope("never-existed"),
            &work_actor("owner"),
            None,
        )
        .await;

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

        let report =
            cascade_bash_on_delete(&registry, &scope("conv-1"), &work_actor("owner"), None).await;
        assert_eq!(report.live_handle_pgids.len(), 2);
        assert!(report.live_handle_pgids.iter().all(|&p| p == 12345));
        assert!(report.kill_failures.is_empty(), "ESRCH must be swallowed");
        assert_eq!(registry.scope_count().await, 1);
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

        let report =
            cascade_bash_on_delete(&registry, &real_scope, &work_actor("owner"), None).await;
        assert!(report.live_handle_pgids.contains(&pgid));
        assert!(report.kill_failures.is_empty());
        assert_eq!(registry.scope_count().await, 1);

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
