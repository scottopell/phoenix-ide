//! Bash tool agent-facing operations: spawn, peek, wait, kill.
//!
//! REQ-BASH-001/002/003/006/008/010/011: dispatch + response shaping for the
//! four operation kinds. Each operation produces a structured JSON envelope
//! delivered through [`ToolOutput`]. Errors use stable string identifiers
//! (REQ-BASH-008); successful operations carry `status` + handle metadata
//! per the design.md "Output Capture and Display" and bash.allium response
//! rules.
//!
//! The request is a tagged enum (`BashRequest`) where each variant
//! corresponds to exactly one of `cmd / peek / wait / kill` — making the
//! "exactly one operation per call" mutual exclusion structurally
//! representable rather than runtime-checked.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::RwLock;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use super::handle::{
    ExitState, ExitWatchPanicGuard, FinalCause, Handle, HandleId, HandleState, KillSignal,
    TOMBSTONE_TAIL_LINES,
};
use super::registry::{BashHandleError, ConversationHandles, LiveHandleSummary};
use super::ring::{RingLine, WindowView};
use super::types::{BashOp, BashToolInput};
use crate::api::wire::{
    BashErrorResponse, BashKillPendingKernelPayload, BashLiveHandleSummary, BashResponse,
    BashRingLine, BashRingWindow, BashRunTombstonePayload, BashRunningPayload,
    BashStillRunningPayload, BashTombstonedPayload, BashWaiterPanickedPayload,
};
use crate::tools::{ToolContext, ToolOutput};

// ---------------------------------------------------------------------------
// Configuration constants (REQ-BASH config)
// ---------------------------------------------------------------------------

/// REQ-BASH-002: default `wait_seconds` when omitted.
pub const DEFAULT_WAIT_SECONDS: u64 = 30;

/// REQ-BASH-002: upper bound on `wait_seconds`.
pub const MAX_WAIT_SECONDS: u64 = 900;

/// REQ-BASH-003: bound on how long a kill call blocks for the process to
/// exit before returning `kill_pending_kernel`.
pub const KILL_RESPONSE_TIMEOUT_SECONDS: u64 = 30;

/// REQ-BASH-004: default lines returned by peek when no read modifier
/// is supplied.
pub const DEFAULT_PEEK_LINES: usize = 200;

/// REQ-BASH-002 / REQ-BASH-010: soft cap on the optional run-call
/// `label` length. Over-cap labels surface as `error: "label_too_long"`.
pub const MAX_LABEL_LENGTH: usize = 64;

// Hint text for the cap-rejection envelope (REQ-BASH-005).
const CAP_HINT: &str =
    "kill or wait on a handle before spawning more, or use the tmux tool for long-runners";

// Hint text for handle_not_found responses (REQ-BASH-008/010): tmux pointer
// for handles that may predate this Phoenix process.
const HANDLE_NOT_FOUND_HINT: &str =
    "if Phoenix restarted since this handle was created, the handle was lost — bash handles do \
     not survive Phoenix restart. For processes that should survive Phoenix restart, use the \
     tmux tool.";

// ---------------------------------------------------------------------------
// Request shape
// ---------------------------------------------------------------------------

/// Parsed read-window arguments (REQ-BASH-004). `lines` xor `since`.
#[derive(Debug, Default, Clone)]
pub struct ReadArgs {
    pub lines: Option<usize>,
    pub since: Option<u64>,
}

/// Parsed, validated request. The variants make "exactly one operation per
/// call" structural — there is no shape representable that has both `cmd`
/// and `peek`.
#[derive(Debug)]
pub enum BashRequest {
    Run {
        cmd: String,
        label: Option<String>,
        wait_seconds: u64,
        read_args: ReadArgs,
    },
    Peek {
        handle_id: String,
        read_args: ReadArgs,
    },
    Wait {
        handle_id: String,
        wait_seconds: u64,
        read_args: ReadArgs,
    },
    Kill {
        handle_id: String,
        signal: KillSignal,
    },
}

/// Stable error identifiers (REQ-BASH-008).
#[derive(Debug)]
pub enum BashError {
    HandleNotFound {
        handle_id: String,
    },
    HandleCapReached(BashHandleError),
    WaitSecondsOutOfRange {
        provided: i64,
        max: u64,
    },
    CommandSafetyRejected {
        reason: String,
    },
    SpawnFailed {
        error_message: String,
    },
    /// Run call carried a label longer than `MAX_LABEL_LENGTH`
    /// (REQ-BASH-002 / REQ-BASH-010).
    LabelTooLong {
        max: usize,
    },
    /// Catch-all for input-shape failures the schema didn't catch:
    /// missing/invalid `op`, missing required peer field for the chosen
    /// op, invalid `lines` value, etc. (REQ-BASH-010). The variant name
    /// is historical — the producer set has shrunk substantially since
    /// the four-sibling shape was retired.
    MutuallyExclusiveModes {
        message: String,
        conflicting_args: Vec<&'static str>,
        recommended_action: String,
        extra: Option<Value>,
    },
}

