//! Assembler for [`BashHandleInspection`] — the per-handle drill-down
//! read-projection (`specs/process-inspector/`).
//!
//! Parallels [`crate::work_scope_inventory::assemble_inventory`]: it is a
//! read-only path over the in-memory bash handle registry. It resolves a
//! single handle by `(WorkScope, handle_id)`, projects its identity, state,
//! and an output window (delegating to the existing ring/tombstone read
//! helpers the bash tool uses), and reports the live `pgid` so the caller can
//! attach a request-time resource sample.
//!
//! The resource sample itself is **not** assembled here: it requires
//! `sysinfo` (a `phoenix-ide` dependency) and the platform-specific
//! process-group reads, which live in the `api` layer's `process_sample`
//! module. This assembler reports `resources: None` and the live `pgid`; the
//! handler fills `resources` when the handle is live (REQ-PINSP-004).

use std::sync::Arc;

use chrono::{DateTime, Utc};

use phoenix_core::domain::process_inspection::BashHandleInspection;
use phoenix_core::domain::work_scope_inventory::BashHandleState;
use phoenix_core::work_scope::WorkScope;

use crate::bash::handle::{Handle, HandleId, HandleState};
use crate::bash::operations::{
    read_window_from_ring, read_window_from_tombstone, window_to_typed, ReadArgs, RingRead,
};
use crate::bash::registry::BashHandleRegistry;

/// Outcome of resolving + projecting one handle for inspection.
///
/// `inspection` carries identity, state, and the output window with
/// `resources: None`. `live_pgid` is `Some(pgid)` exactly when the handle is
/// live (running or `kill_pending_kernel`) — the signal to the caller that a
/// request-time resource sample over that process group should be attached.
/// `None` for a terminal handle (no process group to sample).
#[derive(Debug)]
pub struct InspectionAssembly {
    pub inspection: BashHandleInspection,
    pub live_pgid: Option<i32>,
}

/// Resolve `handle_id` within `work_scope` and project its inspection
/// snapshot for the given incremental `since` offset.
///
/// Read-only: the registry is queried through its non-creating `get_existing`
/// accessor and the table's `get` lookup, so inspecting a handle never
/// allocates a handle table. Returns `None` when the scope has no handle
/// table or the handle id is absent — a not-found condition (REQ-PINSP-001).
pub async fn assemble_inspection(
    work_scope: &WorkScope,
    handle_id: &str,
    since: Option<u64>,
    bash_handles: &Arc<BashHandleRegistry>,
) -> Option<InspectionAssembly> {
    let table = bash_handles.get_existing(work_scope).await?;
    let handle = table
        .read()
        .await
        .get(&HandleId::new(handle_id.to_string()))?;
    Some(project_inspection(&handle, since).await)
}

