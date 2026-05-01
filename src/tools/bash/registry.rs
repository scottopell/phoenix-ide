//! In-memory bash handle registry.
//!
//! REQ-BASH-005 (per-conversation cap), REQ-BASH-006 (in-memory tombstones,
//! no `SQLite` shadow store), REQ-BASH-014 (per-conversation registry).
//!
//! Some methods (e.g. `remove`, `with_caps`, ring/handle cap accessors)
//! are surface that task 02694 (`BashTool` dispatch) and task 02696
//! (hard-delete cascade) consume; until then they read as dead.
#![allow(dead_code)]
//!
//! Lifetime: registries live in process memory only. A Phoenix restart
//! drops them and any handles they hold; agents see `handle_not_found` on
//! a previously-known handle (matching the spec's "handles do NOT survive
//! Phoenix restart" guarantee).
//!
//! Lock ordering for cap enforcement and spawn (consumed by task 02694's
//! `BashTool::spawn`): acquire the registry's `RwLock<HashMap>` for read,
//! then the conversation's `RwLock<ConversationHandles>` for write. The
//! conversation lock holds for the duration of cap-check + handle insert
//! to prevent two concurrent spawns from both observing
//! `count == cap - 1` and racing past the cap.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use thiserror::Error;
use tokio::sync::RwLock;

use super::handle::{Handle, HandleId};
use super::ring::RING_BUFFER_BYTES;

/// Per-conversation cap on `running` handles (REQ-BASH-005:
/// `LIVE_HANDLE_CAP`).
pub const LIVE_HANDLE_CAP: usize = 8;

/// Errors surfaced by the registry. `BashTool` translates these into the
/// stable error envelope on the agent's response.
#[derive(Debug, Error)]
pub enum BashHandleError {
    /// REQ-BASH-005: spawn rejected because the conversation has hit
    /// `LIVE_HANDLE_CAP` live handles.
    #[error("conversation has reached the cap of {cap} live bash handles")]
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
    pub age_seconds: u64,
}

/// Per-conversation handle table. Tracks live handles (for cap enforcement
/// and lookup) and tombstones (so peek/wait/kill on an exited handle still
/// resolves until the conversation is hard-deleted or Phoenix restarts).
///
/// The unified `handles` map covers both live and tombstoned entries;
/// the discrimination is made by inspecting the handle's `HandleState`.
/// This keeps the lookup path single-source — a handle that transitions
/// from `Live` to `Tombstoned` is the SAME `Arc<Handle>` (its `state`
/// field swaps), and lookup never has to "follow" between two maps.
#[derive(Debug, Default)]
pub struct ConversationHandles {
    /// Next sequential handle index for this conversation (`b-1`, `b-2`, ...).
    next_id: u64,
    /// All handles, by id. Live and tombstoned alike.
    handles: HashMap<HandleId, Arc<Handle>>,
}

impl ConversationHandles {
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

    /// Remove a handle entirely (live or tombstoned). Used by the
    /// hard-delete cascade.
    pub fn remove(&mut self, id: &HandleId) -> Option<Arc<Handle>> {
        self.handles.remove(id)
    }

