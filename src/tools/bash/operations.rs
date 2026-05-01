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

use serde::Deserialize;
use serde_json::{json, Value};
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

/// Raw input shape. The `oneOf` constraint at the schema layer keeps mutual
/// exclusion structural for backends that validate the schema; we still
/// re-check at runtime per REQ-BASH-010.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBashInput {
    #[serde(default)]
    cmd: Option<String>,
    #[serde(default)]
    peek: Option<String>,
    #[serde(default)]
    wait: Option<String>,
    #[serde(default)]
    kill: Option<String>,
    #[serde(default)]
    wait_seconds: Option<i64>,
    #[serde(default)]
    signal: Option<String>,
    #[serde(default)]
    lines: Option<i64>,
    #[serde(default)]
    since: Option<i64>,
    #[serde(default)]
    mode: Option<String>,
    /// Legacy alias from the pre-handle revision. Some sub-agents and old
    /// fixtures still pass `command=...`; treat it as `cmd`.
    #[serde(default)]
    command: Option<String>,
}

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
    Spawn {
        cmd: String,
        wait_seconds: u64,
        read_args: ReadArgs,
        deprecation_notice: Option<String>,
    },
    Peek {
        handle_id: String,
        read_args: ReadArgs,
    },
    Wait {
        handle_id: String,
        wait_seconds: u64,
        read_args: ReadArgs,
        deprecation_notice: Option<String>,
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
    PeekArgsMutuallyExclusive,
    CommandSafetyRejected {
        reason: String,
    },
    SpawnFailed {
        error_message: String,
    },
    /// Either zero/multiple of `cmd|peek|wait|kill`, or `mode`+`wait_seconds`,
    /// or some other operation-key conflict (REQ-BASH-010).
    MutuallyExclusiveModes {
        message: String,
        conflicting_args: Vec<&'static str>,
        recommended_action: String,
        extra: Option<Value>,
    },
}

impl BashError {
    fn into_tool_output(self) -> ToolOutput {
        let value = match self {
            BashError::HandleNotFound { handle_id } => json!({
                "error": "handle_not_found",
                "error_message": format!("handle {handle_id} not found in this conversation"),
                "handle_id": handle_id,
                "hint": HANDLE_NOT_FOUND_HINT,
            }),
            BashError::HandleCapReached(BashHandleError::HandleCapReached {
                cap,
                live_handles,
            }) => {
                let live: Vec<Value> = live_handles
                    .iter()
                    .map(|s: &LiveHandleSummary| {
                        json!({
                            "handle": s.handle.as_str(),
                            "cmd": s.cmd,
                            "age_seconds": s.age_seconds,
                            "status": "running",
                        })
                    })
                    .collect();
                json!({
                    "error": "handle_cap_reached",
                    "error_message": format!(
                        "this conversation has reached the cap of {cap} live bash handles"
                    ),
                    "cap": cap,
                    "live_handles": live,
                    "hint": CAP_HINT,
                })
            }
            BashError::WaitSecondsOutOfRange { provided, max } => json!({
                "error": "wait_seconds_out_of_range",
                "error_message": format!(
                    "wait_seconds={provided} is out of range [0, {max}]; long-running operations \
                     should yield a handle and resume via wait calls"
                ),
                "provided": provided,
                "max_wait_seconds": max,
            }),
            BashError::PeekArgsMutuallyExclusive => json!({
                "error": "peek_args_mutually_exclusive",
                "error_message": "specify exactly one of lines or since",
            }),
            BashError::CommandSafetyRejected { reason } => json!({
                "error": "command_safety_rejected",
                "error_message": reason.clone(),
                "reason": reason,
            }),
            BashError::SpawnFailed { error_message } => json!({
                "error": "spawn_failed",
                "error_message": error_message,
            }),
            BashError::MutuallyExclusiveModes {
                message,
                conflicting_args,
                recommended_action,
                extra,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "error".into(),
                    Value::String("mutually_exclusive_modes".into()),
                );
                obj.insert("error_message".into(), Value::String(message));
                obj.insert(
                    "conflicting_args".into(),
                    Value::Array(
                        conflicting_args
                            .into_iter()
                            .map(|s| Value::String(s.into()))
                            .collect(),
                    ),
                );
                obj.insert(
                    "recommended_action".into(),
                    Value::String(recommended_action),
                );
                if let Some(Value::Object(extras)) = extra {
                    for (k, v) in extras {
                        obj.insert(k, v);
                    }
                }
                Value::Object(obj)
            }
        };