impl BashError {
    fn into_tool_output(self) -> ToolOutput {
        let typed: BashErrorResponse = match self {
            BashError::HandleNotFound { handle_id } => BashErrorResponse::HandleNotFound {
                error_message: format!("handle {handle_id} not found in this conversation"),
                handle_id,
                hint: HANDLE_NOT_FOUND_HINT.to_string(),
            },
            BashError::HandleCapReached(BashHandleError::HandleCapReached {
                cap,
                live_handles,
            }) => {
                let live: Vec<BashLiveHandleSummary> = live_handles
                    .iter()
                    .map(|s: &LiveHandleSummary| BashLiveHandleSummary {
                        handle: s.handle.as_str().to_string(),
                        cmd: s.cmd.clone(),
                        label: s.label.clone(),
                        age_seconds: s.age_seconds,
                        status: "running".to_string(),
                    })
                    .collect();
                BashErrorResponse::HandleCapReached {
                    error_message: format!(
                        "this conversation has reached the cap of {cap} live bash handles"
                    ),
                    cap,
                    live_handles: live,
                    hint: CAP_HINT.to_string(),
                }
            }
            BashError::WaitSecondsOutOfRange { provided, max } => {
                BashErrorResponse::WaitSecondsOutOfRange {
                    error_message: format!(
                        "wait_seconds={provided} is out of range [0, {max}]; long-running \
                         operations should yield a handle and resume via wait calls"
                    ),
                    provided,
                    max_wait_seconds: max,
                }
            }
            BashError::CommandSafetyRejected { reason } => {
                BashErrorResponse::CommandSafetyRejected {
                    error_message: reason.clone(),
                    reason,
                }
            }
            BashError::SpawnFailed { error_message } => {
                BashErrorResponse::SpawnFailed { error_message }
            }
            BashError::LabelTooLong { max } => BashErrorResponse::LabelTooLong {
                error_message: format!("label exceeds the {max}-character cap"),
                max_label_length: max,
            },
            BashError::MutuallyExclusiveModes {
                message,
                conflicting_args,
                recommended_action,
                extra,
            } => {
                let typed_err = BashErrorResponse::MutuallyExclusiveModes {
                    error_message: message,
                    conflicting_args: conflicting_args.into_iter().map(String::from).collect(),
                    recommended_action,
                };
                // Merge the dual-pass `extra` onto the typed envelope at
                // the JSON layer (the typed struct intentionally doesn't
                // model these — see the `MutuallyExclusiveModes` doc on
                // `BashErrorResponse`).
                let mut value = serde_json::to_value(&typed_err).unwrap_or(Value::Null);
                if let (Value::Object(obj), Some(Value::Object(extras))) = (&mut value, extra) {
                    for (k, v) in extras {
                        obj.insert(k, v);
                    }
                }
                let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
                return ToolOutput::error(serialized).with_display(value);
            }
        };

        let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
        let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
        ToolOutput::error(serialized).with_display(value)
    }
}

// ---------------------------------------------------------------------------
// Parsing & validation
// ---------------------------------------------------------------------------

/// Parse + validate the agent's request. Returns the typed `BashRequest`
/// or a `BashError` ready to be returned as the tool output.
///
/// Tolerance rules (REQ-BASH-010, revision 3):
///
/// - `op` is required. The retired affordances (legacy four-sibling
///   `peek`/`wait`/`kill` keys, `mode` shim, `command` alias for `cmd`,
///   bare `cmd` with no `op`) are no longer accepted; `deny_unknown_fields`
///   on `BashToolInput` turns them into structured parse errors.
/// - `since=0` is treated as absent (default-fill from models that emit
///   the schema minimum despite `minimum: 1`). A `tracing::debug!` line
///   names the dropped value.
/// - When both `lines` and `since` are supplied, `lines` wins and `since`
///   is silently dropped (default-fill collision on optional integers).
///   A `tracing::debug!` line records the drop.
pub fn parse_request(input: Value) -> Result<BashRequest, BashError> {
    let raw: BashToolInput = serde_json::from_value(input).map_err(|e| {
        // Schema-level rejection — the agent passed a malformed shape, or
        // an unknown field (notably any of the retired affordances).
        BashError::MutuallyExclusiveModes {
            message: format!("invalid bash input: {e}"),
            conflicting_args: vec![],
            recommended_action:
                "send `op` set to run|peek|wait|kill, with the documented field types".into(),
            extra: None,
        }
    })?;

    let read_args = parse_read_args(raw.lines, raw.since)?;

    match raw.op {
        BashOp::Run => {
            let cmd = raw.cmd.filter(|s| !s.is_empty()).ok_or_else(|| {
                BashError::MutuallyExclusiveModes {
                    message: "op=run requires a non-empty 'cmd'".into(),
                    conflicting_args: vec!["cmd"],
                    recommended_action: "supply cmd=<shell command>".into(),
                    extra: None,
                }
            })?;
            let label = parse_label(raw.label)?;
            let wait_seconds = resolve_wait_seconds(raw.wait_seconds)?;
            Ok(BashRequest::Run {
                cmd,
                label,
                wait_seconds,
                read_args,
            })
        }
        BashOp::Peek => {
            let handle_id = resolve_handle(&raw, BashOp::Peek)?;
            Ok(BashRequest::Peek {
                handle_id,
                read_args,
            })
        }
        BashOp::Wait => {
            let handle_id = resolve_handle(&raw, BashOp::Wait)?;
            let wait_seconds = resolve_wait_seconds(raw.wait_seconds)?;
            Ok(BashRequest::Wait {
                handle_id,
                wait_seconds,
                read_args,
            })
        }
        BashOp::Kill => {
            let handle_id = resolve_handle(&raw, BashOp::Kill)?;
            let signal = raw.signal.unwrap_or(KillSignal::Term);
            Ok(BashRequest::Kill { handle_id, signal })
        }
    }
}

/// Resolve the handle id for peek/wait/kill from the canonical `handle`
/// field. The legacy operation-key fallback (`raw.peek` / `raw.wait` /
/// `raw.kill`) was retired alongside the four-sibling shape (REQ-BASH-010,
/// revision 3).
fn resolve_handle(raw: &BashToolInput, op: BashOp) -> Result<String, BashError> {
    raw.handle
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| BashError::MutuallyExclusiveModes {
            message: format!("op={} requires a non-empty 'handle'", op.as_field_name()),
            conflicting_args: vec!["handle"],
            recommended_action: "supply handle=<id>".into(),
            extra: None,
        })
}

/// Validate and normalize the optional `label` for `op=run`. Empty
/// strings (a frequent default-fill emission) are treated as absent;
/// over-cap labels are rejected with `label_too_long` per REQ-BASH-002.
fn parse_label(raw: Option<String>) -> Result<Option<String>, BashError> {
    let Some(label) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_LABEL_LENGTH {
        return Err(BashError::LabelTooLong {
            max: MAX_LABEL_LENGTH,
        });
    }
    Ok(Some(label))
}