    /// REQ-BASH-005: enforce the cap before allocating a new handle id /
    /// spawning a process. If the cap is reached, returns
    /// [`BashHandleError::HandleCapReached`] populated with the current
    /// live-handle summary so the agent can decide what to kill or wait on.
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

/// Top-level registry: maps `conversation_id` -> per-conversation handle table.
///
/// One registry instance per Phoenix process. Owned by the runtime layer
/// and reached by tools through `ToolContext::bash_handles()`.
#[derive(Debug, Default)]
pub struct BashHandleRegistry {
    inner: RwLock<HashMap<String, Arc<RwLock<ConversationHandles>>>>,
    /// Per-handle ring byte cap. Defaults to [`RING_BUFFER_BYTES`]; tests
    /// override to small values to exercise eviction.
    ring_bytes_cap: usize,
    /// Per-conversation live-handle cap. Defaults to [`LIVE_HANDLE_CAP`];
    /// tests override to small values to exercise rejection.
    live_handle_cap: usize,
}

impl BashHandleRegistry {
    /// Create a registry with default caps.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            ring_bytes_cap: RING_BUFFER_BYTES,
            live_handle_cap: LIVE_HANDLE_CAP,
        }
    }

    /// Test-only: build a registry with custom caps.
    #[cfg(test)]
    pub fn with_caps(ring_bytes_cap: usize, live_handle_cap: usize) -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            ring_bytes_cap,
            live_handle_cap,
        }
    }

    /// Configured ring byte cap for live handles in this registry.
    pub fn ring_bytes_cap(&self) -> usize {
        self.ring_bytes_cap
    }

    /// Configured live-handle cap.
    pub fn live_handle_cap(&self) -> usize {
        self.live_handle_cap
    }

    /// Get-or-create the per-conversation handle table. Matches the
    /// `BrowserSessionManager::get_session` pattern — returns the same
    /// `Arc<RwLock<ConversationHandles>>` for repeated calls with the
    /// same conversation id.
    pub async fn get_or_create(&self, conversation_id: &str) -> Arc<RwLock<ConversationHandles>> {
        // Fast path: read-lock and return existing entry.
        {
            let map = self.inner.read().await;
            if let Some(entry) = map.get(conversation_id) {
                return entry.clone();
            }
        }
        // Slow path: write-lock to create. Re-check under the lock to
        // avoid clobbering a concurrent creator.
        let mut map = self.inner.write().await;
        if let Some(entry) = map.get(conversation_id) {
            return entry.clone();
        }
        let entry = Arc::new(RwLock::new(ConversationHandles::new()));
        map.insert(conversation_id.to_string(), entry.clone());
        entry
    }

    /// Snapshot live process-group ids across ALL conversations, for the
    /// shutdown kill-tree pass. Acquires read locks; callers must NOT
    /// hold any conversation lock while invoking this.
    ///
    /// REQ-BASH-007: walks live handles for the `shutdown_kill_tree` pass.
    pub async fn snapshot_live_pgids(&self) -> Vec<i32> {
        let mut out = Vec::new();
        let map = self.inner.read().await;
        for entry in map.values() {
            let convs = entry.read().await;
            for h in convs.all() {
                if let Some(pgid) = h.live_pgid().await {
                    out.push(pgid);
                }
            }
        }
        out
    }

    /// Remove a conversation's handle table outright. Used by the
    /// hard-delete cascade (REQ-BASH-006). Returns the removed entry so
    /// the caller can SIGKILL its live process groups synchronously.
    pub async fn remove_conversation(
        &self,
        conversation_id: &str,
    ) -> Option<Arc<RwLock<ConversationHandles>>> {
        let mut map = self.inner.write().await;
        map.remove(conversation_id)
    }

    /// Number of conversations currently tracked. Test/diagnostic only.
    #[cfg(test)]
    pub async fn conversation_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash::handle::{FinalCause, Handle};

    fn make_handle(conv: &str, id: &str, ring_bytes_cap: usize) -> Arc<Handle> {
        Handle::new_live(
            conv.to_string(),
            HandleId::new(id),
            format!("cmd for {id}"),
            12345,
            12345,
            ring_bytes_cap,
        )
    }

    #[tokio::test]
    async fn allocate_handle_id_is_sequential_per_conversation() {
        let mut convs = ConversationHandles::new();
        assert_eq!(convs.allocate_handle_id().as_str(), "b-1");
        assert_eq!(convs.allocate_handle_id().as_str(), "b-2");
        assert_eq!(convs.allocate_handle_id().as_str(), "b-3");
    }

    #[tokio::test]
    async fn allocate_handle_id_independent_across_conversations() {
        let registry = BashHandleRegistry::new();
        let conv_a = registry.get_or_create("conv-a").await;
        let conv_b = registry.get_or_create("conv-b").await;
        assert_eq!(conv_a.write().await.allocate_handle_id().as_str(), "b-1");
        assert_eq!(conv_b.write().await.allocate_handle_id().as_str(), "b-1");
        assert_eq!(conv_a.write().await.allocate_handle_id().as_str(), "b-2");
    }

    #[tokio::test]
    async fn cap_rejects_when_live_count_at_cap() {
        let registry = BashHandleRegistry::with_caps(RING_BUFFER_BYTES, 2);
        let convs = registry.get_or_create("conv-1").await;
        let mut guard = convs.write().await;
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
        let convs = registry.get_or_create("conv-1").await;
        let mut guard = convs.write().await;
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
        let convs = registry.get_or_create("conv-1").await;
        let mut guard = convs.write().await;
        let h = guard.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));

        // Cap is 1; live_count == 1 right now → reject.
        assert!(guard.check_cap(1).await.is_err());

        // Demote the handle to tombstoned (process exited).
        let did_transition = h
            .transition_to_terminal(
                FinalCause::Exited { exit_code: Some(0) },
                std::time::Duration::from_millis(1),
                crate::tools::bash::handle::TOMBSTONE_TAIL_LINES,
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
        let convs = registry.get_or_create("conv-1").await;
        let guard = convs.read().await;
        // Empty conversation: live_count = 0; cap = 8 → ok.
        assert!(guard.check_cap(8).await.is_ok());
    }

    #[tokio::test]
    async fn get_or_create_returns_same_arc_for_same_conversation() {
        let registry = BashHandleRegistry::new();
        let a = registry.get_or_create("conv-1").await;
        let b = registry.get_or_create("conv-1").await;
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn snapshot_live_pgids_collects_across_conversations() {
        let registry = BashHandleRegistry::new();
        let conv_a = registry.get_or_create("conv-a").await;
        let conv_b = registry.get_or_create("conv-b").await;
        {
            let mut g = conv_a.write().await;
            let mut h = make_handle("conv-a", "b-1", RING_BUFFER_BYTES);
            // Override pgid via construction — we built it with 12345 in
            // make_handle. Just use that.
            let _ = Arc::get_mut(&mut h); // ensure no aliasing for the assertion below
            g.insert(h);
        }
        {
            let mut g = conv_b.write().await;
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
        let convs = registry.get_or_create("conv-1").await;
        let h = {
            let mut g = convs.write().await;
            g.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES))
        };
        h.transition_to_terminal(
            FinalCause::Exited { exit_code: Some(0) },
            std::time::Duration::from_millis(1),
            crate::tools::bash::handle::TOMBSTONE_TAIL_LINES,
        )
        .await;
        let pgids = registry.snapshot_live_pgids().await;
        assert!(
            pgids.is_empty(),
            "tombstoned handles must not appear in live pgid snapshot"
        );
    }

    #[tokio::test]
    async fn remove_conversation_returns_entry() {
        let registry = BashHandleRegistry::new();
        let _ = registry.get_or_create("conv-1").await;
        assert_eq!(registry.conversation_count().await, 1);
        assert!(registry.remove_conversation("conv-1").await.is_some());
        assert_eq!(registry.conversation_count().await, 0);
        assert!(registry.remove_conversation("conv-1").await.is_none());
    }
}