        let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
        ToolOutput::error(serialized).with_display(value)
    }
}

// ---------------------------------------------------------------------------
// Parsing & validation
// ---------------------------------------------------------------------------

/// Parse + validate the agent's request. Returns the typed `BashRequest`
/// or a `BashError` ready to be returned as the tool output.
#[allow(clippy::too_many_lines)]
pub fn parse_request(input: Value) -> Result<BashRequest, BashError> {
    let raw: RawBashInput = serde_json::from_value(input).map_err(|e| {
        // Schema-level rejection — the agent passed a malformed shape.
        BashError::MutuallyExclusiveModes {
            message: format!("invalid bash input: {e}"),
            conflicting_args: vec![],
            recommended_action:
                "send exactly one of cmd, peek, wait, kill, with the documented field types".into(),
            extra: None,
        }
    })?;

    // Operation-key mutual exclusion (REQ-BASH-010).
    let mut provided: Vec<&'static str> = Vec::new();
    if raw.cmd.is_some() || raw.command.is_some() {
        provided.push("cmd");
    }
    if raw.peek.is_some() {
        provided.push("peek");
    }
    if raw.wait.is_some() {
        provided.push("wait");
    }
    if raw.kill.is_some() {
        provided.push("kill");
    }
    if provided.len() != 1 {
        let message = if provided.is_empty() {
            "exactly one of cmd, peek, wait, kill must be provided".to_string()
        } else {
            format!(
                "exactly one of cmd, peek, wait, kill must be provided; received: {}",
                provided.join(", ")
            )
        };
        return Err(BashError::MutuallyExclusiveModes {
            message,
            conflicting_args: provided,
            recommended_action: "remove the extra operation keys; keep exactly one".into(),
            extra: None,
        });
    }

    // Mode/wait_seconds conflict (REQ-BASH-010).
    if raw.mode.is_some() && raw.wait_seconds.is_some() {
        let extra = json!({
            "mode": raw.mode,
            "wait_seconds": raw.wait_seconds,
        });
        return Err(BashError::MutuallyExclusiveModes {
            message: "the deprecated 'mode' parameter cannot be used with 'wait_seconds'; pass \
                 wait_seconds alone"
                .into(),
            conflicting_args: vec!["mode", "wait_seconds"],
            recommended_action: "remove the deprecated 'mode' parameter; pass 'wait_seconds' alone"
                .into(),
            extra: Some(extra),
        });
    }

    // Resolve wait_seconds. If `mode` is provided alone, map to its value
    // and produce a deprecation notice (REQ-BASH-010).
    let (effective_wait_seconds_opt, deprecation_notice) = if let Some(m) = raw.mode.as_deref() {
        let mapped = match m {
            "default" => 30u64,
            "slow" => 900u64,
            "background" => 0u64,
            _ => {
                return Err(BashError::MutuallyExclusiveModes {
                    message: format!(
                        "mode='{m}' is not recognized; valid values are 'default', 'slow', \
                         'background' (all deprecated — use wait_seconds instead)"
                    ),
                    conflicting_args: vec!["mode"],
                    recommended_action:
                        "drop the 'mode' parameter; pass wait_seconds (integer seconds) instead"
                            .into(),
                    extra: None,
                });
            }
        };
        let notice = format!(
            "the 'mode' parameter is deprecated and will be removed in the second Phoenix \
             release after this revision; pass wait_seconds={mapped} instead"
        );
        (Some(mapped), Some(notice))
    } else {
        (None, None)
    };

    // Read args (REQ-BASH-004).
    let read_args = parse_read_args(raw.lines, raw.since)?;

    // Dispatch.
    if raw.cmd.is_some() || raw.command.is_some() {
        let cmd = raw.cmd.or(raw.command).unwrap_or_default();
        if cmd.is_empty() {
            return Err(BashError::MutuallyExclusiveModes {
                message: "cmd must be a non-empty shell command".into(),
                conflicting_args: vec!["cmd"],
                recommended_action: "supply a non-empty cmd string".into(),
                extra: None,
            });
        }
        let wait_seconds = resolve_wait_seconds(raw.wait_seconds, effective_wait_seconds_opt)?;
        return Ok(BashRequest::Spawn {
            cmd,
            wait_seconds,
            read_args,
            deprecation_notice,
        });
    }
    if let Some(handle_id) = raw.peek {
        return Ok(BashRequest::Peek {
            handle_id,
            read_args,
        });
    }
    if let Some(handle_id) = raw.wait {
        let wait_seconds = resolve_wait_seconds(raw.wait_seconds, effective_wait_seconds_opt)?;
        return Ok(BashRequest::Wait {
            handle_id,
            wait_seconds,
            read_args,
            deprecation_notice,
        });
    }
    if let Some(handle_id) = raw.kill {
        let signal = match raw.signal.as_deref() {
            None | Some("TERM") => KillSignal::Term,
            Some("KILL") => KillSignal::Kill,
            Some(other) => {
                return Err(BashError::MutuallyExclusiveModes {
                    message: format!(
                        "signal='{other}' is not recognized; valid values are 'TERM' or 'KILL'"
                    ),
                    conflicting_args: vec!["signal"],
                    recommended_action: "use signal='TERM' (default) or signal='KILL'".into(),
                    extra: None,
                });
            }
        };
        return Ok(BashRequest::Kill { handle_id, signal });
    }
    unreachable!("provided.len() checked == 1 above")
}

