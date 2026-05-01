//! In-memory bash handle registry.
//!
//! REQ-BASH-005 (per-conversation cap), REQ-BASH-006 (in-memory tombstones,
//! no `SQLite` shadow store), REQ-BASH-014 (per-conversation registry).
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

    /// Remove a handle entirely (live or tombstoned). Granular complement
    /// to `BashHandleRegistry::remove_conversation`; not currently used
    /// by the hard-delete cascade (which removes the whole conversation
    /// table) but kept on the API surface for surgical removal flows.
    #[allow(dead_code)]
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

/// Best-effort cascade outcome for the hard-delete orchestrator
/// (REQ-BASH-006). Failure surfaces as a structured record the
/// orchestrator logs at WARN; nothing here is fatal — the conversation
/// row is removed regardless. The orchestrator already knows the
/// `conv_id` (it's the path parameter), so it is not duplicated here.
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

/// Run the bash side of the hard-delete cascade for `conversation_id`
/// (REQ-BASH-006). Atomically:
///
///   1. Removes the conversation's handle table from the registry — any
///      subsequent tool call for this conversation will see "no handle
///      table" and produce `handle_not_found`, which is the correct
///      behaviour for a deleted conversation.
///   2. Snapshots live pgid / pid / `kill_pending_kernel` state across the
///      removed handles.
///   3. Sends `SIGKILL` to each live process group via
///      `kill(-pgid, SIGKILL)` (catches immediate descendants per
///      REQ-BASH-007's setpgid contract).
///
/// The Allium `HandlesRemovedByConversationDelete` rule's post-condition
/// (`not exists Handle{conversation: c}`) is satisfied by step 1 alone:
/// the live handles drop with the registry entry. Step 3 satisfies the
/// `KillSignalSentForAllLiveHandles` ensures clause; failures populate
/// `kill_failures` for the orchestrator's WARN log but are not fatal —
/// the spec's policy is "log and continue" (REQ-BED-032).
///
/// SIGKILL rather than SIGTERM: hard-delete deletes the conversation
/// outright, so no agent is left to observe a graceful close. Same
/// rationale as `shutdown_kill_tree` in [`super::reaper`].
pub async fn cascade_bash_on_delete(
    registry: &Arc<BashHandleRegistry>,
    conversation_id: &str,
) -> CascadeBashReport {
    let mut report = CascadeBashReport::default();

    let Some(entry) = registry.remove_conversation(conversation_id).await else {
        return report;
    };

    let convs = entry.read().await;
    for h in convs.all() {
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

    #[tokio::test]
    async fn cascade_bash_on_delete_no_entry_is_clean() {
        let registry = Arc::new(BashHandleRegistry::new());
        let report = cascade_bash_on_delete(&registry, "never-existed").await;
        assert!(report.kill_failures.is_empty());
        assert!(report.live_handle_pgids.is_empty());
        assert!(report.live_handle_pids.is_empty());
        assert!(report.kill_pending_kernel_pids.is_empty());
    }

    #[tokio::test]
    async fn cascade_bash_on_delete_tombstoned_only_is_clean() {
        let registry = Arc::new(BashHandleRegistry::new());
        let convs = registry.get_or_create("conv-1").await;
        let h = {
            let mut g = convs.write().await;
            g.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES))
        };
        // Demote so there are no live handles to kill.
        h.transition_to_terminal(
            FinalCause::Exited { exit_code: Some(0) },
            std::time::Duration::from_millis(1),
            crate::tools::bash::handle::TOMBSTONE_TAIL_LINES,
        )
        .await;

        let report = cascade_bash_on_delete(&registry, "conv-1").await;
        assert!(report.kill_failures.is_empty());
        assert!(report.live_handle_pgids.is_empty());
        // Registry entry is gone after cascade.
        assert_eq!(registry.conversation_count().await, 0);
    }

    #[tokio::test]
    async fn cascade_bash_on_delete_records_live_pgids_and_drops_entry() {
        // The fake handle uses pgid 12345 (a process group that almost
        // certainly does not exist on the test host). `kill(-12345, …)`
        // will return ESRCH, which the cascade swallows — so this test
        // verifies the bookkeeping side: the pgid is recorded in the
        // report and the registry entry is removed.
        let registry = Arc::new(BashHandleRegistry::new());
        let convs = registry.get_or_create("conv-1").await;
        {
            let mut g = convs.write().await;
            g.insert(make_handle("conv-1", "b-1", RING_BUFFER_BYTES));
            g.insert(make_handle("conv-1", "b-2", RING_BUFFER_BYTES));
        }

        let report = cascade_bash_on_delete(&registry, "conv-1").await;
        assert_eq!(report.live_handle_pgids.len(), 2);
        assert!(report.live_handle_pgids.iter().all(|&p| p == 12345));
        assert!(report.kill_failures.is_empty(), "ESRCH must be swallowed");
        assert_eq!(registry.conversation_count().await, 0);
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
        let convs = registry.get_or_create("conv-real").await;
        {
            let mut g = convs.write().await;
            let h = Handle::new_live(
                "conv-real".to_string(),
                HandleId::new("b-1"),
                "sleep 60".to_string(),
                pgid,
                pid,
                RING_BUFFER_BYTES,
            );
            g.insert(h);
        }

        let report = cascade_bash_on_delete(&registry, "conv-real").await;
        assert!(report.live_handle_pgids.contains(&pgid));
        assert!(report.kill_failures.is_empty());
        assert_eq!(registry.conversation_count().await, 0);

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