fn parse_read_args(lines: Option<i64>, since: Option<i64>) -> Result<ReadArgs, BashError> {
    // since=0 is below the schema's advertised `minimum: 1`, but current
    // OpenAI Responses-API models still emit it as a default-fill on
    // optional integers. Treating it as absent routes through the default
    // `lines` window. REQ-BASH-010 (revision 3) requires this be an
    // auditable tolerance, not a silent one.
    let since = match since {
        Some(0) => {
            tracing::debug!(
                "bash read window: dropping since=0 (likely default-fill below schema \
                 minimum: 1); routing through default `lines` window"
            );
            None
        }
        Some(n) if n > 0 => Some(n),
        Some(_) | None => None,
    };

    // When both `lines` and `since` are provided, prefer `lines` and drop
    // `since`. Models on structured-output APIs default-fill optional
    // integers with their schema minimums (`lines=1`, `since=1`); for
    // a short command output, `since=1` returns nothing while the request
    // default `lines=200` returns the actual tail. Real incremental-peek
    // users (subsequent peeks with a stored end_offset) typically omit
    // `lines` entirely.
    let since = if lines.is_some() && since.is_some() {
        tracing::debug!(
            dropped_since = ?since,
            "bash read window: both `lines` and `since` provided — preferring `lines` (likely \
             default-fill); dropping `since`"
        );
        None
    } else {
        since
    };

    let lines = match lines {
        None => None,
        Some(n) if n > 0 => Some(usize::try_from(n).unwrap_or(usize::MAX)),
        Some(_) => {
            return Err(BashError::MutuallyExclusiveModes {
                message: "lines must be a positive integer".into(),
                conflicting_args: vec!["lines"],
                recommended_action: "pass lines as a positive integer (e.g., lines=200)".into(),
                extra: None,
            });
        }
    };
    let since = since.map(|n| u64::try_from(n).unwrap_or(0));
    Ok(ReadArgs { lines, since })
}