fn parse_read_args(lines: Option<i64>, since: Option<i64>) -> Result<ReadArgs, BashError> {
    if lines.is_some() && since.is_some() {
        return Err(BashError::PeekArgsMutuallyExclusive);
    }
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
    let since = match since {
        None => None,
        Some(n) if n >= 0 => Some(u64::try_from(n).unwrap_or(0)),
        Some(_) => {
            return Err(BashError::MutuallyExclusiveModes {
                message: "since must be a non-negative integer".into(),
                conflicting_args: vec!["since"],
                recommended_action: "pass since as a non-negative integer offset".into(),
                extra: None,
            });
        }
    };
    Ok(ReadArgs { lines, since })
}

fn resolve_wait_seconds(raw: Option<i64>, from_mode: Option<u64>) -> Result<u64, BashError> {
    let value: i64 = match (raw, from_mode) {
        (Some(v), _) => v,
        (None, Some(m)) => {
            // mapped from mode — already in valid range.
            return Ok(m);
        }
        (None, None) => i64::try_from(DEFAULT_WAIT_SECONDS).unwrap_or(30),
    };
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
        BashRequest::Spawn {
            cmd,
            wait_seconds,
            read_args,
            deprecation_notice,
        } => run_spawn(&cmd, wait_seconds, read_args, deprecation_notice, &ctx).await,
        BashRequest::Peek {
            handle_id,
            read_args,
        } => run_peek(&handle_id, read_args, &ctx).await,
        BashRequest::Wait {
            handle_id,
            wait_seconds,
            read_args,
            deprecation_notice,
        } => {
            run_wait(
                &handle_id,
                wait_seconds,
                read_args,
                deprecation_notice,
                &ctx,
            )
            .await
        }
        BashRequest::Kill { handle_id, signal } => run_kill(&handle_id, signal, &ctx).await,
    }
}

// ---------------------------------------------------------------------------
// Spawn
// ---------------------------------------------------------------------------

