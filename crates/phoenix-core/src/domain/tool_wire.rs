//! Bash and tmux tool response wire types.
//!
//! These structs are the typed contract for what `BashTool` and `TmuxTool`
//! emit on the wire as `tool_result` content (`ToolOutput.output` /
//! `ToolOutput.display_data`). They are NOT directly transported as
//! `SseWireEvent` variants — bash/tmux results travel inside an enriched
//! message's `content` / `display_data` payload, which is carried as
//! `serde_json::Value` (see the "Deliberately opaque fields" note in
//! `crate::api::wire`).
//!
//! These live in the base crate so the `tools` layer depends *down* onto a
//! common vocabulary instead of onto `api`. `crate::api::wire` re-exports
//! them so the SSE boundary and ts-rs codegen surface is unchanged.
//!
//! Wire shape MUST remain byte-for-byte compatible with the JSON the
//! `BashTool` / `TmuxTool` operations produce.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Bash response shape, tagged by `status`. Encompasses every successful
/// (non-error) shape emitted by [`crate::tools::BashTool`] across spawn /
/// peek / wait / kill operations (REQ-BASH-002, REQ-BASH-003, REQ-BASH-006).
///
/// Variant correspondence:
///
/// | `status`              | When                                                              |
/// |-----------------------|-------------------------------------------------------------------|
/// | `running`             | peek on a live handle (no kill in flight)                         |
/// | `still_running`       | spawn-window elapsed; wait re-timeout                             |
/// | `kill_pending_kernel` | kill response window expired without exit                          |
/// | `tombstoned`          | peek/wait/kill served from a tombstone                            |
/// | `exited`              | spawn observed exit within `wait_seconds`                         |
/// | `killed`              | spawn observed signal-termination within `wait_seconds`           |
/// | `waiter_panicked`     | exit observer fired while state was still `Live` (waiter panic)   |
///
/// Field availability per status is intentionally non-uniform — it
/// matches what the tool actually emits (and what tests assert against).
/// `serde(tag = "status")` produces a flat object with `status` as the
/// discriminator.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum BashResponse {
    /// Live handle observed via peek (no kill in flight).
    Running(BashRunningPayload),
    /// Spawn / wait window elapsed without exit; same handle is returned.
    StillRunning(BashStillRunningPayload),
    /// Kill response timer expired; signal sent but process not yet exited.
    KillPendingKernel(BashKillPendingKernelPayload),
    /// Handle is terminal; served from tombstone (peek / wait / kill).
    Tombstoned(BashTombstonedPayload),
    /// Run-path: process exited normally inside the wait window.
    Exited(BashRunTombstonePayload),
    /// Run-path: process was signal-terminated inside the wait window.
    Killed(BashRunTombstonePayload),
    /// Waiter task panicked; surface as a structured response so callers
    /// don't hang. Only fields needed for diagnosis are emitted.
    WaiterPanicked(BashWaiterPanickedPayload),
}

/// Common ring-buffer view returned alongside any handle response
/// (REQ-BASH-004).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashRingWindow {
    pub start_offset: u64,
    pub end_offset: u64,
    pub truncated_before: bool,
    pub lines: Vec<BashRingLine>,
    /// The current trailing partial (un-newlined) line as lossy UTF-8, when
    /// the live ring holds one. Structurally distinct from `lines` (complete
    /// lines): this is the in-progress final line the process has emitted but
    /// not yet terminated with `\n`. Present only on live reads; `None` for
    /// tombstone reads (the partial was flushed to a line on EOF). The LLM
    /// `peek` may ignore it; the process inspector renders it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[ts(optional)]
    pub partial: Option<String>,
}

/// Single ring line; `bytes` is the line contents as a (lossy) UTF-8
/// string, matching what the JSON wire emits today.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashRingLine {
    pub offset: u64,
    pub bytes: String,
}