fn resolve_wait_seconds(raw: Option<i64>) -> Result<u64, BashError> {
    let value: i64 = raw.unwrap_or_else(|| i64::try_from(DEFAULT_WAIT_SECONDS).unwrap_or(30));
    let max_signed: i64 = i64::try_from(MAX_WAIT_SECONDS).unwrap_or(i64::MAX);
    if !(0..=max_signed).contains(&value) {
        return Err(BashError::WaitSecondsOutOfRange {
            provided: value,
            max: MAX_WAIT_SECONDS,
        });
    }
    Ok(u64::try_from(value).unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Top-level dispatch
// ---------------------------------------------------------------------------

/// Run a bash request end-to-end and produce the `ToolOutput`.
pub async fn dispatch(input: Value, ctx: ToolContext) -> ToolOutput {
    let request = match parse_request(input) {
        Ok(r) => r,
        Err(e) => return e.into_tool_output(),
    };

    match request {
        BashRequest::Run {
            cmd,
            label,
            wait_seconds,
            read_args,
        } => run_run(&cmd, label, wait_seconds, read_args, &ctx).await,
        BashRequest::Peek {
            handle_id,
            read_args,
        } => run_peek(&handle_id, read_args, &ctx).await,
        BashRequest::Wait {
            handle_id,
            wait_seconds,
            read_args,
        } => run_wait(&handle_id, wait_seconds, read_args, &ctx).await,
        BashRequest::Kill { handle_id, signal } => run_kill(&handle_id, signal, &ctx).await,
    }
}

// ---------------------------------------------------------------------------
// Run (formerly "spawn") — the agent's run-and-optionally-yield-a-handle
// operation. The OS-level fork/exec is still called "spawn" within this
// module where the distinction matters.
// ---------------------------------------------------------------------------

async fn run_run(
    cmd: &str,
    label: Option<String>,
    wait_seconds: u64,
    read_args: ReadArgs,
    ctx: &ToolContext,
) -> ToolOutput {
    // REQ-BASH-011: safety check before reserving any resources.
    if let Err(e) = crate::tools::bash_check::check(cmd) {
        return BashError::CommandSafetyRejected { reason: e.message }.into_tool_output();
    }

    let registry = ctx.bash_handle_registry().clone();
    let handles_arc = match ctx.bash_handles().await {
        Ok(h) => h,
        Err(e) => {
            return BashError::SpawnFailed {
                error_message: format!("could not access bash handle registry: {e}"),
            }
            .into_tool_output();
        }
    };

    // REQ-BASH-005: cap check + handle id allocation under the same write
    // lock so two concurrent runs can't both race past the cap.
    let cap = registry.live_handle_cap();
    let ring_bytes_cap = registry.ring_bytes_cap();
    let handle_id;
    {
        let mut handles = handles_arc.write().await;
        if let Err(e) = handles.check_cap(cap).await {
            return BashError::HandleCapReached(e).into_tool_output();
        }
        handle_id = handles.allocate_handle_id();
        // We deliberately do not insert the Handle yet — we need the pgid
        // from the spawned child. We hold the write lock across the spawn
        // so no other run can race the cap check, then insert below.
        // Spawn is fast (a fork+exec) so this lock-hold is bounded.
        match spawn_child(cmd, label, ctx, handle_id.clone(), ring_bytes_cap) {
            Ok((handle, child)) => {
                let inserted = handles.insert(handle.clone());
                drop(handles);
                start_io_tasks(&inserted, child);
                race_run_response(inserted, cmd, wait_seconds, read_args, ctx).await
            }
            Err(e) => {
                // Drop the allocated handle id by NOT inserting — next
                // allocation will simply skip past it; that's harmless
                // (handle ids are sequential and don't need to be dense).
                drop(handles);
                BashError::SpawnFailed { error_message: e }.into_tool_output()
            }
        }
    }
}

// `pgid` and `pid` mirror the Allium-spec field names; renaming for the
// similar-names lint would diverge from the spec.
#[allow(clippy::similar_names)]
fn spawn_child(
    cmd: &str,
    label: Option<String>,
    ctx: &ToolContext,
    handle_id: HandleId,
    ring_bytes_cap: usize,
) -> Result<(Arc<Handle>, tokio::process::Child), String> {
    // Per bash.allium @guidance on HandleSpawned:
    //   "Spawn child via Command::new(\"bash\").args([\"-c\", cmd]) with
    //    pre_exec(setpgid(0,0))"
    // The schema description and design.md text mention an `exec <cmd>`
    // wrapping for the "user command replaces the bash shell" benefit.
    // That wrapping fundamentally breaks compound commands like
    // `cd /foo && cargo test` (`exec cd ...` errors because cd is a
    // builtin; even when the first word IS a program, the `&& <rest>`
    // tail is unreachable because exec replaces the shell — we tested
    // this against the actual bash). REQ-BASH-006 explicitly accommodates
    // the alternative path via the 128+signum convention recovery in the
    // waiter task. For external SIGKILL targeted at the bash pid (the
    // case the design rationale was written for), `ExitStatus::signal()`
    // still returns Some(9) under plain `bash -c <cmd>` because bash
    // itself is what gets signaled — same outcome as exec'd bash. The
    // load-bearing piece is the process-group leader bit (setpgid below)
    // so that `kill(-pgid, sig)` reaches the user's processes.
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(&ctx.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            // Become a process group leader so the kill path can signal
            // the entire group via kill(-pgid, sig).
            // SAFETY: setpgid in pre_exec is a documented pattern; no
            // memory or fd implications.
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command
        .spawn()
        .map_err(|e| format!("failed to spawn bash child: {e}"))?;

    let pid = child
        .id()
        .ok_or_else(|| "spawned child has no pid".to_string())?;
    // pgid == pid because we made the child a process group leader.
    let pgid = i32::try_from(pid).unwrap_or(0);

    let handle = Handle::new_live(
        ctx.conversation_id.clone(),
        handle_id,
        cmd.to_string(),
        label,
        pgid,
        pid,
        ring_bytes_cap,
    );
    Ok((handle, child))
}

fn start_io_tasks(handle: &Arc<Handle>, mut child: tokio::process::Child) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Spawn the readers and capture their JoinHandles so the waiter
    // can join them between `child.wait()` and `transition_to_terminal`.
    // Without this synchronisation, the waiter races readers: the child
    // exits, the waiter resolves wait() and tombstones the handle, and
    // the readers' next read of the (already-buffered) final chunk hits
    // a non-Live state and silently drops those bytes.
    let stdout_join = stdout.map(|s| {
        let h = handle.clone();
        tokio::spawn(async move {
            read_pipe_to_ring(s, h, "stdout").await;
        })
    });
    let stderr_join = stderr.map(|s| {
        let h = handle.clone();
        tokio::spawn(async move {
            read_pipe_to_ring(s, h, "stderr").await;
        })
    });

    // Waiter task: call wait(), drain readers, then demote the handle.
    let h = handle.clone();
    tokio::spawn(async move {
        run_waiter(h, child, stdout_join, stderr_join).await;
    });
}

async fn read_pipe_to_ring<R>(mut pipe: R, handle: Arc<Handle>, _which: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 4096];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                // Append to ring under the live mutex. If the handle has
                // already transitioned to tombstone (waiter beat us to
                // EOF), the live ring is gone and we silently drop the
                // bytes.
                let state = handle.state().await;
                if let HandleState::Live(live) = state.as_ref() {
                    let mut ring = live.ring.lock().await;
                    ring.append(&buf[..n]);
                }
            }
            Err(e) => {
                tracing::debug!(?e, %handle.handle_id, "pipe read error");
                break;
            }
        }
    }
    // Flush trailing partial line on EOF (only meaningful if still live).
    let state = handle.state().await;
    if let HandleState::Live(live) = state.as_ref() {
        let mut ring = live.ring.lock().await;
        ring.flush_partial();
    }
}

/// Bound on how long the waiter waits for the stdout/stderr reader
/// tasks to drain after `child.wait()` resolves. Pipes EOF promptly
/// when the child exits, but a grandchild that inherited the stdout fd
/// and still holds it open would block the reader indefinitely; this
/// timeout protects the waiter from that pathological case at the cost
/// of dropping a few bytes in extreme scenarios.
const READER_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);

async fn run_waiter(
    handle: Arc<Handle>,
    mut child: tokio::process::Child,
    stdout_join: Option<tokio::task::JoinHandle<()>>,
    stderr_join: Option<tokio::task::JoinHandle<()>>,
) {
    let panic_guard = ExitWatchPanicGuard::new(handle.clone());
    let started_at = Instant::now();
    let exit_status = child.wait().await;

    let cause = match exit_status {
        Ok(status) => exit_status_to_cause(status),
        Err(e) => {
            tracing::warn!(?e, %handle.handle_id, "child wait() failed; recording as exited(None)");
            FinalCause::Exited { exit_code: None }
        }
    };

    // Drain the reader tasks BEFORE transitioning to terminal so the
    // ring captures any final chunk the kernel buffered between the
    // child's last write and its exit. Without this, the tombstone
    // tail can be missing trailing output from fast-exiting commands
    // or commands whose final line was unterminated.
    let drain = async {
        if let Some(h) = stdout_join {
            let _ = h.await;
        }
        if let Some(h) = stderr_join {
            let _ = h.await;
        }
    };
    let _ = tokio::time::timeout(READER_DRAIN_TIMEOUT, drain).await;

    let elapsed = started_at.elapsed();
    if handle
        .transition_to_terminal(cause, elapsed, TOMBSTONE_TAIL_LINES)
        .await
    {
        handle.publish_exit(ExitState::Exited);
    }
    panic_guard.disarm();
}