async fn run_spawn(
    cmd: &str,
    wait_seconds: u64,
    read_args: ReadArgs,
    deprecation_notice: Option<String>,
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
    // lock so two concurrent spawns can't both race past the cap.
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
        // so no other spawn can race the cap check, then insert below.
        // Spawn is fast (a fork+exec) so this lock-hold is bounded.
        match spawn_child(cmd, ctx, handle_id.clone(), ring_bytes_cap) {
            Ok((handle, child)) => {
                let inserted = handles.insert(handle.clone());
                drop(handles);
                start_io_tasks(&inserted, child);
                race_spawn_response(
                    inserted,
                    cmd,
                    wait_seconds,
                    read_args,
                    deprecation_notice,
                    ctx,
                )
                .await
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
        pgid,
        pid,
        ring_bytes_cap,
    );
    Ok((handle, child))
}

fn start_io_tasks(handle: &Arc<Handle>, mut child: tokio::process::Child) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    if let Some(stdout) = stdout {
        let h = handle.clone();
        tokio::spawn(async move {
            read_pipe_to_ring(stdout, h, "stdout").await;
        });
    }
    if let Some(stderr) = stderr {
        let h = handle.clone();
        tokio::spawn(async move {
            read_pipe_to_ring(stderr, h, "stderr").await;
        });
    }

    // Waiter task: call wait() and demote the handle on exit.
    let h = handle.clone();
    tokio::spawn(async move {
        run_waiter(h, child).await;
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

async fn run_waiter(handle: Arc<Handle>, mut child: tokio::process::Child) {
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
    if let Some(sig) = status.signal() {
        // The bash wrapper was replaced by `exec <cmd>`, so a signal here
        // means the user command received the signal directly.
        FinalCause::Killed {
            exit_code: status.code(),
            signal_number: Some(sig),
        }
    } else if let Some(code) = status.code() {
        // Bash convention: exit code in [128, 192) is "killed by signal
        // (code-128)". This rarely fires under our `exec <cmd>` wrapping
        // (signals reach the user code directly), but if a non-exec'd
        // descendant trips it, recover the signal number for log
        // readability — the cause is still `Exited` because the kernel
        // returned a status code rather than reporting WIFSIGNALED.
        let signal_number = if (128..192).contains(&code) {
            Some(code - 128)
        } else {
            None
        };
        // signal_number is included on the Tombstone (it's a struct field
        // computed inside transition_to_terminal from FinalCause); but
        // FinalCause::Exited doesn't carry signal_number directly. The
        // foundation's transition_to_terminal only reads signal_number
        // from FinalCause::Killed. Per design.md "Waiter task" code: a
        // 128+signum exit is reported as FinalCause::Exited with the
        // signal in `signal_number` only when WIFSIGNALED was true.
        // For the conventional code path we report Exited with no signal
        // number — the convention is informational only and does not
        // change the agent-visible status.
        let _ = signal_number;
        FinalCause::Exited {
            exit_code: Some(code),
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

async fn race_spawn_response(
    handle: Arc<Handle>,
    cmd: &str,
    wait_seconds: u64,
    read_args: ReadArgs,
    deprecation_notice: Option<String>,
    ctx: &ToolContext,
) -> ToolOutput {
    let mut exit_rx = handle.exit_observer();
    let started = Instant::now();

    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            // Spawn cancellation: treat as still_running — the agent
            // can choose to peek/kill the handle later. We do not
            // proactively kill: that's what kill is for.
            still_running_response(&handle, started.elapsed(), &read_args, deprecation_notice.as_deref(), cmd).await
        }
        Ok(()) = exit_rx.changed() => {
            // Process exited (or waiter panicked). Either way, build the
            // appropriate response from current state.
            terminal_or_panic_response(&handle, &read_args, true, false, deprecation_notice.as_deref(), Some(cmd)).await
        }
        () = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            still_running_response(&handle, Duration::from_secs(wait_seconds), &read_args, deprecation_notice.as_deref(), cmd).await
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
    shape_handle_response(&handle, &read_args, ResponseKind::Peek, None, None).await
}

// ---------------------------------------------------------------------------
// Wait
// ---------------------------------------------------------------------------

async fn run_wait(
    handle_id: &str,
    wait_seconds: u64,
    read_args: ReadArgs,
    deprecation_notice: Option<String>,
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
        return shape_handle_response(
            &handle,
            &read_args,
            ResponseKind::Wait,
            deprecation_notice.as_deref(),
            None,
        )
        .await;
    }

    let mut exit_rx = handle.exit_observer();
    let started = Instant::now();
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            still_running_response(&handle, started.elapsed(), &read_args, deprecation_notice.as_deref(), &handle.cmd).await
        }
        Ok(()) = exit_rx.changed() => {
            terminal_or_panic_response(&handle, &read_args, false, false, deprecation_notice.as_deref(), None).await
        }
        () = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            // Re-timeout: SAME handle id (REQ-BASH-003).
            still_running_response(&handle, Duration::from_secs(wait_seconds), &read_args, deprecation_notice.as_deref(), &handle.cmd).await
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
    deprecation_notice: Option<&str>,
    // Optional command override (for spawn responses where we want the
    // original cmd text; for non-spawn we read handle.cmd).
    cmd_override: Option<&str>,
) -> ToolOutput {
    let state = handle.state().await;
    let mut obj = serde_json::Map::new();

    // status field per REQ-BASH-002/003/006.
    let cmd = cmd_override.unwrap_or(handle.cmd.as_str()).to_string();

    match state.as_ref() {
        HandleState::Live(live) => {
            // Determine kill_pending_kernel vs running by inspecting kill_attempt.
            let kill_attempt = handle.kill_attempt().await;
            let is_kill_pending_kernel = kill_attempt.is_some();
            let status = match kind {
                ResponseKind::Kill { pending, .. } => {
                    if pending {
                        "kill_pending_kernel"
                    } else {
                        // Kill response saw exit (pending=false) and yet
                        // state is still Live? Shouldn't happen because
                        // the waiter would have demoted; fall back to
                        // still_running.
                        "still_running"
                    }
                }
                ResponseKind::Peek | ResponseKind::Wait if is_kill_pending_kernel => {
                    "kill_pending_kernel"
                }
                ResponseKind::Peek => "running",
                ResponseKind::Wait => "still_running",
            };
            obj.insert("status".into(), Value::String(status.into()));
            obj.insert("handle".into(), Value::String(handle.handle_id.to_string()));
            obj.insert("cmd".into(), Value::String(cmd));

            let ring = live.ring.lock().await;
            let view = read_window_from_ring(&ring, read_args);
            insert_window(&mut obj, &view);

            if let Some(attempt) = kill_attempt {
                obj.insert(
                    "kill_signal_sent".into(),
                    Value::String(attempt.signal_sent.as_str().into()),
                );
                obj.insert(
                    "kill_attempted_at".into(),
                    Value::String(format_systime(attempt.attempted_at)),
                );
            }
        }
        HandleState::Tombstoned(t) => {
            obj.insert("status".into(), Value::String("tombstoned".into()));
            obj.insert("handle".into(), Value::String(handle.handle_id.to_string()));
            obj.insert("cmd".into(), Value::String(cmd));
            obj.insert(
                "final_cause".into(),
                Value::String(final_cause_str(&t.final_cause).into()),
            );
            if let Some(code) = t.exit_code {
                obj.insert("exit_code".into(), Value::Number(code.into()));
            } else {
                obj.insert("exit_code".into(), Value::Null);
            }
            if let Some(sig) = t.signal_number {
                obj.insert("signal_number".into(), Value::Number(sig.into()));
            }
            obj.insert("duration_ms".into(), Value::Number(t.duration_ms.into()));
            obj.insert(
                "finished_at".into(),
                Value::String(format_systime(t.finished_at)),
            );
            if let Some(sig) = t.kill_signal_sent {
                obj.insert(
                    "kill_signal_sent".into(),
                    Value::String(sig.as_str().into()),
                );
            }
            if let Some(at) = t.kill_attempted_at {
                obj.insert(
                    "kill_attempted_at".into(),
                    Value::String(format_systime(at)),
                );
            }
            let view = read_window_from_tombstone(t, read_args);
            insert_window(&mut obj, &view);
        }
    }

    // kind-specific fields.
    if let ResponseKind::Kill {
        signal_sent: Some(sig),
        ..
    } = kind
    {
        obj.insert("signal_sent".into(), Value::String(sig.as_str().into()));
    }

    if let Some(notice) = deprecation_notice {
        obj.insert(
            "deprecation_notice".into(),
            Value::String(notice.to_string()),
        );
    }

    // Display label per REQ-BASH-015 for non-spawn ops.
    obj.insert("display".into(), Value::String(display_label(handle, kind)));

    let value = Value::Object(obj);
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::success(serialized).with_display(value)
}

/// Build a `still_running` response for spawn/wait/cancel paths. `elapsed`
/// is the wait window observed by the agent (for `waited_ms`).
async fn still_running_response(
    handle: &Arc<Handle>,
    elapsed: Duration,
    read_args: &ReadArgs,
    deprecation_notice: Option<&str>,
    cmd: &str,
) -> ToolOutput {
    let state = handle.state().await;
    let kill_attempt = handle.kill_attempt().await;
    let mut obj = serde_json::Map::new();
    let status = if kill_attempt.is_some() {
        "kill_pending_kernel"
    } else {
        "still_running"
    };
    obj.insert("status".into(), Value::String(status.into()));
    obj.insert("handle".into(), Value::String(handle.handle_id.to_string()));
    obj.insert("cmd".into(), Value::String(cmd.to_string()));
    let waited_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    obj.insert("waited_ms".into(), Value::Number(waited_ms.into()));

    if let HandleState::Live(live) = state.as_ref() {
        let ring = live.ring.lock().await;
        let view = read_window_from_ring(&ring, read_args);
        insert_window(&mut obj, &view);
    }

    if let Some(attempt) = kill_attempt {
        obj.insert(
            "kill_signal_sent".into(),
            Value::String(attempt.signal_sent.as_str().into()),
        );
        obj.insert(
            "kill_attempted_at".into(),
            Value::String(format_systime(attempt.attempted_at)),
        );
    }
    if let Some(notice) = deprecation_notice {
        obj.insert(
            "deprecation_notice".into(),
            Value::String(notice.to_string()),
        );
    }

    let value = Value::Object(obj);
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::success(serialized).with_display(value)
}

/// Build a response that observed the exit watch (terminal or waiter panic).
async fn terminal_or_panic_response(
    handle: &Arc<Handle>,
    read_args: &ReadArgs,
    spawn_path: bool,
    cancelled: bool,
    deprecation_notice: Option<&str>,
    cmd_override: Option<&str>,
) -> ToolOutput {
    let _ = cancelled; // unused for now; kept for symmetry with future cancel handling
    let state = handle.state().await;
    match state.as_ref() {
        HandleState::Tombstoned(t) => {
            // For spawn responses, REQ-BASH-002 says use status = "exited" |
            // "killed" verbatim (NOT "tombstoned"). For wait/peek, the
            // bash.allium rule says return "tombstoned".
            let mut obj = serde_json::Map::new();
            let status = if spawn_path {
                match &t.final_cause {
                    FinalCause::Exited { .. } => "exited",
                    FinalCause::Killed { .. } => "killed",
                }
            } else {
                "tombstoned"
            };
            obj.insert("status".into(), Value::String(status.into()));
            obj.insert("handle".into(), Value::String(handle.handle_id.to_string()));
            let cmd = cmd_override.unwrap_or(handle.cmd.as_str()).to_string();
            obj.insert("cmd".into(), Value::String(cmd));
            obj.insert(
                "final_cause".into(),
                Value::String(final_cause_str(&t.final_cause).into()),
            );
            if let Some(code) = t.exit_code {
                obj.insert("exit_code".into(), Value::Number(code.into()));
            } else {
                obj.insert("exit_code".into(), Value::Null);
            }
            if let Some(sig) = t.signal_number {
                obj.insert("signal_number".into(), Value::Number(sig.into()));
            }
            obj.insert("duration_ms".into(), Value::Number(t.duration_ms.into()));
            obj.insert(
                "finished_at".into(),
                Value::String(format_systime(t.finished_at)),
            );
            if let Some(sig) = t.kill_signal_sent {
                obj.insert(
                    "kill_signal_sent".into(),
                    Value::String(sig.as_str().into()),
                );
            }
            if let Some(at) = t.kill_attempted_at {
                obj.insert(
                    "kill_attempted_at".into(),
                    Value::String(format_systime(at)),
                );
            }
            let view = read_window_from_tombstone(t, read_args);
            insert_window(&mut obj, &view);
            if let Some(notice) = deprecation_notice {
                obj.insert(
                    "deprecation_notice".into(),
                    Value::String(notice.to_string()),
                );
            }
            let value = Value::Object(obj);
            let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
            ToolOutput::success(serialized).with_display(value)
        }
        HandleState::Live(_live) => {
            // Live but exit observer fired? Likely waiter panic sentinel.
            let mut obj = serde_json::Map::new();
            obj.insert("status".into(), Value::String("waiter_panicked".into()));
            obj.insert("handle".into(), Value::String(handle.handle_id.to_string()));
            obj.insert(
                "cmd".into(),
                Value::String(cmd_override.unwrap_or(handle.cmd.as_str()).to_string()),
            );
            obj.insert(
                "error_message".into(),
                Value::String(
                    "the waiter task for this handle panicked; the process state is unknown".into(),
                ),
            );
            let value = Value::Object(obj);
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

fn insert_window(obj: &mut serde_json::Map<String, Value>, view: &WindowView) {
    obj.insert(
        "start_offset".into(),
        Value::Number(view.start_offset.into()),
    );
    obj.insert("end_offset".into(), Value::Number(view.end_offset.into()));
    obj.insert(
        "truncated_before".into(),
        Value::Bool(view.truncated_before),
    );
    let lines: Vec<Value> = view
        .lines
        .iter()
        .map(|l| {
            let text = String::from_utf8_lossy(&l.bytes).into_owned();
            json!({
                "offset": l.offset,
                "bytes": text,
            })
        })
        .collect();
    obj.insert("lines".into(), Value::Array(lines));
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
