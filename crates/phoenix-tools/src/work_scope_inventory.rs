//! Assembler for [`WorkScopeInventory`] — the read-projection over the three
//! work-affine registries (bash handles, tmux servers, browser sessions).
//!
//! The assembler is **read-only**: it uses the non-creating `get_existing`
//! accessors on each registry so observing a scope never allocates a handle
//! table, spawns a tmux probe, or launches Chrome. A scope with no resources
//! yields an inventory with an empty `bash` vector and `None` for both `tmux`
//! and `browser` (see `specs/work-scope-ui/design.md`).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use phoenix_core::domain::work_scope_inventory::{
    BashHandleInventory, BashHandleState, BrowserInventory, BrowserSessionLiveness, TmuxInventory,
    TmuxServerStatus, WorkScopeInventory,
};
use phoenix_core::work_scope::WorkScope;

use crate::bash::handle::{Handle, HandleState};
use crate::bash::registry::BashHandleRegistry;
use crate::browser::session::BrowserSessionManager;
use crate::tmux::registry::{ServerStatus, TmuxRegistry};

/// Assemble the full inventory for `work_scope` from the live registries.
///
/// Read-only: each registry is queried through its non-creating
/// `get_existing` accessor, so an inventory request for a scope that has
/// never used a resource produces empty/`None` sections rather than
/// materialising one.
pub async fn assemble_inventory(
    work_scope: &WorkScope,
    bash_handles: &Arc<BashHandleRegistry>,
    tmux_registry: &Arc<TmuxRegistry>,
    browser_sessions: &Arc<BrowserSessionManager>,
) -> WorkScopeInventory {
    WorkScopeInventory {
        scope_key: work_scope.stable_key(),
        bash: assemble_bash(work_scope, bash_handles).await,
        tmux: assemble_tmux(work_scope, tmux_registry).await,
        browser: assemble_browser(work_scope, browser_sessions).await,
    }
}

/// Project every handle in the scope's table (live and tombstoned) into a
/// [`BashHandleInventory`]. Empty when the scope has no handle table.
async fn assemble_bash(
    work_scope: &WorkScope,
    bash_handles: &Arc<BashHandleRegistry>,
) -> Vec<BashHandleInventory> {
    let Some(table) = bash_handles.get_existing(work_scope).await else {
        return Vec::new();
    };
    let table = table.read().await;
    let mut out = Vec::new();
    for handle in table.all() {
        out.push(project_handle(handle).await);
    }
    out
}

async fn project_handle(handle: &Arc<Handle>) -> BashHandleInventory {
    let started_at: DateTime<Utc> = handle.started_at.into();
    let state_arc = handle.state().await;
    match state_arc.as_ref() {
        HandleState::Live(live) => {
            // `Live` discriminates `running` vs `kill_pending_kernel` by the
            // presence of a recorded kill attempt — the same rule the bash
            // tool uses for the wire `status`.
            let bash_state = if handle.kill_attempt().await.is_some() {
                BashHandleState::KillPendingKernel
            } else {
                BashHandleState::Running
            };
            let ring_bytes_used = u64::try_from(live.ring.lock().await.bytes_used()).ok();
            BashHandleInventory {
                handle_id: handle.handle_id.to_string(),
                label: handle.label.clone(),
                cmd: handle.cmd.clone(),
                state: bash_state,
                pid: Some(live.pid),
                pgid: Some(live.pgid),
                started_at,
                duration_ms: None,
                ring_bytes_used,
            }
        }
        HandleState::Tombstoned(tomb) => BashHandleInventory {
            handle_id: handle.handle_id.to_string(),
            label: handle.label.clone(),
            cmd: handle.cmd.clone(),
            state: BashHandleState::Tombstoned,
            pid: None,
            pgid: None,
            started_at,
            duration_ms: Some(tomb.duration_ms),
            ring_bytes_used: None,
        },
    }
}

/// Read the scope's tmux server entry (in-memory status only — no `tmux ls`
/// probe). `None` when no entry exists.
async fn assemble_tmux(
    work_scope: &WorkScope,
    tmux_registry: &Arc<TmuxRegistry>,
) -> Option<TmuxInventory> {
    let entry = tmux_registry.get_existing(work_scope).await?;
    let server = entry.read().await;
    Some(TmuxInventory {
        status: project_tmux_status(server.status),
    })
}