#[cfg(unix)]
fn exit_status_to_cause(status: std::process::ExitStatus) -> FinalCause {
    // Two paths surface a signaled termination under plain `bash -c <cmd>`:
    //   1. WIFSIGNALED on bash itself — the wrapper was directly signaled.
    //      `ExitStatus::signal()` returns the signal number.
    //   2. 128+signum exit code — bash exited normally and reported that
    //      its child died by signal via the conventional code. The kernel
    //      did NOT mark the bash wait as WIFSIGNALED, so `signal()` is
    //      None; the signal number is in `(code - 128)`.
    //
    // Per REQ-BASH-006 and bash.allium:117 ("Signal information is
    // preserved on the `killed` state via the optional `signal_number`
    // field"), both paths map to FinalCause::Killed. The agent sees a
    // consistent "killed" semantic regardless of which wait path tripped.
    if let Some(sig) = status.signal() {
        FinalCause::Killed {
            exit_code: status.code(),
            signal_number: Some(sig),
        }
    } else if let Some(code) = status.code() {
        if (128..192).contains(&code) {
            FinalCause::Killed {
                exit_code: Some(code),
                signal_number: Some(code - 128),
            }
        } else {
            FinalCause::Exited {
                exit_code: Some(code),
            }
        }
    } else {
        FinalCause::Exited { exit_code: None }
    }
}

#[cfg(not(unix))]
fn exit_status_to_cause(status: std::process::ExitStatus) -> FinalCause {
    FinalCause::Exited {
        exit_code: status.code(),
    }
}

async fn race_run_response(
    handle: Arc<Handle>,
    cmd: &str,
    wait_seconds: u64,
    read_args: ReadArgs,
    ctx: &ToolContext,
) -> ToolOutput {
    let mut exit_rx = handle.exit_observer();
    let started = Instant::now();

    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            // Run cancellation: treat as still_running — the agent
            // can choose to peek/kill the handle later. We do not
            // proactively kill: that's what kill is for.
            still_running_response(&handle, started.elapsed(), &read_args, cmd).await
        }
        Ok(()) = exit_rx.changed() => {
            // Process exited (or waiter panicked). Either way, build the
            // appropriate response from current state.
            terminal_or_panic_response(&handle, &read_args, true, false, Some(cmd)).await
        }
        () = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            still_running_response(&handle, Duration::from_secs(wait_seconds), &read_args, cmd).await
        }
    }
}

// ---------------------------------------------------------------------------
// Peek
// ---------------------------------------------------------------------------

async fn run_peek(handle_id: &str, read_args: ReadArgs, ctx: &ToolContext) -> ToolOutput {
    let handle = match lookup_handle(ctx, handle_id).await {
        Ok(h) => h,
        Err(e) => return e.into_tool_output(),
    };
    shape_handle_response(&handle, &read_args, ResponseKind::Peek, None).await
}

// ---------------------------------------------------------------------------
// Wait
// ---------------------------------------------------------------------------

async fn run_wait(
    handle_id: &str,
    wait_seconds: u64,
    read_args: ReadArgs,
    ctx: &ToolContext,
) -> ToolOutput {
    let handle = match lookup_handle(ctx, handle_id).await {
        Ok(h) => h,
        Err(e) => return e.into_tool_output(),
    };

    // Tombstone fast-path: if already terminal, return the tombstoned
    // response immediately. Avoids the watch-channel-already-fired pitfall
    // (design.md "Watch-channel rule").
    if handle.state().await.is_terminal() {
        return shape_handle_response(&handle, &read_args, ResponseKind::Wait, None).await;
    }

    let mut exit_rx = handle.exit_observer();
    let started = Instant::now();
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            still_running_response(&handle, started.elapsed(), &read_args, &handle.cmd).await
        }
        Ok(()) = exit_rx.changed() => {
            terminal_or_panic_response(&handle, &read_args, false, false, None).await
        }
        () = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            // Re-timeout: SAME handle id (REQ-BASH-003).
            still_running_response(&handle, Duration::from_secs(wait_seconds), &read_args, &handle.cmd).await
        }
    }
}

// ---------------------------------------------------------------------------
// Kill
// ---------------------------------------------------------------------------

async fn run_kill(handle_id: &str, signal: KillSignal, ctx: &ToolContext) -> ToolOutput {
    let handle = match lookup_handle(ctx, handle_id).await {
        Ok(h) => h,
        Err(e) => return e.into_tool_output(),
    };

    // Already terminal? Return the tombstoned shape; no signal is sent.
    let state = handle.state().await;
    let pgid = match state.as_ref() {
        HandleState::Tombstoned(_) => {
            return shape_handle_response(
                &handle,
                &ReadArgs::default(),
                ResponseKind::Kill {
                    signal_sent: None,
                    pending: false,
                },
                None,
            )
            .await;
        }
        HandleState::Live(live) => live.pgid,
    };
    drop(state);

    // Send the signal EXACTLY ONCE (REQ-BASH-003).
    send_signal_to_group(pgid, signal);

    // Race exit observer vs KILL_RESPONSE_TIMEOUT.
    let mut exit_rx = handle.exit_observer();
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            // Cancellation during kill: surface current state as a kill response.
            shape_handle_response(
                &handle,
                &ReadArgs::default(),
                ResponseKind::Kill { signal_sent: Some(signal), pending: !handle.state().await.is_terminal() },
                None,
            )
            .await
        }
        Ok(()) = exit_rx.changed() => {
            // Process exited within the response window.
            shape_handle_response(
                &handle,
                &ReadArgs::default(),
                ResponseKind::Kill { signal_sent: Some(signal), pending: false },
                None,
            )
            .await
        }
        () = tokio::time::sleep(Duration::from_secs(KILL_RESPONSE_TIMEOUT_SECONDS)) => {
            // Mark kill_pending_kernel via the foundation helper.
            // The waiter task remains alive — a late exit will eventually
            // demote the handle to tombstoned.
            handle
                .mark_kill_pending_kernel(signal, SystemTime::now())
                .await;
            shape_handle_response(
                &handle,
                &ReadArgs::default(),
                ResponseKind::Kill { signal_sent: Some(signal), pending: true },
                None,
            )
            .await
        }
    }
}