/// `running` response payload. Spec REQ-BASH-003.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashRunningPayload {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call. Echoed verbatim on every
    /// response carrying the handle so the agent can distinguish concurrent
    /// handles (REQ-BASH-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    #[serde(flatten)]
    pub window: BashRingWindow,
    /// Set when a kill has been issued and is in flight against this
    /// otherwise-still-live handle (`kill_pending_kernel` is reached only
    /// after the response window expires; until then `running` carries
    /// the in-flight kill metadata).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    /// Display label per REQ-BASH-015 (peek/wait/kill operations).
    pub display: String,
    /// Optional kill-response top-level field (kill response only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_sent: Option<String>,
}

/// `still_running` response payload. Distinguished from `running` by the
/// `waited_ms` field and the absence of a `display` label (run / wait
/// re-timeout responses don't synthesize a label — REQ-BASH-002 /
/// REQ-BASH-015).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashStillRunningPayload {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    pub waited_ms: u64,
    #[serde(flatten)]
    pub window: BashRingWindow,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
}

/// `kill_pending_kernel` response payload (REQ-BASH-003). The kill
/// response timer expired before the kernel delivered the exit; the
/// process is still alive and the handle stays subscribable.
///
/// Carries the active-kill fields (`display`, `signal_sent`) and the
/// passive-wait fields (`waited_ms`) as `Option`s with
/// `skip_serializing_if`: the same `status="kill_pending_kernel"` envelope
/// is emitted from four distinct producer sites (active kill, peek on
/// kill-in-flight, wait on kill-in-flight, run/wait observing kill-in-
/// flight) whose wire shapes differ in which of these are present. The
/// `Option` encoding makes "absent" structurally distinct from "empty
/// placeholder" so no post-serialization scrubbing is needed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashKillPendingKernelPayload {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    #[serde(flatten)]
    pub window: BashRingWindow,
    pub kill_signal_sent: String,
    pub kill_attempted_at: String,
    /// Display label (REQ-BASH-015) — present on kill / peek / wait paths
    /// that synthesise a label; absent on the run/wait passive-wait path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub display: Option<String>,
    /// Echoes the signal sent on a kill call (`TERM` / `KILL`); absent on
    /// peek/wait/passive paths which observe but did not issue the kill.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_sent: Option<String>,
    /// Wait window elapsed (run/wait passive-wait path only); absent on
    /// kill / peek paths.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub waited_ms: Option<u64>,
}

/// `tombstoned` response payload (REQ-BASH-006). Served on peek/wait/kill
/// for handles that have reached a terminal state.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashTombstonedPayload {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    pub final_cause: String,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_number: Option<i32>,
    pub duration_ms: u64,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    #[serde(flatten)]
    pub window: BashRingWindow,
    /// Display label (REQ-BASH-015) — present on the kill path; absent on
    /// peek/wait of an already-terminal handle (run-path uses the
    /// separate [`BashRunTombstonePayload`]).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub display: Option<String>,
    /// Echo of the kill signal on the `kill` operation (None on peek/wait
    /// of an already-terminal handle).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_sent: Option<String>,
}

/// Run-path tombstone response (status `exited` or `killed`). Differs
/// from [`BashTombstonedPayload`] by the absence of the synthesized
/// `display` label — run responses carry the original `cmd` instead.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashRunTombstonePayload {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    pub final_cause: String,
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub signal_number: Option<i32>,
    pub duration_ms: u64,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_signal_sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kill_attempted_at: Option<String>,
    #[serde(flatten)]
    pub window: BashRingWindow,
}

/// `waiter_panicked` response. Surface for the rare case the bash
/// waiter task panicked; carries enough info for the agent to abandon
/// the handle.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashWaiterPanickedPayload {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    pub error_message: String,
}