fn project_tmux_status(status: ServerStatus) -> TmuxServerStatus {
    match status {
        ServerStatus::NotProbed => TmuxServerStatus::NotProbed,
        ServerStatus::Live => TmuxServerStatus::Live,
        ServerStatus::Gone => TmuxServerStatus::Gone,
    }
}

/// Project browser liveness + idle time. `None` when no session is live for
/// the scope. `idle_ms` is derived from the session's monotonic last-activity
/// `Instant` at assembly time.
async fn assemble_browser(
    work_scope: &WorkScope,
    browser_sessions: &Arc<BrowserSessionManager>,
) -> Option<BrowserInventory> {
    let session = browser_sessions.get_existing(work_scope).await?;
    let last_activity = session.read().await.last_activity;
    let idle_ms = u64::try_from(last_activity.elapsed().as_millis()).unwrap_or(u64::MAX);
    Some(BrowserInventory {
        state: BrowserSessionLiveness::Live,
        idle_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::handle::{Handle, HandleId, KillSignal};
    use crate::bash::ring::RING_BUFFER_BYTES;
    use std::time::SystemTime;

    fn scope() -> WorkScope {
        WorkScope::Conversation("conv-inv".into())
    }

    #[tokio::test]
    async fn empty_scope_yields_empty_inventory() {
        let bash = Arc::new(BashHandleRegistry::new());
        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();

        let inv = assemble_inventory(&scope(), &bash, &tmux, &browser).await;
        assert_eq!(inv.scope_key, scope().stable_key());
        assert!(inv.bash.is_empty());
        assert!(inv.tmux.is_none());
        assert!(inv.browser.is_none());
    }

    #[tokio::test]
    async fn live_handle_projects_running_with_pid_and_ring() {
        let bash = Arc::new(BashHandleRegistry::new());
        let table = bash.get_or_create(&scope()).await;
        let handle = Handle::new_live(
            scope(),
            HandleId::new("b-1"),
            "npm run dev".into(),
            Some("dev".into()),
            4321,
            1234,
            RING_BUFFER_BYTES,
        );
        table.write().await.insert(handle);

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), &bash, &tmux, &browser).await;

        assert_eq!(inv.bash.len(), 1);
        let h = &inv.bash[0];
        assert_eq!(h.handle_id, "b-1");
        assert_eq!(h.label.as_deref(), Some("dev"));
        assert_eq!(h.cmd, "npm run dev");
        assert_eq!(h.state, BashHandleState::Running);
        assert_eq!(h.pid, Some(1234));
        assert_eq!(h.pgid, Some(4321));
        assert!(h.duration_ms.is_none());
        assert!(h.ring_bytes_used.is_some());
    }

    #[tokio::test]
    async fn kill_pending_handle_projects_kill_pending_kernel() {
        let bash = Arc::new(BashHandleRegistry::new());
        let table = bash.get_or_create(&scope()).await;
        let handle = Handle::new_live(
            scope(),
            HandleId::new("b-1"),
            "sleep 99".into(),
            None,
            1,
            1,
            RING_BUFFER_BYTES,
        );
        handle
            .mark_kill_pending_kernel(KillSignal::Term, SystemTime::now())
            .await;
        table.write().await.insert(handle);

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), &bash, &tmux, &browser).await;
        assert_eq!(inv.bash[0].state, BashHandleState::KillPendingKernel);
    }

    #[tokio::test]
    async fn tombstoned_handle_projects_terminal_fields() {
        let bash = Arc::new(BashHandleRegistry::new());
        let table = bash.get_or_create(&scope()).await;
        let handle = Handle::new_live(
            scope(),
            HandleId::new("b-1"),
            "true".into(),
            None,
            1,
            1,
            RING_BUFFER_BYTES,
        );
        handle
            .transition_to_terminal(
                crate::bash::handle::FinalCause::Exited { exit_code: Some(0) },
                std::time::Duration::from_millis(7),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
        table.write().await.insert(handle);

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), &bash, &tmux, &browser).await;
        let h = &inv.bash[0];
        assert_eq!(h.state, BashHandleState::Tombstoned);
        assert_eq!(h.duration_ms, Some(7));
        assert!(h.pid.is_none());
        assert!(h.ring_bytes_used.is_none());
    }
}