#[cfg(unix)]
fn send_signal_to_group(pgid: i32, signal: KillSignal) {
    // SAFETY: kill(2) with negative pid signals the entire process group;
    // no memory implications. Errors (e.g., ESRCH if group already exited)
    // are not surfaced — the subsequent select! reacts to the actual exit
    // observer or the timeout.
    unsafe {
        let _ = libc::kill(-pgid, signal.as_libc());
    }
}

#[cfg(not(unix))]
fn send_signal_to_group(_pgid: i32, _signal: KillSignal) {
    // No-op on non-Unix.
}

// ---------------------------------------------------------------------------
// Lookup helper
// ---------------------------------------------------------------------------

async fn lookup_handle(ctx: &ToolContext, handle_id: &str) -> Result<Arc<Handle>, BashError> {
    let handles_arc: Arc<RwLock<ConversationHandles>> = ctx.bash_handles().await.map_err(|e| {
        // The accessor is currently infallible (returns Ok), but if it
        // ever fails we surface as handle_not_found-shaped. Use the
        // BashHandleError debug for the message.
        BashError::HandleNotFound {
            handle_id: format!("{handle_id} (registry error: {e:?})"),
        }
    })?;
    let handles = handles_arc.read().await;
    handles
        .get(&HandleId::new(handle_id.to_string()))
        .ok_or_else(|| BashError::HandleNotFound {
            handle_id: handle_id.to_string(),
        })
}

// ---------------------------------------------------------------------------
// Response shaping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum ResponseKind {
    Peek,
    Wait,
    Kill {
        signal_sent: Option<KillSignal>,
        pending: bool,
    },
}

#[allow(clippy::too_many_lines)]
async fn shape_handle_response(
    handle: &Arc<Handle>,
    read_args: &ReadArgs,
    kind: ResponseKind,
    // Optional command override (for run responses where we want the
    // original cmd text; for non-run we read handle.cmd).
    cmd_override: Option<&str>,
) -> ToolOutput {
    let state = handle.state().await;
    let cmd = cmd_override.unwrap_or(handle.cmd.as_str()).to_string();
    let label = handle.label.clone();
    let display = display_label(handle, kind);
    let signal_sent_top: Option<String> = match kind {
        ResponseKind::Kill {
            signal_sent: Some(sig),
            ..
        } => Some(sig.as_str().into()),
        _ => None,
    };

    let typed: BashResponse = match state.as_ref() {
        HandleState::Live(live) => {
            let kill_attempt = handle.kill_attempt().await;
            let is_kill_pending_kernel = kill_attempt.is_some();
            let ring = live.ring.lock().await;
            let view = read_window_from_ring(&ring, read_args);
            let window = window_to_typed(&view);
            drop(ring);

            let (kill_signal_str, kill_attempted_str) = match &kill_attempt {
                Some(a) => (
                    Some(a.signal_sent.as_str().to_string()),
                    Some(format_systime(a.attempted_at)),
                ),
                None => (None, None),
            };

            match kind {
                // Pending kill: kill response timer expired without an exit.
                ResponseKind::Kill { pending: true, .. } => {
                    BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
                        handle: handle.handle_id.to_string(),
                        cmd,
                        label: label.clone(),
                        window,
                        kill_signal_sent: kill_signal_str.unwrap_or_else(|| "TERM".into()),
                        kill_attempted_at: kill_attempted_str
                            .unwrap_or_else(|| format_systime(SystemTime::now())),
                        display,
                        signal_sent: signal_sent_top.unwrap_or_else(|| "TERM".into()),
                    })
                }
                // Kill resolved (pending=false) but state is still Live —
                // race we don't expect (waiter should have demoted). Fall
                // back to still_running shape with the in-flight kill
                // metadata, matching the prior JSON behaviour.
                ResponseKind::Kill { pending: false, .. } => {
                    BashResponse::StillRunning(BashStillRunningPayload {
                        handle: handle.handle_id.to_string(),
                        cmd,
                        label: label.clone(),
                        waited_ms: 0,
                        window,
                        kill_signal_sent: kill_signal_str,
                        kill_attempted_at: kill_attempted_str,
                    })
                }
                ResponseKind::Peek if is_kill_pending_kernel => {
                    BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
                        handle: handle.handle_id.to_string(),
                        cmd,
                        label: label.clone(),
                        window,
                        kill_signal_sent: kill_signal_str.unwrap_or_else(|| "TERM".into()),
                        kill_attempted_at: kill_attempted_str
                            .unwrap_or_else(|| format_systime(SystemTime::now())),
                        display,
                        // No `signal_sent` echo on peek.
                        signal_sent: String::new(),
                    })
                }
                ResponseKind::Wait if is_kill_pending_kernel => {
                    // The legacy code emitted status="kill_pending_kernel"
                    // here (no display field with the typed kill payload).
                    // To preserve the prior JSON shape — which carried the
                    // full Running shape (with display) — we use the
                    // KillPendingKernel typed payload but *without* an
                    // embedded `signal_sent` field by emitting an empty
                    // string. The downstream consumers branch on `status`,
                    // which is the reliable discriminator.
                    BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
                        handle: handle.handle_id.to_string(),
                        cmd,
                        label: label.clone(),
                        window,
                        kill_signal_sent: kill_signal_str.unwrap_or_else(|| "TERM".into()),
                        kill_attempted_at: kill_attempted_str
                            .unwrap_or_else(|| format_systime(SystemTime::now())),
                        display,
                        signal_sent: String::new(),
                    })
                }
                ResponseKind::Peek => BashResponse::Running(BashRunningPayload {
                    handle: handle.handle_id.to_string(),
                    cmd,
                    label: label.clone(),
                    window,
                    kill_signal_sent: kill_signal_str,
                    kill_attempted_at: kill_attempted_str,
                    display,
                    signal_sent: signal_sent_top,
                }),
                ResponseKind::Wait => BashResponse::StillRunning(BashStillRunningPayload {
                    handle: handle.handle_id.to_string(),
                    cmd,
                    label: label.clone(),
                    waited_ms: 0,
                    window,
                    kill_signal_sent: kill_signal_str,
                    kill_attempted_at: kill_attempted_str,
                }),
            }
        }
        HandleState::Tombstoned(t) => {
            let view = read_window_from_tombstone(t, read_args);
            let window = window_to_typed(&view);
            BashResponse::Tombstoned(BashTombstonedPayload {
                handle: handle.handle_id.to_string(),
                cmd,
                label: label.clone(),
                final_cause: final_cause_str(&t.final_cause).to_string(),
                exit_code: t.exit_code,
                signal_number: t.signal_number,
                duration_ms: t.duration_ms,
                finished_at: format_systime(t.finished_at),
                kill_signal_sent: t.kill_signal_sent.map(|s| s.as_str().to_string()),
                kill_attempted_at: t.kill_attempted_at.map(format_systime),
                window,
                display,
                signal_sent: signal_sent_top,
            })
        }
    };

    let mut value = serde_json::to_value(&typed).unwrap_or(Value::Null);
    // The `peek (kill_pending_kernel)` and `wait (kill_pending_kernel)`
    // branches above produce a typed payload with `signal_sent: ""` —
    // strip the empty echo to match the legacy wire shape (peek/wait do
    // not echo a signal_sent on the response).
    if let Value::Object(obj) = &mut value {
        if let Some(Value::String(s)) = obj.get("signal_sent") {
            if s.is_empty() {
                obj.remove("signal_sent");
            }
        }
    }
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::success(serialized).with_display(value)
}

