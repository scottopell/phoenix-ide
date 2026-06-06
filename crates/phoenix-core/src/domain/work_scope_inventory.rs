//! Work-scope observability inventory wire types.
//!
//! `WorkScopeInventory` is the read-projection of the three in-memory
//! work-affine registries — the `WorkScope`-keyed bash handle registry, the
//! tmux registry, and the browser session manager — surfaced over the pull
//! endpoint `GET /api/work-scope/:scope_key/inventory` (see
//! `specs/work-scope-ui/`). It is assembled on demand and never stored;
//! there is exactly one carrier of resource state at each layer (the
//! registries own it, this is the wire projection).
//!
//! These types live in the base crate alongside the other domain/wire types
//! (`tool_wire`, etc.) so the `tools` and `api` layers depend *down* onto a
//! common vocabulary. The assembler that fills these from the live registries
//! lives in the `tools` layer, which has access to the registry types.

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

/// Full inventory of work-affine resources owned by one `WorkScope`.
///
/// Carries the **complete** snapshot — the pull endpoint returns it whole and
/// the (separately-specified) push variant re-broadcasts it whole, so there is
/// no partial-state reconciliation bug class.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct WorkScopeInventory {
    /// `WorkScope::stable_key()` of the scope this inventory describes.
    pub scope_key: String,
    /// Bash handles (live and tombstoned) registered for this scope. Empty
    /// when the scope has no handle table.
    pub bash: Vec<BashHandleInventory>,
    /// The per-scope tmux server entry, or `None` when no entry exists
    /// (the scope's tools never reached a tmux operation).
    pub tmux: Option<TmuxInventory>,
    /// The browser session, or `None` when no session is live for this scope.
    pub browser: Option<BrowserInventory>,
}

/// Lifecycle status of one bash handle, projected from `HandleState` +
/// the handle's kill-attempt bookkeeping.
///
/// `Live` splits into `running` vs `kill_pending_kernel` by the presence of a
/// recorded `KillAttempt`; `Tombstoned` maps to `tombstoned`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum BashHandleState {
    /// `Live` with no kill in flight.
    Running,
    /// `Live` with a recorded kill attempt the kernel has not yet honored.
    KillPendingKernel,
    /// Terminal — exited or killed.
    Tombstoned,
}

/// One bash handle's observability projection.
///
/// `pid` and `pgid` exist only while the handle is live; `duration_ms` exists
/// only once it is terminal. Each is skipped on the wire when absent so the
/// TS side sees an optional field. `output_bytes` is ALWAYS present — total
/// bytes the process has written is defined in every state (0 at spawn), and
/// it survives the tombstone transition (snapshotted at demotion).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashHandleInventory {
    /// `Handle.handle_id` (e.g. `b-1`).
    pub handle_id: String,
    /// Optional human-readable label set on the run call.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    /// The command, display-simplified.
    pub cmd: String,
    pub state: BashHandleState,
    /// Native pid; present while live.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    /// Process-group id; present while live.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pgid: Option<i32>,
    /// When the command was spawned. RFC3339 on the wire.
    pub started_at: DateTime<Utc>,
    /// Wall time the handle ran for; present only when terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_ms: Option<u64>,
    /// Total bytes the process has written (monotonic, partial-inclusive).
    /// Always present: defined as 0 at spawn, grows as output is produced,
    /// and persisted into the tombstone so terminal handles report it too.
    pub output_bytes: u64,
}

/// In-memory probe status of a per-scope tmux server, projected from the
/// registry's `ServerStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum TmuxServerStatus {
    /// Entry exists but no operation has probed the server yet.
    NotProbed,
    /// A probe saw the server reachable.
    Live,
    /// The entry is being torn down.
    Gone,
}

/// The tmux server entry for a scope. Presence is encoded by the parent's
/// `Option<TmuxInventory>` (`None` when there is no entry); `status`
/// distinguishes a not-yet-probed, live, or gone server.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TmuxInventory {
    pub status: TmuxServerStatus,
}

/// Two-value liveness of a browser session.
///
/// Named distinctly from the SSE `BrowserSessionState` event (which signals
/// up/down edges) — this is the inventory's point-in-time liveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum BrowserSessionLiveness {
    /// A session is present in the manager for this scope.
    Live,
    /// No session is present.
    TornDown,
}

/// The browser session projection for a scope.
///
/// `idle_ms` is computed at assembly time from the session's monotonic
/// last-activity `Instant` (`Instant::elapsed().as_millis()`); there is
/// deliberately no wall-clock timestamp on the wire, because the source is a
/// monotonic clock with no absolute value.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BrowserInventory {
    pub state: BrowserSessionLiveness,
    pub idle_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_handle_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(BashHandleState::KillPendingKernel).unwrap(),
            serde_json::Value::String("kill_pending_kernel".into())
        );
        assert_eq!(
            serde_json::to_value(BashHandleState::Running).unwrap(),
            serde_json::Value::String("running".into())
        );
        assert_eq!(
            serde_json::to_value(BashHandleState::Tombstoned).unwrap(),
            serde_json::Value::String("tombstoned".into())
        );
    }

    #[test]
    fn browser_liveness_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(BrowserSessionLiveness::TornDown).unwrap(),
            serde_json::Value::String("torn_down".into())
        );
        assert_eq!(
            serde_json::to_value(BrowserSessionLiveness::Live).unwrap(),
            serde_json::Value::String("live".into())
        );
    }

    #[test]
    fn tmux_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(TmuxServerStatus::NotProbed).unwrap(),
            serde_json::Value::String("not_probed".into())
        );
    }

    #[test]
    fn live_bash_handle_omits_terminal_only_fields() {
        let inv = BashHandleInventory {
            handle_id: "b-1".into(),
            label: Some("dev".into()),
            cmd: "npm run dev".into(),
            state: BashHandleState::Running,
            pid: Some(123),
            pgid: Some(123),
            started_at: Utc::now(),
            duration_ms: None,
            output_bytes: 4096,
        };
        let v = serde_json::to_value(&inv).unwrap();
        assert_eq!(v["state"], "running");
        assert_eq!(v["pid"], 123);
        assert_eq!(v["output_bytes"], 4096);
        assert!(v.get("duration_ms").is_none());
    }

    #[test]
    fn tombstoned_bash_handle_omits_live_only_fields() {
        let inv = BashHandleInventory {
            handle_id: "b-2".into(),
            label: None,
            cmd: "true".into(),
            state: BashHandleState::Tombstoned,
            pid: None,
            pgid: None,
            started_at: Utc::now(),
            duration_ms: Some(42),
            output_bytes: 512,
        };
        let v = serde_json::to_value(&inv).unwrap();
        assert_eq!(v["state"], "tombstoned");
        assert_eq!(v["duration_ms"], 42);
        assert!(v.get("pid").is_none());
        assert!(v.get("pgid").is_none());
        // output_bytes is always present, including on terminal handles.
        assert_eq!(v["output_bytes"], 512);
        assert!(v.get("label").is_none());
    }
}
