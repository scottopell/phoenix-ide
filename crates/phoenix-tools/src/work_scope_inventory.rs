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
use phoenix_core::work_scope::{EffectiveResourceAccess, ResourceScopeKey};

use crate::bash::handle::HandleState;
use crate::bash::registry::{BashHandleRegistry, RegisteredHandle};
use crate::browser::session::{BrowserInventoryState, BrowserSessionManager};
use crate::tmux::registry::{ServerStatus, TmuxRegistry};

/// Assemble the full inventory for `work_scope` from the live registries.
///
/// Read-only: each registry is queried through its non-creating
/// `get_existing` accessor, so an inventory request for a scope that has
/// never used a resource produces empty/`None` sections rather than
/// materialising one.
pub async fn assemble_inventory(
    work_scope: &ResourceScopeKey,
    actor: Option<&EffectiveResourceAccess>,
    tmux_visible: bool,
    bash_handles: &Arc<BashHandleRegistry>,
    tmux_registry: &Arc<TmuxRegistry>,
    browser_sessions: &Arc<BrowserSessionManager>,
) -> WorkScopeInventory {
    WorkScopeInventory {
        scope_key: work_scope.stable_key(),
        bash: assemble_bash(work_scope, actor, bash_handles).await,
        tmux: if tmux_visible {
            assemble_tmux(work_scope, tmux_registry).await
        } else {
            None
        },
        browser: assemble_browser(work_scope, actor, browser_sessions).await,
        health_sampled_at: None,
        health: None,
    }
}

/// Project every handle in the scope's table (live and tombstoned) into a
/// [`BashHandleInventory`]. Empty when the scope has no handle table.
async fn assemble_bash(
    work_scope: &ResourceScopeKey,
    _actor: Option<&EffectiveResourceAccess>,
    bash_handles: &Arc<BashHandleRegistry>,
) -> Vec<BashHandleInventory> {
    let handles = bash_handles.owner_handles(work_scope).await;
    let mut out = Vec::with_capacity(handles.len());
    for registered in &handles {
        out.push(project_handle(registered).await);
    }
    out
}

async fn project_handle(registered: &RegisteredHandle) -> BashHandleInventory {
    let handle = &registered.handle;
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
            let output_bytes = live.ring.lock().await.output_bytes();
            BashHandleInventory {
                handle_id: handle.handle_id.to_string(),
                label: handle.label.clone(),
                cmd: handle.cmd.clone(),
                state: bash_state,
                pid: Some(live.pid),
                pgid: Some(live.pgid),
                started_at,
                duration_ms: None,
                exit_code: None,
                signal_number: None,
                output_bytes,
                health: None,
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
            // Raw outcome from the tombstone: success is `exit_code == Some(0)`
            // with no `signal_number`; the UI derives the ✓/✗ glyph from this.
            exit_code: tomb.exit_code,
            signal_number: tomb.signal_number,
            output_bytes: tomb.output_bytes,
            health: None,
        },
    }
}