/// Build a `still_running` response for run/wait/cancel paths. `elapsed`
/// is the wait window observed by the agent (for `waited_ms`).
async fn still_running_response(
    handle: &Arc<Handle>,
    elapsed: Duration,
    read_args: &ReadArgs,
    cmd: &str,
) -> ToolOutput {
    let state = handle.state().await;
    let kill_attempt = handle.kill_attempt().await;
    let waited_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let label = handle.label.clone();

    let window = if let HandleState::Live(live) = state.as_ref() {
        let ring = live.ring.lock().await;
        let view = read_window_from_ring(&ring, read_args);
        window_to_typed(&view)
    } else {
        BashRingWindow {
            start_offset: 0,
            end_offset: 0,
            truncated_before: false,
            lines: vec![],
        }
    };

    let kill_signal_str = kill_attempt
        .as_ref()
        .map(|a| a.signal_sent.as_str().to_string());
    let kill_attempted_str = kill_attempt
        .as_ref()
        .map(|a| format_systime(a.attempted_at));

    // If a kill is in flight, surface as kill_pending_kernel (REQ-BASH-003).
    let typed = if kill_attempt.is_some() {
        BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
            handle: handle.handle_id.to_string(),
            cmd: cmd.to_string(),
            label: label.clone(),
            window,
            kill_signal_sent: kill_signal_str.unwrap_or_else(|| "TERM".into()),
            kill_attempted_at: kill_attempted_str
                .unwrap_or_else(|| format_systime(SystemTime::now())),
            // No display label on run/wait still_running paths.
            display: String::new(),
            // No signal_sent echo: this is a passive wait, not a kill call.
            signal_sent: String::new(),
        })
    } else {
        BashResponse::StillRunning(BashStillRunningPayload {
            handle: handle.handle_id.to_string(),
            cmd: cmd.to_string(),
            label,
            waited_ms,
            window,
            kill_signal_sent: kill_signal_str,
            kill_attempted_at: kill_attempted_str,
        })
    };

    let mut value = serde_json::to_value(&typed).unwrap_or(Value::Null);
    if let Value::Object(obj) = &mut value {
        // Strip the empty `display` and `signal_sent` placeholders we set
        // for the kill_pending_kernel path here so the wire shape matches
        // the legacy still_running JSON (no display, no signal_sent on the
        // passive-wait code path).
        if let Some(Value::String(s)) = obj.get("display") {
            if s.is_empty() {
                obj.remove("display");
            }
        }
        if let Some(Value::String(s)) = obj.get("signal_sent") {
            if s.is_empty() {
                obj.remove("signal_sent");
            }
        }
        // Add waited_ms for the kill_pending_kernel still-running variant
        // emitted from this code path; the typed payload omits it because
        // KillPendingKernel doesn't carry it as a structural field, but
        // the legacy wire kept it for parity with still_running shape.
        if obj.get("status").and_then(Value::as_str) == Some("kill_pending_kernel")
            && !obj.contains_key("waited_ms")
        {
            obj.insert("waited_ms".into(), Value::Number(waited_ms.into()));
        }
    }
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::success(serialized).with_display(value)
}

