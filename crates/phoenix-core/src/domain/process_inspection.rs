//! Process Inspector wire types (`specs/process-inspector/`).
//!
//! [`BashHandleInspection`] is the per-handle drill-down read-projection
//! served over `GET /api/work-scope/:scope_key/bash/:handle_id/inspect`. It
//! complements the work-scope roll-up ([`crate::domain::work_scope_inventory`]):
//! the inventory answers "what is running?"; the inspection answers "what is
//! *this one* doing, and is it healthy?".
//!
//! Like the inventory, it is assembled on demand from the in-memory bash
//! handle registry plus a request-time process-group sample, and is never
//! stored. It reuses the inventory's [`BashHandleState`] vocabulary and the
//! bash tool's [`BashRingWindow`] output shape verbatim — there is no second
//! representation of handle state or output to diverge from.

use chrono::{DateTime, Utc};
use serde::Serialize;
use ts_rs::TS;

use super::tool_wire::BashRingWindow;
use super::work_scope_inventory::BashHandleState;

/// One bash handle's full inspection snapshot: identity + state, an output
/// delta (the ring read), and a live resource sample.
///
/// `pid`/`pgid` and `resources` are present only while the handle is live;
/// `exit_code`/`signal_number`/`duration_ms` only once terminal. Each is
/// skipped on the wire when absent so the TS side sees an optional field —
/// the same optional-while-live pattern [`crate::domain::work_scope_inventory::BashHandleInventory`]
/// uses.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashHandleInspection {
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
    /// Kernel-supplied exit code; present when terminal and the kernel
    /// returned a code.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit_code: Option<i32>,
    /// Terminating signal number; present when terminal and a signal
    /// terminated the process.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signal_number: Option<i32>,
    /// Wall time the handle ran for; present only when terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_ms: Option<u64>,
    /// Output delta — the existing ring-read window shape (REQ-BASH-004),
    /// reused verbatim from the bash tool's response.
    pub output: BashRingWindow,
    /// Resource sample over the handle's process group. `None` when the
    /// handle is terminal (there is no process group to sample). Skipped on
    /// the wire when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resources: Option<ResourceSample>,
}

/// The core resource trio over a bash handle's process group, sampled at
/// request time.
///
/// Each field is independently `Option`: a `null` is a real capability gap
/// (the metric could not be read on this platform/kernel, logged at `debug`),
/// distinct from a `0` sample. This is distinct from the parent
/// [`BashHandleInspection::resources`] being `None`, which means "no process
/// group to sample" (terminal handle).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct ResourceSample {
    /// Summed CPU percentage over the process group; null if unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cpu_pct: Option<f32>,
    /// Proportional, shared-aware memory of the group in bytes — PSS on
    /// Linux, `phys_footprint` on macOS, NOT RSS. Null if unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub memory_bytes: Option<u64>,
    /// Count of live processes in the group; null if unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub process_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tool_wire::BashRingWindow;

    fn empty_window() -> BashRingWindow {
        BashRingWindow {
            start_offset: 0,
            end_offset: 0,
            truncated_before: false,
            lines: vec![],
        }
    }

    #[test]
    fn live_inspection_carries_resources_and_omits_terminal_fields() {
        let inspection = BashHandleInspection {
            handle_id: "b-1".into(),
            label: Some("dev".into()),
            cmd: "npm run dev".into(),
            state: BashHandleState::Running,
            pid: Some(1234),
            pgid: Some(1234),
            started_at: Utc::now(),
            exit_code: None,
            signal_number: None,
            duration_ms: None,
            output: empty_window(),
            resources: Some(ResourceSample {
                cpu_pct: Some(12.5),
                memory_bytes: Some(4096),
                process_count: Some(2),
            }),
        };
        let v = serde_json::to_value(&inspection).unwrap();
        assert_eq!(v["state"], "running");
        assert_eq!(v["pid"], 1234);
        assert_eq!(v["resources"]["cpu_pct"], 12.5);
        assert_eq!(v["resources"]["process_count"], 2);
        assert!(v.get("exit_code").is_none());
        assert!(v.get("duration_ms").is_none());
        assert!(v["output"]["lines"].is_array());
    }

    #[test]
    fn terminal_inspection_omits_resources_and_live_fields() {
        let inspection = BashHandleInspection {
            handle_id: "b-2".into(),
            label: None,
            cmd: "true".into(),
            state: BashHandleState::Tombstoned,
            pid: None,
            pgid: None,
            started_at: Utc::now(),
            exit_code: Some(0),
            signal_number: None,
            duration_ms: Some(42),
            output: empty_window(),
            resources: None,
        };
        let v = serde_json::to_value(&inspection).unwrap();
        assert_eq!(v["state"], "tombstoned");
        assert_eq!(v["exit_code"], 0);
        assert_eq!(v["duration_ms"], 42);
        assert!(v.get("pid").is_none());
        assert!(v.get("pgid").is_none());
        assert!(v.get("resources").is_none());
        assert!(v.get("label").is_none());
    }

    #[test]
    fn resource_sample_nulls_skip_on_the_wire() {
        let sample = ResourceSample {
            cpu_pct: None,
            memory_bytes: Some(2048),
            process_count: None,
        };
        let v = serde_json::to_value(&sample).unwrap();
        assert_eq!(v["memory_bytes"], 2048);
        assert!(v.get("cpu_pct").is_none());
        assert!(v.get("process_count").is_none());
    }
}