/// Bash error envelope (REQ-BASH-008). All tool-level failures share
/// the `error` discriminator + an `error_message`; structured fields
/// vary by error id.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "error", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum BashErrorResponse {
    HandleNotFound {
        error_message: String,
        handle_id: String,
        hint: String,
    },
    HandleCapReached {
        error_message: String,
        cap: usize,
        live_handles: Vec<BashLiveHandleSummary>,
        hint: String,
    },
    WaitSecondsOutOfRange {
        error_message: String,
        provided: i64,
        max_wait_seconds: u64,
    },
    CommandSafetyRejected {
        error_message: String,
        reason: String,
    },
    SpawnFailed {
        error_message: String,
    },
    /// Run call carried a `label` longer than `MAX_LABEL_LENGTH`
    /// (REQ-BASH-002 / REQ-BASH-010). Structured so the agent can drop
    /// or shorten the label and retry.
    LabelTooLong {
        error_message: String,
        max_label_length: usize,
    },
    /// Catch-all for input-shape failures the schema didn't reject:
    /// missing `op`, missing required peer field for the chosen op,
    /// invalid `lines` value (REQ-BASH-010). The variant name is
    /// historical — the original four-sibling shape made "mutually
    /// exclusive" the natural framing — but the producers today are
    /// generic input-shape errors. Renaming the wire string would be a
    /// breaking change with no upside.
    MutuallyExclusiveModes {
        error_message: String,
        conflicting_args: Vec<String>,
        recommended_action: String,
    },
}

/// One entry of the live-handle snapshot returned with `handle_cap_reached`
/// (REQ-BASH-005).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashLiveHandleSummary {
    pub handle: String,
    pub cmd: String,
    /// Optional handle label set on the run call (REQ-BASH-002). Echoed
    /// here so the agent has something stable to identify the handle by
    /// even when many concurrent commands share similar `cmd` prefixes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    pub age_seconds: u64,
    /// Always `"running"` today; reserved for future state-aware listings.
    pub status: String,
}

/// Tmux tool successful response (REQ-TMUX-012). The shape differs
/// deliberately from [`BashResponse`] — tmux surfaces stdout / stderr
/// separately because tmux subcommands emit structured CLI output where
/// the distinction matters (see `specs/tmux-integration/design.md`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TmuxToolResponse {
    /// `ok` (subprocess exited normally), `timed_out` (Phoenix-side
    /// `wait_seconds` expired), or `cancelled` (turn cancellation token).
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

/// Tmux tool error envelope. Stable error ids: `invalid_input`,
/// `wait_seconds_out_of_range`, `tmux_binary_unavailable`,
/// `tmux_server_unavailable`, `tmux_spawn_failed`, `tmux_wait_failed`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct TmuxErrorResponse {
    pub error: String,
    pub message: String,
}

#[cfg(test)]
mod bash_tmux_wire_tests {
    use super::*;

    #[test]
    fn bash_running_serializes_with_status_tag() {
        let resp = BashResponse::Running(BashRunningPayload {
            handle: "b-1".into(),
            cmd: "ls".into(),
            label: None,
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 1,
                truncated_before: false,
                lines: vec![BashRingLine {
                    offset: 0,
                    bytes: "hello".into(),
                }],
                partial: None,
            },
            kill_signal_sent: None,
            kill_attempted_at: None,
            display: "peek b-1".into(),
            signal_sent: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "running");
        assert_eq!(v["handle"], "b-1");
        assert_eq!(v["cmd"], "ls");
        assert_eq!(v["display"], "peek b-1");
        assert_eq!(v["start_offset"], 0);
        assert_eq!(v["end_offset"], 1);
        assert_eq!(v["truncated_before"], false);
        assert_eq!(v["lines"][0]["offset"], 0);
        assert_eq!(v["lines"][0]["bytes"], "hello");
        // label omitted on serialize when None.
        assert!(v.get("label").is_none());
    }

    #[test]
    fn bash_running_with_label_round_trips() {
        let resp = BashResponse::Running(BashRunningPayload {
            handle: "b-1".into(),
            cmd: "npm run dev".into(),
            label: Some("dev-server".into()),
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 0,
                truncated_before: false,
                lines: vec![],
                partial: None,
            },
            kill_signal_sent: None,
            kill_attempted_at: None,
            display: "peek b-1".into(),
            signal_sent: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["label"], "dev-server");
    }