/// Build a response that observed the exit watch (terminal or waiter panic).
async fn terminal_or_panic_response(
    handle: &Arc<Handle>,
    read_args: &ReadArgs,
    run_path: bool,
    cancelled: bool,
    cmd_override: Option<&str>,
) -> ToolOutput {
    let _ = cancelled; // unused for now; kept for symmetry with future cancel handling
    let state = handle.state().await;
    let label = handle.label.clone();
    match state.as_ref() {
        HandleState::Tombstoned(t) => {
            let cmd = cmd_override.unwrap_or(handle.cmd.as_str()).to_string();
            let view = read_window_from_tombstone(t, read_args);
            let window = window_to_typed(&view);

            let typed = if run_path {
                let payload = BashRunTombstonePayload {
                    handle: handle.handle_id.to_string(),
                    cmd,
                    label: label.clone(),
                    final_cause: final_cause_str(&t.final_cause).to_string(),
                    exit_code: t.exit_code,
                    signal_number: t.signal_number,
                    duration_ms: t.duration_ms,
                    finished_at: format_systime(t.finished_at),
                    kill_signal_sent: t.kill_signal_sent.map(|s| s.as_str().to_string()),
                    kill_attempted_at: t.kill_attempted_at.map(format_systime),
                    window,
                };
                match &t.final_cause {
                    FinalCause::Exited { .. } => BashResponse::Exited(payload),
                    FinalCause::Killed { .. } => BashResponse::Killed(payload),
                }
            } else {
                BashResponse::Tombstoned(BashTombstonedPayload {
                    handle: handle.handle_id.to_string(),
                    cmd,
                    label: label.clone(),
                    final_cause: final_cause_str(&t.final_cause).to_string(),
                    exit_code: t.exit_code,
                    signal_number: t.signal_number,
                    duration_ms: t.duration_ms,
                    finished_at: format_systime(t.finished_at),
                    kill_signal_sent: t.kill_signal_sent.map(|s| s.as_str().to_string()),
                    kill_attempted_at: t.kill_attempted_at.map(format_systime),
                    window,
                    // wait/peek path: no synthesized display label here
                    // (callers that want the label go through
                    // `shape_handle_response`).
                    display: String::new(),
                    signal_sent: None,
                })
            };

            let mut value = serde_json::to_value(&typed).unwrap_or(Value::Null);
            if let Value::Object(obj) = &mut value {
                if let Some(Value::String(s)) = obj.get("display") {
                    if s.is_empty() {
                        obj.remove("display");
                    }
                }
            }
            let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
            ToolOutput::success(serialized).with_display(value)
        }
        HandleState::Live(_live) => {
            // Live but exit observer fired? Likely waiter panic sentinel.
            let typed = BashResponse::WaiterPanicked(BashWaiterPanickedPayload {
                handle: handle.handle_id.to_string(),
                cmd: cmd_override.unwrap_or(handle.cmd.as_str()).to_string(),
                label,
                error_message:
                    "the waiter task for this handle panicked; the process state is unknown"
                        .to_string(),
            });
            let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
            let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
            ToolOutput::error(serialized).with_display(value)
        }
    }
}

fn read_window_from_ring(ring: &super::ring::RingBuffer, args: &ReadArgs) -> WindowView {
    if let Some(since) = args.since {
        ring.since(since)
    } else {
        let n = args.lines.unwrap_or(DEFAULT_PEEK_LINES);
        ring.tail(n)
    }
}

fn read_window_from_tombstone(tomb: &super::handle::Tombstone, args: &ReadArgs) -> WindowView {
    let lines = &tomb.final_tail;
    let next_offset = tomb.next_offset_at_exit;
    let earliest_offset = lines.first().map_or(next_offset, |l| l.offset);

    if let Some(since) = args.since {
        let effective_start = since.max(earliest_offset);
        let kept: Vec<RingLine> = lines
            .iter()
            .filter(|l| l.offset >= effective_start)
            .cloned()
            .collect();
        let view_start = kept.first().map_or(effective_start, |l| l.offset);
        WindowView {
            start_offset: view_start,
            end_offset: next_offset,
            // Truncated if the caller asked for offsets older than what
            // the tombstone retained, OR if eviction had advanced the live
            // ring's start_offset before demotion (we don't carry that
            // explicitly — the tombstone holds final_tail, so any line
            // before final_tail[0] is by definition no longer available).
            truncated_before: since < earliest_offset,
            lines: kept,
        }
    } else {
        let n = args.lines.unwrap_or(DEFAULT_PEEK_LINES);
        let total = lines.len();
        let take = n.min(total);
        let skip = total - take;
        let kept: Vec<RingLine> = lines.iter().skip(skip).cloned().collect();
        let view_start = kept.first().map_or(next_offset, |l| l.offset);
        // Tail-mode tombstone read: truncated_before is true iff the
        // tombstone tail does not cover from offset 0 (i.e., the live
        // ring evicted some lines before exit) OR the requested tail
        // does not include the earliest tombstone line.
        let truncated_before =
            earliest_offset > 0 || kept.first().map_or(view_start, |l| l.offset) > earliest_offset;
        WindowView {
            start_offset: view_start,
            end_offset: next_offset,
            truncated_before,
            lines: kept,
        }
    }
}

fn window_to_typed(view: &WindowView) -> BashRingWindow {
    BashRingWindow {
        start_offset: view.start_offset,
        end_offset: view.end_offset,
        truncated_before: view.truncated_before,
        lines: view
            .lines
            .iter()
            .map(|l| BashRingLine {
                offset: l.offset,
                bytes: String::from_utf8_lossy(&l.bytes).into_owned(),
            })
            .collect(),
    }
}

fn final_cause_str(cause: &FinalCause) -> &'static str {
    match cause {
        FinalCause::Exited { .. } => "exited",
        FinalCause::Killed { .. } => "killed",
    }
}

fn format_systime(t: SystemTime) -> String {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs_f64()),
        Err(_) => "0".into(),
    }
}

fn display_label(handle: &Arc<Handle>, kind: ResponseKind) -> String {
    match kind {
        ResponseKind::Peek => format!("peek {}", handle.handle_id),
        ResponseKind::Wait => format!("wait {}", handle.handle_id),
        ResponseKind::Kill {
            signal_sent: Some(sig),
            pending,
        } => {
            if pending {
                format!("kill {} ({}, pending)", handle.handle_id, sig.as_str())
            } else {
                format!("kill {} ({})", handle.handle_id, sig.as_str())
            }
        }
        ResponseKind::Kill {
            signal_sent: None, ..
        } => format!("kill {} (already terminal)", handle.handle_id),
    }
}