/// Read the scope's tmux server entry (in-memory status only — no `tmux ls`
/// probe). `None` when no entry exists.
async fn assemble_tmux(
    work_scope: &ResourceScopeKey,
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

/// Project browser liveness + idle time. `None` when no session is tracked for
/// the scope. Live-session `idle_ms` is derived from the session's monotonic
/// last-activity `Instant`; teardown states report it as unavailable.
async fn assemble_browser(
    work_scope: &ResourceScopeKey,
    actor: Option<&EffectiveResourceAccess>,
    browser_sessions: &Arc<BrowserSessionManager>,
) -> Option<BrowserInventory> {
    let metadata = match actor {
        Some(access) => browser_sessions
            .inventory_metadata_for_actor(work_scope, access)
            .await
            .ok()??,
        None => browser_sessions.inventory_metadata(work_scope).await?,
    };
    let state = match metadata.state {
        BrowserInventoryState::Live => BrowserSessionLiveness::Live,
        BrowserInventoryState::TeardownPending => BrowserSessionLiveness::TeardownPending,
        BrowserInventoryState::TeardownFailed => BrowserSessionLiveness::TeardownFailed,
    };
    let idle_ms = metadata
        .idle
        .map(|idle| u64::try_from(idle.as_millis()).unwrap_or(u64::MAX));
    Some(BrowserInventory { state, idle_ms })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::handle::{Handle, HandleId, KillSignal};
    use crate::bash::ring::RING_BUFFER_BYTES;
    use std::time::SystemTime;

    fn scope() -> ResourceScopeKey {
        ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse("conv-inv").unwrap())
    }

    #[tokio::test]
    async fn empty_scope_yields_empty_inventory() {
        let bash = Arc::new(BashHandleRegistry::new());
        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();

        let inv = assemble_inventory(&scope(), None, true, &bash, &tmux, &browser).await;
        assert_eq!(inv.scope_key, scope().stable_key());
        assert!(inv.bash.is_empty());
        assert!(inv.tmux.is_none());
        assert!(inv.browser.is_none());
    }

    #[tokio::test]
    async fn live_handle_projects_running_with_pid_and_ring() {
        let bash = Arc::new(BashHandleRegistry::new());
        let handle = Handle::new_live(
            scope(),
            HandleId::new("b-1"),
            "npm run dev".into(),
            Some("dev".into()),
            4321,
            1234,
            RING_BUFFER_BYTES,
        );
        bash.register_existing_handle(&scope(), handle).await;

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), None, true, &bash, &tmux, &browser).await;

        assert_eq!(inv.bash.len(), 1);
        let h = &inv.bash[0];
        assert_eq!(h.handle_id, "b-1");
        assert_eq!(h.label.as_deref(), Some("dev"));
        assert_eq!(h.cmd, "npm run dev");
        assert_eq!(h.state, BashHandleState::Running);
        assert_eq!(h.pid, Some(1234));
        assert_eq!(h.pgid, Some(4321));
        assert!(h.duration_ms.is_none());
        // output_bytes is always present (0 at spawn, no output written yet).
        assert_eq!(h.output_bytes, 0);
    }

    #[tokio::test]
    async fn inventory_finds_handle_by_lifecycle_scope_across_control_scope() {
        use phoenix_core::work_scope::ResourceAuthority;

        let bash = Arc::new(BashHandleRegistry::new());
        bash.register_existing_handle(
            &scope(),
            Handle::new_live_for_actor_with_owner(
                ResourceScopeKey::Coordinator,
                HandleId::new("b-coordinator"),
                "coordinator".to_string(),
                ResourceAuthority::Work,
                "git status".to_string(),
                None,
                4321,
                1234,
                RING_BUFFER_BYTES,
            ),
        )
        .await;
        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();

        let inventory = assemble_inventory(&scope(), None, true, &bash, &tmux, &browser).await;

        assert_eq!(inventory.bash.len(), 1);
        assert_eq!(inventory.bash[0].handle_id, "b-coordinator");
    }

    #[tokio::test]
    async fn owner_inventory_includes_all_owned_handles() {
        use phoenix_core::work_scope::ResourceAuthority;

        let bash = Arc::new(BashHandleRegistry::new());
        for actor in ["sibling-a", "sibling-b"] {
            bash.register_existing_handle(
                &scope(),
                Handle::new_live_for_actor(
                    scope(),
                    HandleId::new(format!("b-{actor}")),
                    actor.to_string(),
                    ResourceAuthority::Restricted,
                    format!("secret-{actor}"),
                    None,
                    1,
                    1,
                    RING_BUFFER_BYTES,
                ),
            )
            .await;
        }
        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let actor = EffectiveResourceAccess::new("sibling-a", ResourceAuthority::Restricted);

        let inventory =
            assemble_inventory(&scope(), Some(&actor), false, &bash, &tmux, &browser).await;

        assert_eq!(inventory.bash.len(), 2);
        let commands = inventory
            .bash
            .iter()
            .map(|handle| handle.cmd.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            commands,
            std::collections::HashSet::from(["secret-sibling-a", "secret-sibling-b"])
        );
    }

    #[tokio::test]
    async fn kill_pending_handle_projects_kill_pending_kernel() {
        let bash = Arc::new(BashHandleRegistry::new());
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
        bash.register_existing_handle(&scope(), handle).await;

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), None, true, &bash, &tmux, &browser).await;
        assert_eq!(inv.bash[0].state, BashHandleState::KillPendingKernel);
    }

    #[tokio::test]
    async fn tombstoned_handle_projects_terminal_fields() {
        let bash = Arc::new(BashHandleRegistry::new());
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
                std::time::SystemTime::now(),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
        bash.register_existing_handle(&scope(), handle).await;

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), None, true, &bash, &tmux, &browser).await;
        let h = &inv.bash[0];
        assert_eq!(h.state, BashHandleState::Tombstoned);
        assert_eq!(h.duration_ms, Some(7));
        assert!(h.pid.is_none());
        // A clean exit projects the success outcome: exit_code 0, no signal.
        assert_eq!(h.exit_code, Some(0));
        assert!(h.signal_number.is_none());
        // output_bytes persists into the tombstone; no output was written
        // here, so the snapshotted total is 0 (present, not absent).
        assert_eq!(h.output_bytes, 0);
    }

    #[tokio::test]
    async fn tombstoned_killed_handle_projects_signal_outcome() {
        let bash = Arc::new(BashHandleRegistry::new());
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
            .transition_to_terminal(
                crate::bash::handle::FinalCause::Killed {
                    exit_code: None,
                    signal_number: Some(9),
                },
                std::time::Duration::from_millis(3),
                std::time::SystemTime::now(),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
        bash.register_existing_handle(&scope(), handle).await;

        let tmux = Arc::new(TmuxRegistry::with_socket_dir(
            "/tmp/phoenix-inv-test".into(),
        ));
        let browser = BrowserSessionManager::new();
        let inv = assemble_inventory(&scope(), None, true, &bash, &tmux, &browser).await;
        let h = &inv.bash[0];
        assert_eq!(h.state, BashHandleState::Tombstoned);
        // A signal kill projects the failure outcome: a recorded signal, no
        // success exit code.
        assert_eq!(h.signal_number, Some(9));
        assert!(h.exit_code.is_none());
    }
}