/// Project one resolved handle into an [`InspectionAssembly`] for the given
/// `since` offset. The output window delegates to the same ring/tombstone
/// read helpers the bash peek uses (`read_window_from_ring` /
/// `read_window_from_tombstone` + `window_to_typed`).
async fn project_inspection(handle: &Arc<Handle>, since: Option<u64>) -> InspectionAssembly {
    let started_at: DateTime<Utc> = handle.started_at.into();
    let read_args = ReadArgs::from_since(since);
    let state_arc = handle.state().await;

    match state_arc.as_ref() {
        HandleState::Live(live) => {
            // `Live` discriminates `running` vs `kill_pending_kernel` by the
            // presence of a recorded kill attempt — the same rule
            // `project_handle` uses in `work_scope_inventory`.
            let state = if handle.kill_attempt().await.is_some() {
                BashHandleState::KillPendingKernel
            } else {
                BashHandleState::Running
            };
            let output = {
                let ring = live.ring.lock().await;
                let RingRead { view, partial } = read_window_from_ring(&ring, &read_args);
                window_to_typed(&view, partial)
            };
            InspectionAssembly {
                inspection: BashHandleInspection {
                    handle_id: handle.handle_id.to_string(),
                    label: handle.label.clone(),
                    cmd: handle.cmd.clone(),
                    state,
                    pid: Some(live.pid),
                    pgid: Some(live.pgid),
                    started_at,
                    exit_code: None,
                    signal_number: None,
                    duration_ms: None,
                    output,
                    // Filled by the caller for live handles (REQ-PINSP-004).
                    resources: None,
                },
                live_pgid: Some(live.pgid),
            }
        }
        HandleState::Tombstoned(tomb) => {
            let output = window_to_typed(&read_window_from_tombstone(tomb, &read_args), None);
            InspectionAssembly {
                inspection: BashHandleInspection {
                    handle_id: handle.handle_id.to_string(),
                    label: handle.label.clone(),
                    cmd: handle.cmd.clone(),
                    state: BashHandleState::Tombstoned,
                    pid: None,
                    pgid: None,
                    started_at,
                    exit_code: tomb.exit_code,
                    signal_number: tomb.signal_number,
                    duration_ms: Some(tomb.duration_ms),
                    output,
                    resources: None,
                },
                live_pgid: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::handle::{FinalCause, Handle, HandleId, KillSignal};
    use crate::bash::ring::RING_BUFFER_BYTES;
    use std::time::{Duration, SystemTime};

    fn scope() -> WorkScope {
        WorkScope::Conversation("conv-inspect".into())
    }

    #[tokio::test]
    async fn unknown_scope_or_handle_is_none() {
        let bash = Arc::new(BashHandleRegistry::new());
        // No table at all.
        assert!(assemble_inspection(&scope(), "b-1", None, &bash)
            .await
            .is_none());
        // Table exists but handle absent.
        let _ = bash.get_or_create(&scope()).await;
        assert!(assemble_inspection(&scope(), "b-999", None, &bash)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn live_handle_reports_identity_state_and_live_pgid() {
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

        let assembly = assemble_inspection(&scope(), "b-1", None, &bash)
            .await
            .expect("inspection");
        let inv = &assembly.inspection;
        assert_eq!(inv.handle_id, "b-1");
        assert_eq!(inv.label.as_deref(), Some("dev"));
        assert_eq!(inv.cmd, "npm run dev");
        assert_eq!(inv.state, BashHandleState::Running);
        assert_eq!(inv.pid, Some(1234));
        assert_eq!(inv.pgid, Some(4321));
        assert!(inv.duration_ms.is_none());
        assert!(
            inv.resources.is_none(),
            "assembler leaves resources for caller"
        );
        assert_eq!(assembly.live_pgid, Some(4321));
    }

    #[tokio::test]
    async fn kill_pending_handle_projects_kill_pending_kernel_and_live_pgid() {
        let bash = Arc::new(BashHandleRegistry::new());
        let table = bash.get_or_create(&scope()).await;
        let handle = Handle::new_live(
            scope(),
            HandleId::new("b-1"),
            "sleep 99".into(),
            None,
            55,
            55,
            RING_BUFFER_BYTES,
        );
        handle
            .mark_kill_pending_kernel(KillSignal::Term, SystemTime::now())
            .await;
        table.write().await.insert(handle);

        let assembly = assemble_inspection(&scope(), "b-1", None, &bash)
            .await
            .expect("inspection");
        assert_eq!(
            assembly.inspection.state,
            BashHandleState::KillPendingKernel
        );
        assert_eq!(assembly.live_pgid, Some(55));
    }

    #[tokio::test]
    async fn tombstoned_handle_serves_terminal_fields_and_no_live_pgid() {
        let bash = Arc::new(BashHandleRegistry::new());
        let table = bash.get_or_create(&scope()).await;
        let handle = Handle::new_live(
            scope(),
            HandleId::new("b-1"),
            "false".into(),
            None,
            7,
            7,
            RING_BUFFER_BYTES,
        );
        handle
            .transition_to_terminal(
                FinalCause::Killed {
                    exit_code: None,
                    signal_number: Some(9),
                },
                Duration::from_millis(13),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
        table.write().await.insert(handle);

        let assembly = assemble_inspection(&scope(), "b-1", None, &bash)
            .await
            .expect("inspection");
        let inv = &assembly.inspection;
        assert_eq!(inv.state, BashHandleState::Tombstoned);
        assert_eq!(inv.duration_ms, Some(13));
        assert_eq!(inv.signal_number, Some(9));
        assert!(inv.exit_code.is_none());
        assert!(inv.pid.is_none());
        assert!(inv.pgid.is_none());
        assert!(assembly.live_pgid.is_none());
    }
}