    #[test]
    fn bash_tombstoned_carries_final_cause_and_signal_number() {
        let resp = BashResponse::Tombstoned(BashTombstonedPayload {
            handle: "b-2".into(),
            cmd: "sleep 1".into(),
            label: None,
            final_cause: "killed".into(),
            exit_code: None,
            signal_number: Some(15),
            duration_ms: 1000,
            finished_at: "1700000000".into(),
            kill_signal_sent: Some("TERM".into()),
            kill_attempted_at: Some("1700000000".into()),
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 0,
                truncated_before: false,
                lines: vec![],
                partial: None,
            },
            display: Some("kill b-2 (TERM)".into()),
            signal_sent: Some("TERM".into()),
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "tombstoned");
        assert_eq!(v["final_cause"], "killed");
        assert_eq!(v["signal_number"], 15);
        assert_eq!(v["signal_sent"], "TERM");
        assert_eq!(v["display"], "kill b-2 (TERM)");
    }

    #[test]
    fn bash_run_exited_status_is_exited_not_tombstoned() {
        // REQ-BASH-002: run responses use `exited` / `killed` directly.
        let resp = BashResponse::Exited(BashRunTombstonePayload {
            handle: "b-3".into(),
            cmd: "echo hi".into(),
            label: None,
            final_cause: "exited".into(),
            exit_code: Some(0),
            signal_number: None,
            duration_ms: 5,
            finished_at: "1700000000".into(),
            kill_signal_sent: None,
            kill_attempted_at: None,
            window: BashRingWindow {
                start_offset: 0,
                end_offset: 1,
                truncated_before: false,
                lines: vec![BashRingLine {
                    offset: 0,
                    bytes: "hi".into(),
                }],
                partial: None,
            },
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "exited");
        // No display field on the run-tombstone shape.
        assert!(v.get("display").is_none());
    }

    /// Helper: produce a typical [`BashRingWindow`] for the
    /// kill-pending-kernel shape tests below.
    fn kpk_test_window() -> BashRingWindow {
        BashRingWindow {
            start_offset: 0,
            end_offset: 0,
            truncated_before: false,
            lines: vec![],
            partial: None,
        }
    }

    /// `kill` operation that timed out before the kernel delivered the exit:
    /// `display` is the synthesised label, `signal_sent` echoes the issued
    /// signal, `waited_ms` is absent (this is not a passive wait).
    #[test]
    fn bash_kill_pending_active_kill_shape() {
        let resp = BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
            handle: "b-1".into(),
            cmd: "trap '' TERM; sleep 9".into(),
            label: None,
            window: kpk_test_window(),
            kill_signal_sent: "TERM".into(),
            kill_attempted_at: "1700000000".into(),
            display: Some("kill b-1 (TERM, pending)".into()),
            signal_sent: Some("TERM".into()),
            waited_ms: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "kill_pending_kernel");
        assert_eq!(v["display"], "kill b-1 (TERM, pending)");
        assert_eq!(v["signal_sent"], "TERM");
        assert!(
            v.get("waited_ms").is_none(),
            "active-kill path must not emit waited_ms"
        );
    }

    /// `peek` / `wait` on a handle already in kill_pending_kernel: `display`
    /// carries the peek/wait label, but `signal_sent` is absent (this caller
    /// did not issue the kill) and `waited_ms` is absent (not a passive run).
    #[test]
    fn bash_kill_pending_peek_or_wait_shape() {
        let resp = BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
            handle: "b-2".into(),
            cmd: "trap '' TERM; sleep 9".into(),
            label: None,
            window: kpk_test_window(),
            kill_signal_sent: "TERM".into(),
            kill_attempted_at: "1700000000".into(),
            display: Some("peek b-2".into()),
            signal_sent: None,
            waited_ms: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "kill_pending_kernel");
        assert_eq!(v["display"], "peek b-2");
        assert!(
            v.get("signal_sent").is_none(),
            "peek/wait must not echo signal_sent"
        );
        assert!(v.get("waited_ms").is_none());
    }

    /// Passive `run` / `wait` observing an in-flight kill: no `display`
    /// label (only the kill / peek / wait paths synthesise one) and no
    /// `signal_sent` echo, but `waited_ms` carries the elapsed wait.
    /// This is the path the previous code reached by emitting
    /// `display: ""` + `signal_sent: ""` placeholders and scrubbing them
    /// after the fact.
    #[test]
    fn bash_kill_pending_passive_wait_shape() {
        let resp = BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
            handle: "b-3".into(),
            cmd: "trap '' TERM; sleep 9".into(),
            label: None,
            window: kpk_test_window(),
            kill_signal_sent: "TERM".into(),
            kill_attempted_at: "1700000000".into(),
            display: None,
            signal_sent: None,
            waited_ms: Some(30_000),
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "kill_pending_kernel");
        assert!(
            v.get("display").is_none(),
            "passive-wait must not emit a display label"
        );
        assert!(
            v.get("signal_sent").is_none(),
            "passive-wait must not echo signal_sent"
        );
        assert_eq!(v["waited_ms"], 30_000);
    }

    /// `peek` / `wait` of an already-terminal handle: tombstoned with no
    /// synthesised `display` label and no `signal_sent` echo. Previously
    /// emitted `display: ""` and scrubbed it.
    #[test]
    fn bash_tombstoned_passive_observe_shape() {
        let resp = BashResponse::Tombstoned(BashTombstonedPayload {
            handle: "b-4".into(),
            cmd: "true".into(),
            label: None,
            final_cause: "exited".into(),
            exit_code: Some(0),
            signal_number: None,
            duration_ms: 1,
            finished_at: "1700000000".into(),
            kill_signal_sent: None,
            kill_attempted_at: None,
            window: kpk_test_window(),
            display: None,
            signal_sent: None,
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "tombstoned");
        assert!(
            v.get("display").is_none(),
            "passive-observe of tombstoned must not emit a display label"
        );
        assert!(v.get("signal_sent").is_none());
    }

    #[test]
    fn bash_error_handle_cap_reached_includes_live_handles() {
        let resp = BashErrorResponse::HandleCapReached {
            error_message: "this work scope has reached the cap of 8 live bash handles".into(),
            cap: 8,
            live_handles: vec![BashLiveHandleSummary {
                handle: "b-1".into(),
                cmd: "cargo test".into(),
                label: Some("tests".into()),
                age_seconds: 1820,
                status: "running".into(),
            }],
            hint: "kill or wait".into(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"], "handle_cap_reached");
        assert_eq!(v["cap"], 8);
        assert_eq!(v["live_handles"][0]["handle"], "b-1");
        assert_eq!(v["live_handles"][0]["label"], "tests");
        assert_eq!(v["live_handles"][0]["status"], "running");
    }

    #[test]
    fn bash_error_mutually_exclusive_modes_serializes_with_error_tag() {
        let resp = BashErrorResponse::MutuallyExclusiveModes {
            error_message: "op is required and must be one of: run, peek, wait, kill".into(),
            conflicting_args: vec![],
            recommended_action: "set op to one of: run, peek, wait, kill".into(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"], "mutually_exclusive_modes");
        assert!(v["conflicting_args"].is_array());
    }

    #[test]
    fn bash_error_label_too_long_serializes_with_error_tag() {
        let resp = BashErrorResponse::LabelTooLong {
            error_message: "label exceeds the 64-character cap".into(),
            max_label_length: 64,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["error"], "label_too_long");
        assert_eq!(v["max_label_length"], 64);
    }

    #[test]
    fn bash_waiter_panicked_with_label_round_trips() {
        // Regression for the codex review on PR #42: BashWaiterPanickedPayload
        // also carries `handle`, so it must echo `label` for consistency with
        // the rest of REQ-BASH-002's "every response carrying the handle"
        // contract.
        let resp = BashResponse::WaiterPanicked(BashWaiterPanickedPayload {
            handle: "b-9".into(),
            cmd: "npm run dev".into(),
            label: Some("dev-server".into()),
            error_message: "the waiter task for this handle panicked; the process state is unknown"
                .into(),
        });
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "waiter_panicked");
        assert_eq!(v["handle"], "b-9");
        assert_eq!(v["label"], "dev-server");
    }

    #[test]
    fn tmux_response_carries_separate_stdout_and_stderr() {
        let resp = TmuxToolResponse {
            status: "ok".into(),
            exit_code: Some(0),
            duration_ms: 12,
            stdout: "main: 1 windows".into(),
            stderr: String::new(),
            truncated: false,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["stdout"], "main: 1 windows");
        assert_eq!(v["stderr"], "");
        assert_eq!(v["truncated"], false);
        assert_eq!(v["exit_code"], 0);
    }
}
