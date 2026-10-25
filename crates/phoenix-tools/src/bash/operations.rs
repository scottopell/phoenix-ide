//! Bash tool agent-facing operations: spawn, peek, wait, kill.
//!
//! REQ-BASH-001/002/003/006/008/010/011: dispatch + response shaping for the
//! four operation kinds. Each operation produces a structured JSON envelope
//! delivered through [`ToolOutput`]. Errors use stable string identifiers
//! (REQ-BASH-008); successful operations carry `status` + handle metadata
//! per the response rules in `specs/bash/bash.allium` and the requirement-side
//! wire contracts in `specs/bash/requirements.md`.
//!
//! The request is a tagged enum (`BashRequest`) where each variant
//! corresponds to exactly one of `cmd / peek / wait / kill` — making the
//! "exactly one operation per call" mutual exclusion structurally
//! representable rather than runtime-checked.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::io::AsyncReadExt;

use serde_json::Value;
use tokio::process::Command;

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use super::handle::{
    BashLaunchIdentity, ExitState, ExitWatchPanicGuard, FinalCause, Handle, HandleId, HandleState,
    KillSignal, BASH_LAUNCH_UUID_ENV_VAR, TOMBSTONE_TAIL_LINES,
};
use super::registry::{BashHandleError, BashTerminalEffect, LiveHandleSummary, SpawnReservation};
use super::ring::RingBuffer;
use super::ring::{RingLine, WindowView};
use super::sandbox::ExploreSandboxLauncher;
use super::types::{BashOp, BashToolInput};
use super::ValidatedBashSpawnTarget;
use crate::{ResourceScopeKey, ToolContext, ToolOutput};
use phoenix_core::domain::bash_progress::{BashProgressLine, BashToolProgress};
use phoenix_core::domain::tool_wire::{
    BashErrorResponse, BashKillPendingKernelPayload, BashLaunchError, BashLiveHandleSummary,
    BashResponse, BashRingLine, BashRingWindow, BashRunTombstonePayload, BashRunningPayload,
    BashStillRunningPayload, BashTombstonedPayload, BashUnregisteredDescendants,
    BashUnregisteredRun, BashUnregisteredTerminalPayload, BashUnregisteredUnknownPayload,
    BashWaiterPanickedPayload,
};
#[cfg(test)]
use phoenix_core::process_identity::current_process_identity;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
fn rca_probe_record(
    pid: u32,
) -> (
    Result<
        phoenix_core::process_identity::ProcessIdentity,
        phoenix_core::process_identity::IdentityObservationFailure,
    >,
    String,
) {
    let observed = phoenix_core::process_identity::observe_process_identity(pid);
    let record = match &observed {
        Ok(identity) => format!(
            "category=present rc=none errno=none birth={}",
            identity.start_time
        ),
        Err(failure) => format!(
            "category={} rc={} errno={}",
            failure.kind(),
            failure
                .rc()
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            failure
                .errno()
                .map_or_else(|| "none".to_string(), |value| value.to_string())
        ),
    };
    (observed, record)
}

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

/// REQ-BASH-002 / REQ-BASH-010: display bound for the optional run-call
/// `label`. Longer presentation metadata is deterministically shortened.
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

impl ReadArgs {
    /// Construct read args from an optional incremental `since` offset.
    /// When `since` is absent, the read falls back to the default peek tail
    /// (`DEFAULT_PEEK_LINES`). Used by the process inspector's output delta
    /// (`specs/process-inspector/` REQ-PINSP-003), which exposes only the
    /// `since` cursor — not the `lines` count.
    #[must_use]
    pub fn from_since(since: Option<u64>) -> Self {
        Self { lines: None, since }
    }
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
    LaunchReleaseUnknown {
        handle: String,
        error_message: String,
    },
    SpawnFailed {
        error_message: String,
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
        extra: Option<Box<Value>>,
    },
}

impl BashError {
    fn into_tool_output(self) -> ToolOutput {
        let typed: BashErrorResponse = match self {
            BashError::HandleNotFound { handle_id } => BashErrorResponse::HandleNotFound {
                error_message: format!("handle {handle_id} not found in this work scope"),
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
                        "this work scope has reached the cap of {cap} live bash handles"
                    ),
                    cap,
                    live_handles: live,
                    hint: CAP_HINT.to_string(),
                }
            }
            BashError::HandleCapReached(BashHandleError::SpawnFenced) => {
                BashErrorResponse::SpawnFailed {
                    error_message: BashHandleError::SpawnFenced.to_string(),
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
            BashError::LaunchReleaseUnknown {
                handle,
                error_message,
            } => BashErrorResponse::LaunchReleaseUnknown {
                handle,
                error_message,
            },
            BashError::SpawnFailed { error_message } => {
                BashErrorResponse::SpawnFailed { error_message }
            }
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
                if let (Value::Object(obj), Some(extra)) = (&mut value, extra) {
                    if let Value::Object(extras) = *extra {
                        for (k, v) in extras {
                            obj.insert(k, v);
                        }
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
            let label = parse_label(raw.label);
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

/// Normalize the optional presentation-only `label` for `op=run`. Empty
/// strings (a frequent default-fill emission) are treated as absent.
fn parse_label(raw: Option<String>) -> Option<String> {
    const ELLIPSIS: char = '…';
    let label = raw.filter(|s| !s.is_empty())?;
    let chars: Vec<char> = label.chars().collect();
    if chars.len() <= MAX_LABEL_LENGTH {
        return Some(label);
    }
    let prefix_len = (MAX_LABEL_LENGTH - 1) * 3 / 4;
    let suffix_len = MAX_LABEL_LENGTH - 1 - prefix_len;
    let mut normalized = String::with_capacity(MAX_LABEL_LENGTH * 4);
    normalized.extend(chars[..prefix_len].iter());
    normalized.push(ELLIPSIS);
    normalized.extend(chars[chars.len() - suffix_len..].iter());
    Some(normalized)
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

#[derive(Debug, Clone)]
pub enum BashSpawnMode {
    Direct,
    ExploreReadOnly,
    ExplicitTarget(ValidatedBashSpawnTarget),
}

#[derive(Debug, Clone)]
struct SpawnContext {
    working_dir: std::path::PathBuf,
    lifecycle_scope: ResourceScopeKey,
    terminal_effect: BashTerminalEffect,
}

impl BashSpawnMode {
    fn authority(&self) -> phoenix_core::work_scope::ResourceAuthority {
        match self {
            Self::Direct | Self::ExplicitTarget(_) => {
                phoenix_core::work_scope::ResourceAuthority::Work
            }
            Self::ExploreReadOnly => phoenix_core::work_scope::ResourceAuthority::Restricted,
        }
    }
}

/// Run a bash request end-to-end and produce the `ToolOutput`.
pub async fn dispatch(input: Value, ctx: ToolContext) -> ToolOutput {
    dispatch_with_spawn_mode(input, ctx, BashSpawnMode::Direct).await
}

pub async fn dispatch_sandboxed(input: Value, ctx: ToolContext) -> ToolOutput {
    dispatch_with_spawn_mode(input, ctx, BashSpawnMode::ExploreReadOnly).await
}

pub async fn dispatch_explicit_target(
    input: Value,
    target: ValidatedBashSpawnTarget,
    ctx: ToolContext,
) -> ToolOutput {
    dispatch_with_spawn_mode(input, ctx, BashSpawnMode::ExplicitTarget(target)).await
}

async fn dispatch_with_spawn_mode(
    input: Value,
    ctx: ToolContext,
    spawn_mode: BashSpawnMode,
) -> ToolOutput {
    // Dispatch kind, not caller convention, selects effective Bash authority.
    let ctx = ctx.with_resource_authority(spawn_mode.authority());
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
        } => {
            let spawn_context = match &spawn_mode {
                BashSpawnMode::Direct | BashSpawnMode::ExploreReadOnly => SpawnContext {
                    working_dir: ctx.working_dir().to_path_buf(),
                    lifecycle_scope: ctx.work_scope.clone(),
                    terminal_effect: BashTerminalEffect::InventoryAndBranchReconcile,
                },
                BashSpawnMode::ExplicitTarget(target) => SpawnContext {
                    working_dir: target.working_dir.clone(),
                    lifecycle_scope: ResourceScopeKey::Work(target.lifecycle_scope.clone()),
                    terminal_effect: BashTerminalEffect::InventoryOnly,
                },
            };
            run_run(
                &cmd,
                label,
                wait_seconds,
                read_args,
                &ctx,
                &spawn_context,
                spawn_mode,
            )
            .await
        }
        BashRequest::Peek {
            handle_id,
            read_args,
        } => run_peek(&handle_id, read_args, &ctx).await,
        BashRequest::Wait {
            handle_id,
            wait_seconds,
            read_args,
        } => run_wait(&handle_id, wait_seconds, read_args, &ctx).await,
        BashRequest::Kill { handle_id, signal } => {
            run_kill(
                &handle_id,
                signal,
                Duration::from_secs(KILL_RESPONSE_TIMEOUT_SECONDS),
                &ctx,
            )
            .await
        }
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
    spawn_context: &SpawnContext,
    spawn_mode: BashSpawnMode,
) -> ToolOutput {
    run_run_with_identity_probe_mode(
        cmd,
        label,
        wait_seconds,
        read_args,
        ctx,
        spawn_context,
        spawn_mode,
        if cfg!(test) { SpawnProtocol::Legacy } else { SpawnProtocol::Gated },
        |pid| match phoenix_core::process_identity::observe_process_identity(pid) {
            Ok(identity) => Some(identity),
            Err(cause) => {
                tracing::warn!(
                    pid,
                    cause = cause.kind(),
                    rc = cause.rc(),
                    errno = cause.errno(),
                    "bash child identity unavailable; execution outcome may be ambiguous and automatic retry is unsafe"
                );
                None
            }
        },
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnProtocol {
    Legacy,
    Gated,
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn run_run_with_identity_probe(
    cmd: &str,
    label: Option<String>,
    wait_seconds: u64,
    read_args: ReadArgs,
    ctx: &ToolContext,
    spawn_context: &SpawnContext,
    spawn_mode: BashSpawnMode,
    probe: impl FnOnce(u32) -> Option<phoenix_core::process_identity::ProcessIdentity>,
) -> ToolOutput {
    run_run_with_identity_probe_mode(
        cmd,
        label,
        wait_seconds,
        read_args,
        ctx,
        spawn_context,
        spawn_mode,
        SpawnProtocol::Legacy,
        probe,
    )
    .await
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_run_with_identity_probe_mode(
    cmd: &str,
    label: Option<String>,
    wait_seconds: u64,
    read_args: ReadArgs,
    ctx: &ToolContext,
    spawn_context: &SpawnContext,
    spawn_mode: BashSpawnMode,
    protocol: SpawnProtocol,
    probe: impl FnOnce(u32) -> Option<phoenix_core::process_identity::ProcessIdentity>,
) -> ToolOutput {
    // REQ-BASH-011: command safety is enforced upstream by the permission seam
    // (specs/permissions, the deterministic deny layer) before this tool runs,
    // not here — see `crate::bash_check`, which the seam invokes. No second
    // check at the tool boundary keeps enforcement single-homed.

    if !matches!(
        spawn_context.lifecycle_scope,
        ResourceScopeKey::Work(_) | ResourceScopeKey::Unattached(_)
    ) {
        return BashError::SpawnFailed {
            error_message: "bash processes require a conversation-scoped owner".to_string(),
        }
        .into_tool_output();
    }

    let registry = ctx.bash_handle_registry().clone();
    let mut reservation = match registry.reserve_spawn(&spawn_context.lifecycle_scope).await {
        Ok(reservation) => reservation,
        Err(BashHandleError::HandleCapReached { cap, live_handles }) => {
            return BashError::HandleCapReached(BashHandleError::HandleCapReached {
                cap,
                live_handles,
            })
            .into_tool_output();
        }
        Err(BashHandleError::SpawnFenced) => {
            return BashError::SpawnFailed {
                error_message: BashHandleError::SpawnFenced.to_string(),
            }
            .into_tool_output();
        }
    };

    let ring_bytes_cap = registry.ring_bytes_cap();
    let launched = spawn_child_with_identity_probe_protocol(
        cmd,
        label.clone(),
        ctx,
        spawn_context,
        reservation.handle_id().clone(),
        ring_bytes_cap,
        &spawn_mode,
        protocol,
        probe,
    )
    .map(|(handle, spawned)| {
        let started_at = spawned.started_at;
        let (child, launch_gate, sandbox_scratch_dir) = spawned.into_waiter_parts();
        (
            handle,
            UnregisteredChild {
                child,
                launch_gate,
                sandbox_scratch_dir,
                started_at,
            },
        )
    });
    match &launched {
        Ok(_) => reservation.mark_launched(),
        Err(SpawnChildError::Unregistered(_)) => reservation.mark_unregistered(),
        Err(SpawnChildError::NotSpawned(_)) => {}
    }
    let fallback_cmd = cmd.to_string();
    let fallback_label = label.clone();
    let cmd = cmd.to_string();
    let ctx = ctx.clone();
    let spawn_context = spawn_context.clone();
    let owner = tokio::spawn(async move {
        let cmd = cmd.as_str();
        let ctx = &ctx;
        match launched {
            Ok((handle, spawned)) => {
                let gated_launch = spawned.launch_gate.is_some();
                let commit = async {
                    if gated_launch {
                        registry
                            .commit_spawn_gated(&mut reservation, handle.clone())
                            .await
                    } else {
                        registry
                            .commit_spawn(&mut reservation, handle.clone())
                            .await
                    }
                };
                let Ok(Ok(inserted)) = tokio::time::timeout(REGISTRATION_TIMEOUT, commit).await
                else {
                    return settle_unregistered_run(
                        spawned,
                        reservation,
                        cmd,
                        label,
                        ring_bytes_cap,
                        &read_args,
                        BashLaunchError::RegistrationFailed,
                    )
                    .await;
                };
                // Spawn edge: a new bash handle exists for this scope. Emit a
                // work-scope state-transition signal (REQ-WSUI-007). The
                // detached waiter started below carries the sink so it can
                // emit the terminal edge off-thread.
                registry.emit_lifecycle(
                    &spawn_context.lifecycle_scope,
                    Some(handle.handle_id.clone()),
                    crate::bash::registry::BashLifecyclePhase::Spawned,
                    None,
                    None,
                );
                let progress_reporter = ctx
                    .bash_progress_sink()
                    .map(|sink| Arc::new(LiveBashProgressReporter::new(sink)));
                let _progress_lease = progress_reporter
                    .as_ref()
                    .map(|reporter| LiveBashProgressLease(reporter.clone()));
                let UnregisteredChild {
                    child,
                    mut launch_gate,
                    sandbox_scratch_dir,
                    ..
                } = spawned;
                start_io_tasks(
                    &inserted,
                    spawn_context.lifecycle_scope.clone(),
                    child,
                    registry.lifecycle_sink(),
                    spawn_context.terminal_effect,
                    sandbox_scratch_dir,
                    progress_reporter.clone(),
                );
                if let Some(gate) = launch_gate.take() {
                    if let Err(error) =
                        release_registered_gate(gate, reservation, &registry, handle.clone()).await
                    {
                        tracing::error!(?error, handle = %handle.handle_id, "registered bash launch gate release failed; command execution may have started and outcome is uncertain");
                        return error.into_tool_output();
                    }
                }
                race_run_response(inserted, cmd, wait_seconds, read_args, ctx).await
            }
            Err(SpawnChildError::NotSpawned(error_message)) => {
                registry.abort_spawn(reservation);
                BashError::SpawnFailed { error_message }.into_tool_output()
            }
            Err(SpawnChildError::Unregistered(child)) => {
                settle_unregistered_run(
                    *child,
                    reservation,
                    cmd,
                    label,
                    ring_bytes_cap,
                    &read_args,
                    BashLaunchError::IdentityUnavailable,
                )
                .await
            }
        }
    });
    owner.await.unwrap_or_else(|error| {
        tracing::error!(%error, "bash launch custody task failed; outcome unknown");
        let ring = RingBuffer::new(ring_bytes_cap);
        let read = read_window_from_ring(&ring, &ReadArgs::default());
        unregistered_tool_output(
            BashUnregisteredRun {
                cmd: fallback_cmd,
                label: fallback_label,
                launch_error: BashLaunchError::RegistrationFailed,
                descendants: BashUnregisteredDescendants::Unverified,
                output_complete: false,
                duration_ms: 0,
                window: window_to_typed(&read.view, read.partial),
            },
            None,
        )
    })
}

#[derive(Debug)]
enum SpawnChildError {
    NotSpawned(String),
    Unregistered(Box<UnregisteredChild>),
}

impl From<String> for SpawnChildError {
    fn from(message: String) -> Self {
        Self::NotSpawned(message)
    }
}

#[derive(Debug)]
struct UnregisteredChild {
    child: tokio::process::Child,
    launch_gate: Option<LaunchGate>,
    sandbox_scratch_dir: Option<std::path::PathBuf>,
    started_at: Instant,
}

#[derive(Debug)]
struct LaunchGate {
    writer: tokio::process::ChildStdin,
    payload: Vec<u8>,
}

impl LaunchGate {
    async fn release(self) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let mut writer = self.writer;
        match tokio::time::timeout(LAUNCH_RELEASE_TIMEOUT, async {
            writer
                .write_all(&self.payload)
                .await
                .map_err(|error| format!("failed to write bash launch payload: {error}"))?;
            writer
                .shutdown()
                .await
                .map_err(|error| format!("failed to close bash launch gate: {error}"))
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!(
                "bash launch payload write/close exceeded {LAUNCH_RELEASE_TIMEOUT:?}"
            )),
        }
    }
}

async fn release_registered_gate(
    gate: LaunchGate,
    mut reservation: SpawnReservation,
    registry: &super::registry::BashHandleRegistry,
    handle: Arc<Handle>,
) -> Result<(), BashError> {
    match gate.release().await {
        Ok(()) => {
            registry.abort_spawn(reservation);
            Ok(())
        }
        Err(error) => {
            reservation.mark_unregistered();
            registry.abort_spawn(reservation);
            Err(BashError::LaunchReleaseUnknown {
                handle: handle.handle_id.to_string(),
                error_message: format!(
                    "bash launch release failed ({error}); command execution may have started. Do not retry automatically; the command may have executed."
                ),
            })
        }
    }
}

const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(1);
const LAUNCH_RELEASE_TIMEOUT: Duration = Duration::from_secs(1);
const UNREGISTERED_SETTLEMENT_TIMEOUT: Duration = Duration::from_millis(250);
const DIRECT_CHILD_KILL_TIMEOUT: Duration = Duration::from_secs(1);

async fn settle_direct_child(child: &mut tokio::process::Child) -> Option<FinalCause> {
    match child.try_wait() {
        Ok(Some(status)) => return Some(exit_status_to_cause(status)),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(%error, errno = error.raw_os_error(), "unregistered bash initial try_wait failed; outcome unknown");
            return None;
        }
    }
    match tokio::time::timeout(UNREGISTERED_SETTLEMENT_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => return Some(exit_status_to_cause(status)),
        Ok(Err(error)) => {
            tracing::error!(%error, errno = error.raw_os_error(), "unregistered bash wait failed; no further signal attempted");
            return None;
        }
        Err(_) => {}
    }
    match child.try_wait() {
        Ok(Some(status)) => return Some(exit_status_to_cause(status)),
        Ok(None) => {}
        Err(error) => {
            tracing::error!(%error, errno = error.raw_os_error(), "unregistered bash try_wait failed; no further signal attempted");
            return None;
        }
    }
    // Child owns this unreaped process; start_kill never targets its process group.
    if let Err(error) = child.start_kill() {
        tracing::error!(%error, errno = error.raw_os_error(), "unregistered bash direct-child termination failed");
    }
    match tokio::time::timeout(DIRECT_CHILD_KILL_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => Some(exit_status_to_cause(status)),
        Ok(Err(error)) => {
            tracing::error!(%error, errno = error.raw_os_error(), "unregistered bash reap failed");
            None
        }
        Err(_) => {
            tracing::error!(
                "unregistered bash direct-child settlement deadline elapsed; outcome remains unknown"
            );
            None
        }
    }
}

async fn drain_unregistered_readers(
    mut readers: tokio::task::JoinSet<std::io::Result<()>>,
    ring: Arc<tokio::sync::Mutex<RingBuffer>>,
) -> bool {
    let complete = tokio::time::timeout(READER_DRAIN_TIMEOUT, async {
        let mut complete = true;
        while let Some(result) = readers.join_next().await {
            complete &= matches!(result, Ok(Ok(())));
        }
        complete
    })
    .await
    .unwrap_or(false);
    readers.abort_all();
    while readers.join_next().await.is_some() {}
    ring.lock().await.flush_partial();
    if !complete {
        tracing::warn!(
            "unregistered bash output capture incomplete; closing bounded readers without signaling child or descendants"
        );
    }
    complete
}

async fn read_unregistered_pipe<R: tokio::io::AsyncRead + Unpin>(
    mut pipe: R,
    ring: Arc<tokio::sync::Mutex<RingBuffer>>,
) -> std::io::Result<()> {
    use tokio::io::AsyncReadExt;
    let mut bytes = [0; 4096];
    loop {
        let count = pipe.read(&mut bytes).await?;
        if count == 0 {
            return Ok(());
        }
        ring.lock().await.append(&bytes[..count]);
    }
}

fn settle_unregistered_run(
    mut unregistered: UnregisteredChild,
    mut reservation: SpawnReservation,
    cmd: &str,
    label: Option<String>,
    ring_bytes_cap: usize,
    read_args: &ReadArgs,
    launch_error: BashLaunchError,
) -> impl std::future::Future<Output = ToolOutput> + Send + 'static {
    drop(unregistered.launch_gate.take());
    let started = unregistered.started_at;
    let cmd = cmd.to_string();
    let read_args = read_args.clone();
    reservation.mark_unregistered();
    tracing::warn!(
        ?launch_error,
        "bash child launched without a registered handle; bounded direct-child settlement, never retrying"
    );
    let ring = Arc::new(tokio::sync::Mutex::new(RingBuffer::new(ring_bytes_cap)));
    let mut readers = tokio::task::JoinSet::new();
    if let Some(stdout) = unregistered.child.stdout.take() {
        readers.spawn(read_unregistered_pipe(stdout, ring.clone()));
    }
    if let Some(stderr) = unregistered.child.stderr.take() {
        readers.spawn(read_unregistered_pipe(stderr, ring.clone()));
    }
    let capture = ring.clone();
    let owner = tokio::spawn(async move {
        let cause = settle_direct_child(&mut unregistered.child).await;
        let output_complete = if cause.is_some() {
            drain_unregistered_readers(readers, capture).await
        } else {
            tracing::warn!(
                "unregistered bash settlement exhausted; closing capture as explicitly incomplete while direct-child outcome remains unknown"
            );
            readers.abort_all();
            while readers.join_next().await.is_some() {}
            capture.lock().await.flush_partial();
            false
        };
        if cause.is_some() {
            // Descendants are unverified and may still use the sandbox scratch.
            if unregistered.sandbox_scratch_dir.is_some() {
                tracing::warn!(
                    "retaining unregistered bash sandbox scratch: descendant shutdown is unverified"
                );
            }
        }
        drop(unregistered);
        drop(reservation);
        (cause, output_complete)
    });
    async move {
        let (cause, output_complete) = owner.await.unwrap_or((None, false));
        let ring = ring.lock().await;
        let read = read_window_from_ring(&ring, &read_args);
        unregistered_tool_output(
            BashUnregisteredRun {
                cmd,
                label,
                launch_error,
                descendants: BashUnregisteredDescendants::Unverified,
                output_complete: output_complete && ring.start_offset() == 0,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                window: window_to_typed(&read.view, read.partial),
            },
            cause.as_ref(),
        )
    }
}

fn unregistered_tool_output(run: BashUnregisteredRun, cause: Option<&FinalCause>) -> ToolOutput {
    let is_error = cause.is_none();
    let response = match cause {
        Some(FinalCause::Exited { exit_code }) => BashResponse::UnregisteredExited(BashUnregisteredTerminalPayload { run, exit_code: *exit_code, signal_number: None }),
        Some(FinalCause::Killed { exit_code, signal_number }) => BashResponse::UnregisteredKilled(BashUnregisteredTerminalPayload { run, exit_code: *exit_code, signal_number: *signal_number }),
        None => BashResponse::OutcomeUnknown(BashUnregisteredUnknownPayload {
            run,
            error: "launch_outcome_unknown".to_string(),
            error_message: "The command was launched but its outcome is unverified. It may have performed side effects and may still be running. Do not automatically retry. No controllable handle was registered; descendants are unverified.".to_string(),
        }),
    };
    let value = serde_json::to_value(response).expect("unregistered run response serializes");
    let serialized = value.to_string();
    if is_error {
        ToolOutput::error(serialized).with_display(value)
    } else {
        ToolOutput::success(serialized).with_display(value)
    }
}

struct SpawnedProcessGroup {
    child: Option<tokio::process::Child>,
    launch_gate: Option<LaunchGate>,
    started_at: Instant,
    pgid: i32,
    sandbox_scratch_dir: Option<std::path::PathBuf>,
}

impl SpawnedProcessGroup {
    fn new(
        mut child: tokio::process::Child,
        pgid: i32,
        sandbox_scratch_dir: Option<std::path::PathBuf>,
        started_at: Instant,
        payload: Option<Vec<u8>>,
    ) -> Self {
        let launch_gate = payload.map(|payload| LaunchGate {
            writer: child.stdin.take().expect("gated launcher stdin is piped"),
            payload,
        });
        Self {
            child: Some(child),
            launch_gate,
            started_at,
            pgid,
            sandbox_scratch_dir,
        }
    }

    fn into_waiter_parts(
        mut self,
    ) -> (
        tokio::process::Child,
        Option<LaunchGate>,
        Option<std::path::PathBuf>,
    ) {
        let child = self
            .child
            .take()
            .expect("spawned child already transferred");
        (
            child,
            self.launch_gate.take(),
            self.sandbox_scratch_dir.take(),
        )
    }

    #[cfg(test)]
    fn child_mut(&mut self) -> &mut tokio::process::Child {
        self.child
            .as_mut()
            .expect("spawned child already transferred")
    }

    #[cfg(test)]
    fn pgid(&self) -> i32 {
        self.pgid
    }
}

impl Drop for SpawnedProcessGroup {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }
        #[cfg(unix)]
        unsafe {
            // SAFETY: a negative pid targets the process group created by
            // setpgid below. The direct child also has kill_on_drop enabled.
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        if let Some(dir) = self.sandbox_scratch_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

// `pgid` and `pid` mirror the Allium-spec field names; renaming for the
// similar-names lint would diverge from the spec.
#[cfg(test)]
#[allow(clippy::similar_names)]
fn spawn_child(
    cmd: &str,
    label: Option<String>,
    ctx: &ToolContext,
    spawn_context: &SpawnContext,
    handle_id: HandleId,
    ring_bytes_cap: usize,
    spawn_mode: &BashSpawnMode,
) -> Result<(Arc<Handle>, SpawnedProcessGroup), SpawnChildError> {
    spawn_child_with_identity_probe(
        cmd,
        label,
        ctx,
        spawn_context,
        handle_id,
        ring_bytes_cap,
        spawn_mode,
        current_process_identity,
    )
}

#[cfg(test)]
#[allow(clippy::similar_names, clippy::too_many_arguments)]
fn spawn_child_with_identity_probe(
    cmd: &str,
    label: Option<String>,
    ctx: &ToolContext,
    spawn_context: &SpawnContext,
    handle_id: HandleId,
    ring_bytes_cap: usize,
    spawn_mode: &BashSpawnMode,
    probe: impl FnOnce(u32) -> Option<phoenix_core::process_identity::ProcessIdentity>,
) -> Result<(Arc<Handle>, SpawnedProcessGroup), SpawnChildError> {
    spawn_child_with_identity_probe_protocol(
        cmd,
        label,
        ctx,
        spawn_context,
        handle_id,
        ring_bytes_cap,
        spawn_mode,
        SpawnProtocol::Legacy,
        probe,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn spawn_child_with_identity_probe_protocol(
    cmd: &str,
    label: Option<String>,
    ctx: &ToolContext,
    spawn_context: &SpawnContext,
    handle_id: HandleId,
    ring_bytes_cap: usize,
    spawn_mode: &BashSpawnMode,
    protocol: SpawnProtocol,
    probe: impl FnOnce(u32) -> Option<phoenix_core::process_identity::ProcessIdentity>,
) -> Result<(Arc<Handle>, SpawnedProcessGroup), SpawnChildError> {
    // Per bash.allium @guidance on HandleSpawned:
    //   "Spawn child via Command::new(\"bash\").args([\"-c\", cmd]) with
    //    pre_exec(setpgid(0,0))"
    // The user-facing bash contract still runs `bash -c <cmd>` (REQ-BASH-001),
    // and the legacy design material's `exec <cmd>` variant is intentionally
    // not used here for the reasons below.
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
    let mut sandbox_scratch_dir = None;
    let launch_uuid = uuid::Uuid::new_v4().to_string();
    let use_gate = protocol == SpawnProtocol::Gated;
    let payload = if use_gate {
        #[cfg(unix)]
        {
            #[cfg(any(test, feature = "test-support"))]
            let executable = std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE")
                .map(std::path::PathBuf::from)
                .map(Ok)
                .unwrap_or_else(|| {
                    std::env::current_exe()
                        .map_err(|error| format!("failed to resolve phoenix executable: {error}"))
                })?;
            #[cfg(not(any(test, feature = "test-support")))]
            let executable = std::env::current_exe()
                .map_err(|error| format!("failed to resolve phoenix executable: {error}"))?;
            let (mode, working_dir, mut environment): (
                super::launch_protocol::BashLaunchMode,
                std::path::PathBuf,
                Vec<(std::ffi::OsString, std::ffi::OsString)>,
            ) = match spawn_mode {
                BashSpawnMode::ExploreReadOnly => {
                    let sandbox = ExploreSandboxLauncher::command(cmd, &spawn_context.working_dir)?;
                    sandbox_scratch_dir = Some(sandbox.scratch_dir);
                    let working_dir = sandbox
                        .command
                        .get_current_dir()
                        .ok_or_else(|| {
                            "Explore sandbox launcher omitted its working directory".to_string()
                        })?
                        .to_path_buf();
                    let environment = sandbox
                        .command
                        .get_envs()
                        .filter_map(|(key, value)| {
                            value.map(|value| (key.to_os_string(), value.to_os_string()))
                        })
                        .collect();
                    (
                        super::launch_protocol::BashLaunchMode::ExploreReadOnly,
                        working_dir,
                        environment,
                    )
                }
                BashSpawnMode::Direct | BashSpawnMode::ExplicitTarget(_) => (
                    super::launch_protocol::BashLaunchMode::Direct,
                    spawn_context.working_dir.clone(),
                    std::env::vars_os().collect(),
                ),
            };
            environment.retain(|(key, _)| key != BASH_LAUNCH_UUID_ENV_VAR);
            environment.push((BASH_LAUNCH_UUID_ENV_VAR.into(), launch_uuid.clone().into()));
            let payload = super::launch_protocol::BashLaunchPayload {
                mode,
                command: cmd.into(),
                working_dir,
                environment,
            }
            .encode()?;
            Some((executable, payload))
        }
        #[cfg(not(unix))]
        {
            return Err(SpawnChildError::NotSpawned(
                "gated bash launcher requires Unix exec".into(),
            ));
        }
    } else {
        None
    };
    let mut command = match (spawn_mode, payload.as_ref()) {
        (_, Some((executable, _))) => {
            let mut command = Command::new(executable);
            command
                .arg("--bash-launch-v1")
                .current_dir(&spawn_context.working_dir)
                .env_clear();
            command
        }
        (BashSpawnMode::Direct | BashSpawnMode::ExplicitTarget(_), None) => {
            let mut command = Command::new("bash");
            command
                .arg("-c")
                .arg(cmd)
                .current_dir(&spawn_context.working_dir);
            command
        }
        (BashSpawnMode::ExploreReadOnly, _) => {
            let sandbox_command = ExploreSandboxLauncher::command(cmd, &spawn_context.working_dir)?;
            sandbox_scratch_dir = Some(sandbox_command.scratch_dir);
            Command::from(sandbox_command.command)
        }
    };
    command
        .stdin(if use_gate {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !use_gate {
        command.env(BASH_LAUNCH_UUID_ENV_VAR, &launch_uuid);
    }

    command.kill_on_drop(true);

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

    let started_at = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            if let Some(dir) = &sandbox_scratch_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(SpawnChildError::NotSpawned(format!(
                "failed to spawn bash child: {e}"
            )));
        }
    };

    let Some((pid, process)) = child
        .id()
        .and_then(|pid| probe(pid).map(|identity| (pid, identity)))
    else {
        return Err(SpawnChildError::Unregistered(Box::new(UnregisteredChild {
            launch_gate: payload.map(|(_, payload)| LaunchGate {
                writer: child.stdin.take().expect("gated launcher stdin is piped"),
                payload,
            }),
            child,
            sandbox_scratch_dir,
            started_at,
        })));
    };
    let launch_identity = BashLaunchIdentity {
        process,
        launch_uuid,
    };
    // pgid == pid because we made the child a process group leader.
    let process_group_id = i32::try_from(pid).unwrap_or(0);

    let handle = Handle::new_live_for_actor_with_owner_and_launch_identity(
        ctx.work_scope.clone(),
        handle_id,
        launch_identity,
        ctx.resource_access.conversation_id().to_string(),
        ctx.resource_access.authority(),
        cmd.to_string(),
        label,
        spawn_context.working_dir.clone(),
        process_group_id,
        pid,
        ring_bytes_cap,
    );
    Ok((
        handle,
        SpawnedProcessGroup::new(
            child,
            process_group_id,
            sandbox_scratch_dir,
            started_at,
            payload.map(|(_, payload)| payload),
        ),
    ))
}

struct WaiterContext {
    owner_scope: ResourceScopeKey,
    lifecycle_sink: Option<crate::bash::registry::BashLifecycleSink>,
    terminal_effect: BashTerminalEffect,
    sandbox_scratch_dir: Option<std::path::PathBuf>,
    progress_reporter: Option<Arc<LiveBashProgressReporter>>,
}

fn start_io_tasks(
    handle: &Arc<Handle>,
    owner_scope: ResourceScopeKey,
    mut child: tokio::process::Child,
    lifecycle_sink: Option<crate::bash::registry::BashLifecycleSink>,
    terminal_effect: BashTerminalEffect,
    sandbox_scratch_dir: Option<std::path::PathBuf>,
    progress_reporter: Option<Arc<LiveBashProgressReporter>>,
) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    if let Some(reporter) = progress_reporter.clone() {
        let progress_handle = handle.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(LIVE_PROGRESS_MIN_INTERVAL);
            interval.tick().await;
            while reporter.is_active() {
                interval.tick().await;
                reporter.maybe_emit_handle(&progress_handle, false).await;
            }
        });
    }

    // Spawn the readers and capture their JoinHandles so the waiter
    // can join them between `child.wait()` and `transition_to_terminal`.
    // Without this synchronisation, the waiter races readers: the child
    // exits, the waiter resolves wait() and tombstones the handle, and
    // the readers' next read of the (already-buffered) final chunk hits
    // a non-Live state and silently drops those bytes.
    let stdout_join = stdout.map(|s| {
        let h = handle.clone();
        let reporter = progress_reporter.clone();
        tokio::spawn(async move {
            read_pipe_to_ring(s, h, reporter).await;
        })
    });
    let stderr_join = stderr.map(|s| {
        let h = handle.clone();
        let reporter = progress_reporter.clone();
        tokio::spawn(async move {
            read_pipe_to_ring(s, h, reporter).await;
        })
    });

    // Waiter task: call wait(), drain readers, then demote the handle.
    let h = handle.clone();
    tokio::spawn(async move {
        run_waiter(
            h,
            async move { child.wait().await },
            stdout_join,
            stderr_join,
            WaiterContext {
                owner_scope,
                lifecycle_sink,
                terminal_effect,
                sandbox_scratch_dir,
                progress_reporter,
            },
        )
        .await;
    });
}

async fn read_pipe_to_ring<R>(
    mut pipe: R,
    handle: Arc<Handle>,
    progress_reporter: Option<Arc<LiveBashProgressReporter>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 4096];
    let idle_flush = Duration::from_secs(super::ring::PARTIAL_IDLE_FLUSH_SECONDS);
    loop {
        // Race the pipe read against an idle timer. When the process emits a
        // fragment without a trailing newline and then goes quiet, the timer
        // wins and we flush the held partial as an ordinary line so `since`/
        // `tail` reads (which return complete lines only) can observe it.
        // This reuses the existing reader task — no per-handle timer thread.
        let read_result = tokio::select! {
            r = pipe.read(&mut buf) => Some(r),
            () = tokio::time::sleep(idle_flush) => None,
        };
        let Some(read_result) = read_result else {
            // Idle timeout: flush a held partial, then keep reading.
            let state = handle.state().await;
            if let HandleState::Live(live) = state.as_ref() {
                let mut ring = live.ring.lock().await;
                if ring.has_partial() {
                    ring.flush_partial();
                    if let Some(reporter) = &progress_reporter {
                        reporter.maybe_emit(&handle, &ring, true);
                    }
                }
            }

            continue;
        };
        match read_result {
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
                    if let Some(reporter) = &progress_reporter {
                        reporter.maybe_emit(&handle, &ring, false);
                    }
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
        if let Some(reporter) = &progress_reporter {
            reporter.maybe_emit(&handle, &ring, true);
        }
    }
}

/// Bound on how long the waiter waits for the stdout/stderr reader
/// tasks to drain after `child.wait()` resolves. Pipes EOF promptly
/// when the child exits, but a grandchild that inherited the stdout fd
/// and still holds it open would block the reader indefinitely; this
/// timeout protects the waiter from that pathological case at the cost
/// of dropping a few bytes in extreme scenarios.
const READER_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);
const LIVE_PROGRESS_MAX_LINES: usize = 40;
const LIVE_PROGRESS_MAX_BYTES: usize = 24 * 1024;
const LIVE_PROGRESS_MAX_PARTIAL_BYTES: usize = 4 * 1024;
const LIVE_PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(120);
const LIVE_PROGRESS_REANNOUNCE_INTERVAL: Duration = Duration::from_secs(2);

fn suffix_within_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text.get(start..).unwrap_or_default().to_string()
}

struct LiveBashProgressLease(Arc<LiveBashProgressReporter>);

impl Drop for LiveBashProgressLease {
    fn drop(&mut self) {
        self.0.deactivate();
    }
}

struct LiveBashProgressPoller {
    reporter: Arc<LiveBashProgressReporter>,
    task: tokio::task::JoinHandle<()>,
}

impl LiveBashProgressPoller {
    fn spawn(handle: Arc<Handle>, reporter: Arc<LiveBashProgressReporter>) -> Self {
        let task_reporter = reporter.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(LIVE_PROGRESS_MIN_INTERVAL);
            interval.tick().await;
            while task_reporter.is_active() {
                interval.tick().await;
                if handle.state().await.is_terminal() {
                    break;
                }
                task_reporter.maybe_emit_handle(&handle, false).await;
            }
        });
        Self { reporter, task }
    }
}

impl Drop for LiveBashProgressPoller {
    fn drop(&mut self) {
        self.reporter.deactivate();
        self.task.abort();
    }
}

struct LiveBashProgressReporter {
    sink: Arc<dyn crate::BashProgressSink>,
    last_emitted_output_bytes: std::sync::atomic::AtomicU64,
    last_emitted_at_ms: std::sync::atomic::AtomicU64,
    active: std::sync::atomic::AtomicBool,
}

impl LiveBashProgressReporter {
    fn new(sink: Arc<dyn crate::BashProgressSink>) -> Self {
        Self {
            sink,
            last_emitted_output_bytes: std::sync::atomic::AtomicU64::new(0),
            last_emitted_at_ms: std::sync::atomic::AtomicU64::new(0),
            active: std::sync::atomic::AtomicBool::new(true),
        }
    }

    fn deactivate(&self) {
        self.active
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn is_active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::Acquire)
    }

    async fn maybe_emit_handle(&self, handle: &Handle, force: bool) {
        let state = handle.state().await;
        if let HandleState::Live(live) = state.as_ref() {
            let ring = live.ring.lock().await;
            self.maybe_emit(handle, &ring, force);
        }
    }

    fn maybe_emit(&self, handle: &Handle, ring: &super::ring::RingBuffer, force: bool) {
        if !self.active.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let output_bytes = ring.output_bytes();
        let prior_output_bytes = self
            .last_emitted_output_bytes
            .load(std::sync::atomic::Ordering::Acquire);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let prior_at = self
            .last_emitted_at_ms
            .load(std::sync::atomic::Ordering::Acquire);
        let output_unchanged = output_bytes == prior_output_bytes;
        if !force
            && output_unchanged
            && now_ms.saturating_sub(prior_at)
                < u64::try_from(LIVE_PROGRESS_REANNOUNCE_INTERVAL.as_millis()).unwrap_or(u64::MAX)
        {
            return;
        }
        if !force
            && prior_at != 0
            && now_ms.saturating_sub(prior_at)
                < u64::try_from(LIVE_PROGRESS_MIN_INTERVAL.as_millis()).unwrap_or(u64::MAX)
        {
            return;
        }

        let view = ring.tail(LIVE_PROGRESS_MAX_LINES);
        let end_offset = view.end_offset;
        let raw_partial = ring.partial_str();
        let partial_was_truncated = raw_partial
            .as_ref()
            .is_some_and(|text| text.len() > LIVE_PROGRESS_MAX_PARTIAL_BYTES);
        let partial =
            raw_partial.map(|text| suffix_within_bytes(&text, LIVE_PROGRESS_MAX_PARTIAL_BYTES));

        let mut remaining_bytes =
            LIVE_PROGRESS_MAX_BYTES.saturating_sub(partial.as_ref().map_or(0, String::len));
        let mut lines: Vec<BashProgressLine> = Vec::new();
        let mut projected_lines_truncated = false;
        for line in view.lines.iter().rev() {
            if remaining_bytes == 0 {
                projected_lines_truncated = true;
                break;
            }
            let text = String::from_utf8_lossy(&line.bytes);
            let bounded = suffix_within_bytes(&text, remaining_bytes);
            projected_lines_truncated |= bounded.len() < text.len();
            remaining_bytes = remaining_bytes.saturating_sub(bounded.len());
            lines.push(BashProgressLine {
                offset: line.offset,
                text: bounded,
            });
        }
        lines.reverse();
        let line_limit_truncated = ring.len() > view.lines.len();
        let projection_truncated =
            partial_was_truncated || projected_lines_truncated || line_limit_truncated;
        let start_offset = lines.first().map_or(end_offset, |line| line.offset);
        self.sink.emit(BashToolProgress {
            handle: handle.handle_id.to_string(),
            start_offset,
            end_offset,
            truncated_before: view.truncated_before || projection_truncated,
            lines,
            partial,
        });
        self.last_emitted_output_bytes
            .store(output_bytes, std::sync::atomic::Ordering::Release);
        self.last_emitted_at_ms
            .store(now_ms, std::sync::atomic::Ordering::Release);
    }
}

async fn run_waiter<F>(
    handle: Arc<Handle>,
    child_exit: F,
    stdout_join: Option<tokio::task::JoinHandle<()>>,
    stderr_join: Option<tokio::task::JoinHandle<()>>,
    context: WaiterContext,
) where
    F: std::future::Future<Output = std::io::Result<std::process::ExitStatus>>,
{
    let panic_guard = ExitWatchPanicGuard::new(handle.clone());
    let started_at = Instant::now();
    let exit_status = child_exit.await;

    let cause = match exit_status {
        Ok(status) => exit_status_to_cause(status),
        Err(e) => {
            tracing::warn!(?e, %handle.handle_id, "child wait() failed; recording as exited(None)");
            FinalCause::Exited { exit_code: None }
        }
    };

    let finished_at = SystemTime::now();

    drain_then_transition_to_terminal(
        &handle,
        stdout_join,
        stderr_join,
        cause,
        started_at,
        finished_at,
        &context,
    )
    .await;

    if let Some(dir) = context.sandbox_scratch_dir {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::debug!(path = %dir.display(), error = %e, "failed to remove explore bash scratch directory");
        }
    }
    panic_guard.disarm();
}

async fn drain_then_transition_to_terminal(
    handle: &Arc<Handle>,
    stdout_join: Option<tokio::task::JoinHandle<()>>,
    stderr_join: Option<tokio::task::JoinHandle<()>>,
    cause: FinalCause,
    started_at: Instant,
    finished_at: SystemTime,
    context: &WaiterContext,
) {
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
    if let Some(reporter) = &context.progress_reporter {
        reporter.maybe_emit_handle(handle, true).await;
        reporter.deactivate();
    }

    let elapsed = started_at.elapsed();
    if handle
        .transition_to_terminal(cause.clone(), elapsed, finished_at, TOMBSTONE_TAIL_LINES)
        .await
    {
        handle.publish_exit(ExitState::Exited);
        // Terminal edge: the handle just demoted to tombstoned. Emit the
        // work-scope state-transition signal (REQ-WSUI-007). Guarded by the
        // `transition_to_terminal` return so a redundant late transition does
        // not double-emit. Best-effort: a closed sink is logged by the bridge.
        if let Some(sink) = &context.lifecycle_sink {
            if let Err(e) = sink.send(crate::bash::registry::BashLifecycleEvent {
                owner: context.owner_scope.clone(),
                handle_id: Some(handle.handle_id.clone()),
                phase: crate::bash::registry::BashLifecyclePhase::Terminal,
                cause: Some(cause),
                terminal_effect: Some(context.terminal_effect),
            }) {
                tracing::debug!(
                    owner = %context.owner_scope,
                    handle_id = %handle.handle_id,
                    terminal_effect = ?context.terminal_effect,
                    error = %e,
                    "dropping bash terminal lifecycle event — sink closed"
                );
            }
        }
    }
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
        if (129..192).contains(&code) {
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
            background_run_response(&handle, started.elapsed(), &read_args, cmd).await
        }
        Ok(()) = exit_rx.changed() => {
            // Process exited (or waiter panicked). Either way, build the
            // appropriate response from current state.
            terminal_or_panic_response(&handle, &read_args, true, false, Some(cmd)).await
        }
        () = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            background_run_response(&handle, Duration::from_secs(wait_seconds), &read_args, cmd).await
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
    // response immediately. Avoids parking on a receiver cloned after the
    // terminal watch update already published (ADR-002 / `wait` rules in
    // `specs/bash/bash.allium`).
    if handle.state().await.is_terminal() {
        return shape_handle_response(&handle, &read_args, ResponseKind::Wait, None).await;
    }

    let progress_reporter = ctx
        .bash_progress_sink()
        .map(|sink| Arc::new(LiveBashProgressReporter::new(sink)));
    if let Some(reporter) = &progress_reporter {
        reporter.maybe_emit_handle(&handle, true).await;
    }
    let _progress_poller = progress_reporter
        .as_ref()
        .map(|reporter| LiveBashProgressPoller::spawn(handle.clone(), reporter.clone()));

    let mut exit_rx = handle.exit_observer();
    let started = Instant::now();
    let response = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            still_running_response(&handle, started.elapsed(), &read_args, &handle.cmd).await
        }
        Ok(()) = exit_rx.changed() => {
            terminal_or_panic_response(&handle, &read_args, false, false, None).await
        }
        () = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            still_running_response(&handle, Duration::from_secs(wait_seconds), &read_args, &handle.cmd).await
        }
    };
    response
}

// ---------------------------------------------------------------------------
// Kill
// ---------------------------------------------------------------------------

/// Stops one exact live handle after the caller has established its controller scope.
///
/// # Errors
///
/// Returns [`BashError::HandleNotFound`] when the handle is missing, terminal, or owned by a
/// different resource scope.
pub async fn stop_exact_handle_for_scope(
    registry: &crate::BashHandleRegistry,
    owner: &phoenix_core::work_scope::ResourceScopeKey,
    handle_id: &str,
) -> Result<(), BashError> {
    registry
        .signal_exact_registered_handle(&HandleId::new(handle_id.to_string()), owner, libc::SIGTERM)
        .await
        .map_err(|error| BashError::SpawnFailed {
            error_message: error.to_string(),
        })?
        .then_some(())
        .ok_or_else(|| BashError::HandleNotFound {
            handle_id: handle_id.to_string(),
        })?;
    Ok(())
}

async fn run_kill(
    handle_id: &str,
    signal: KillSignal,
    kill_response_timeout: Duration,
    ctx: &ToolContext,
) -> ToolOutput {
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

    let progress_reporter = ctx
        .bash_progress_sink()
        .map(|sink| Arc::new(LiveBashProgressReporter::new(sink)));
    if let Some(reporter) = &progress_reporter {
        reporter.maybe_emit_handle(&handle, true).await;
    }
    let _progress_poller = progress_reporter
        .as_ref()
        .map(|reporter| LiveBashProgressPoller::spawn(handle.clone(), reporter.clone()));

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
        () = tokio::time::sleep(kill_response_timeout) => {
            // Mark kill_pending_kernel via the foundation helper.
            // The waiter task remains alive — a late exit will eventually
            // demote the handle to tombstoned.
            handle
                .mark_kill_pending_kernel(signal, SystemTime::now())
                .await;
            // Kill edge: the handle moved running -> kill_pending_kernel, a
            // state the inventory reflects (REQ-WSUI-007). Emit a non-
            // reconciling lifecycle phase now; the later true terminal
            // transition still emits from the waiter after final I/O drains.
            if let Some(registered) = ctx
                .bash_handle_registry()
                .get_by_id(&handle.handle_id)
                .await
            {
                ctx.bash_handle_registry().emit_lifecycle(
                    &registered.owner,
                    Some(handle.handle_id.clone()),
                    crate::bash::registry::BashLifecyclePhase::KillPendingKernel,
                    None,
                    None,
                );
            } else {
                tracing::debug!(
                    handle_id = %handle.handle_id,
                    "skipping kill-pending lifecycle event — handle owner unavailable"
                );
            }
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
    let registered = ctx
        .bash_handle_registry()
        .get_by_id(&HandleId::new(handle_id.to_string()))
        .await
        .ok_or_else(|| BashError::HandleNotFound {
            handle_id: handle_id.to_string(),
        })?;
    let handle = registered.handle;
    if handle.controller_scope != ctx.work_scope
        || !ctx
            .resource_access
            .can_control(&handle.creator_conversation_id, handle.authority)
    {
        return Err(BashError::HandleNotFound {
            handle_id: handle_id.to_string(),
        });
    }
    Ok(handle)
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
        ResponseKind::Peek | ResponseKind::Wait | ResponseKind::Kill { .. } => None,
    };

    let typed: BashResponse = match state.as_ref() {
        HandleState::Live(live) => {
            let kill_attempt = handle.kill_attempt().await;
            let is_kill_pending_kernel = kill_attempt.is_some();
            let ring = live.ring.lock().await;
            let read = read_window_from_ring(&ring, read_args);
            let window = window_to_typed(&read.view, read.partial);
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
                        display: Some(display),
                        signal_sent: Some(signal_sent_top.unwrap_or_else(|| "TERM".into())),
                        waited_ms: None,
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
                        display: Some(display),
                        signal_sent: None,
                        waited_ms: None,
                    })
                }
                ResponseKind::Wait if is_kill_pending_kernel => {
                    BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
                        handle: handle.handle_id.to_string(),
                        cmd,
                        label: label.clone(),
                        window,
                        kill_signal_sent: kill_signal_str.unwrap_or_else(|| "TERM".into()),
                        kill_attempted_at: kill_attempted_str
                            .unwrap_or_else(|| format_systime(SystemTime::now())),
                        display: Some(display),
                        signal_sent: None,
                        waited_ms: None,
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
            let window = window_to_typed(&view, None);
            let display_field = match kind {
                ResponseKind::Kill { .. } => Some(display),
                ResponseKind::Peek | ResponseKind::Wait => None,
            };
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
                display: display_field,
                signal_sent: signal_sent_top,
            })
        }
    };

    let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::success(serialized).with_display(value)
}

async fn background_run_response(
    handle: &Arc<Handle>,
    elapsed: Duration,
    read_args: &ReadArgs,
    cmd: &str,
) -> ToolOutput {
    still_running_response(handle, elapsed, read_args, cmd).await
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
        let read = read_window_from_ring(&ring, read_args);
        window_to_typed(&read.view, read.partial)
    } else {
        BashRingWindow {
            start_offset: 0,
            end_offset: 0,
            truncated_before: false,
            lines: vec![],
            partial: None,
        }
    };

    let kill_signal_str = kill_attempt
        .as_ref()
        .map(|a| a.signal_sent.as_str().to_string());
    let kill_attempted_str = kill_attempt
        .as_ref()
        .map(|a| format_systime(a.attempted_at));

    // If a kill is in flight, surface as kill_pending_kernel (REQ-BASH-003).
    // This is the passive-wait branch: no `display` label (only kill / peek /
    // wait shape one), no `signal_sent` echo (this caller did not issue the
    // kill), and `waited_ms` carries the elapsed wait window. The
    // `BashKillPendingKernelPayload` fields are `Option`s precisely so this
    // shape is constructible directly — no post-serialization scrubbing.
    let typed = if kill_attempt.is_some() {
        BashResponse::KillPendingKernel(BashKillPendingKernelPayload {
            handle: handle.handle_id.to_string(),
            cmd: cmd.to_string(),
            label: label.clone(),
            window,
            kill_signal_sent: kill_signal_str.unwrap_or_else(|| "TERM".into()),
            kill_attempted_at: kill_attempted_str
                .unwrap_or_else(|| format_systime(SystemTime::now())),
            display: None,
            signal_sent: None,
            waited_ms: Some(waited_ms),
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

    let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
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
            let window = window_to_typed(&view, None);

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
                    display: None,
                    signal_sent: None,
                })
            };

            let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
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

/// A read window over the live ring or a tombstone, plus the live trailing
/// partial line. `partial` is `Some` only for live reads that have an
/// un-newlined trailing fragment; tombstone reads always carry `None`
/// (the partial was flushed to a line on EOF). Keeping `partial` separate
/// from `WindowView.lines` makes "complete lines" and "in-progress line"
/// structurally distinct — the LLM peek may ignore the partial; the
/// inspector renders it.
pub(crate) struct RingRead {
    pub(crate) view: WindowView,
    pub(crate) partial: Option<String>,
}

pub(crate) fn read_window_from_ring(ring: &super::ring::RingBuffer, args: &ReadArgs) -> RingRead {
    let view = if let Some(since) = args.since {
        ring.since(since)
    } else {
        let n = args.lines.unwrap_or(DEFAULT_PEEK_LINES);
        ring.tail(n)
    };
    RingRead {
        view,
        partial: ring.partial_str(),
    }
}

/// Tombstone reads carry no live partial — the trailing partial was flushed
/// to a complete line on EOF before the tombstone was written, so it already
/// lives in `final_tail`. Wrapping the `WindowView` in [`RingRead`] with
/// `partial: None` keeps the wire shaping uniform with the live path.
pub(crate) fn read_window_from_tombstone(
    tomb: &super::handle::Tombstone,
    args: &ReadArgs,
) -> WindowView {
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

pub(crate) fn window_to_typed(view: &WindowView, partial: Option<String>) -> BashRingWindow {
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
        partial,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::handle::{Handle, HandleId};
    use crate::bash::registry::{BashHandleRegistry, BashLifecyclePhase};
    use crate::bash::ring::{PARTIAL_IDLE_FLUSH_SECONDS, RING_BUFFER_BYTES};
    use phoenix_core::process_identity::observe_process_identity;
    use phoenix_core::work_scope::ResourceScopeKey;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, ReadBuf};

    fn scope(id: &str) -> ResourceScopeKey {
        ResourceScopeKey::Work(phoenix_core::work_scope::WorkScopeId::parse(id).unwrap())
    }

    fn spawn_context_for(ctx: &ToolContext) -> SpawnContext {
        SpawnContext {
            working_dir: ctx.working_dir().to_path_buf(),
            lifecycle_scope: ctx.work_scope.clone(),
            terminal_effect: BashTerminalEffect::InventoryAndBranchReconcile,
        }
    }

    #[cfg(unix)]
    fn injected_unregistered_child(
        ctx: &ToolContext,
        working_dir: &std::path::Path,
        cmd: &str,
        probe: impl FnOnce(u32) -> Option<phoenix_core::process_identity::ProcessIdentity>,
    ) -> UnregisteredChild {
        let mut context = spawn_context_for(ctx);
        context.working_dir = working_dir.to_path_buf();
        match spawn_child_with_identity_probe(
            cmd,
            None,
            ctx,
            &context,
            HandleId::new("b-injected"),
            RING_BUFFER_BYTES,
            &BashSpawnMode::Direct,
            probe,
        ) {
            Err(SpawnChildError::Unregistered(child)) => *child,
            Err(SpawnChildError::NotSpawned(error)) => panic!("OS spawn failed: {error}"),
            Ok(_) => panic!("an unverified child must not get a handle"),
        }
    }

    #[cfg(unix)]
    fn response_json(output: &ToolOutput) -> Value {
        serde_json::from_str(output.output()).expect("response JSON")
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn settled_native_run_response(output: &ToolOutput, ctx: &ToolContext) -> Value {
        let response = response_json(output);
        if response["status"] == "exited" {
            return response;
        }
        assert_eq!(response["status"], "still_running");
        let handle_id = response["handle"].as_str().expect("live command handle");
        let handle = lookup_handle(ctx, handle_id)
            .await
            .expect("owned native command handle");
        let mut exit_rx = handle.exit_observer();
        tokio::time::timeout(Duration::from_secs(10), exit_rx.wait_for(Option::is_some))
            .await
            .expect("native command lifecycle did not settle")
            .expect("native command lifecycle observer closed");
        response_json(
            &terminal_or_panic_response(&handle, &ReadArgs::default(), true, false, None).await,
        )
    }

    #[cfg(unix)]
    fn native_launcher_available() -> bool {
        use std::os::unix::fs::PermissionsExt;

        let Some(executable) = std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE") else {
            assert!(
                std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_REQUIRED").is_none(),
                "PHOENIX_BASH_LAUNCH_TEST_EXE is required but unset"
            );
            eprintln!(
                "explicitly skipping native launcher test: PHOENIX_BASH_LAUNCH_TEST_EXE is unset"
            );
            return false;
        };
        let executable = std::path::PathBuf::from(executable);
        let metadata = std::fs::metadata(&executable)
            .unwrap_or_else(|error| panic!("PHOENIX_BASH_LAUNCH_TEST_EXE is invalid: {error}"));
        assert!(metadata.is_file(), "native launcher path is not a file");
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "native launcher is not executable"
        );
        true
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_raw_launcher_rejects_empty_truncated_and_oversized_eof_payloads() {
        if !native_launcher_available() {
            return;
        }
        let executable =
            std::path::PathBuf::from(std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE").unwrap());
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("empty", Vec::new()),
            ("truncated", b"PHXB\\x01\\x00\\x00".to_vec()),
            (
                "oversized",
                vec![b'x'; super::super::launch_protocol::MAX_PAYLOAD + 1],
            ),
        ] {
            let marker = dir.path().join(format!("{name}.marker"));
            let mut child = tokio::process::Command::new(&executable)
                .arg("--bash-launch-v1")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            if !bytes.is_empty() {
                child.stdin.take().unwrap().write_all(&bytes).await.unwrap();
            }
            drop(child.stdin.take());
            let status = child.wait().await.unwrap();
            assert_eq!(status.code(), Some(126), "{name} payload must be refused");
            assert!(!marker.exists());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launch_gate_release_reports_closed_child_stdin() {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn helper child");
        let writer = child.stdin.take().expect("piped child stdin");
        let status = child.wait().await.expect("wait for child exit");
        assert!(status.success());

        let error = LaunchGate {
            writer,
            payload: b"launch payload".to_vec(),
        }
        .release()
        .await
        .expect_err("writing to a completed child must fail");
        assert!(error.contains("failed to write bash launch payload"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_raw_launcher_waits_for_eof_and_exec_preserves_process_identity() {
        if !native_launcher_available() {
            return;
        }

        let executable =
            std::path::PathBuf::from(std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_EXE").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("release");
        let pid_file = dir.path().join("pid");
        let command = format!(
            "printf '%s' $$ > {}; while [ ! -e {} ]; do sleep 0.01; done; printf executed > {}",
            pid_file.display(),
            marker.display(),
            dir.path().join("executed").display()
        );
        let payload = super::super::launch_protocol::BashLaunchPayload {
            mode: super::super::launch_protocol::BashLaunchMode::Direct,
            command: command.into(),
            working_dir: dir.path().to_path_buf(),
            environment: std::env::vars_os().collect(),
        }
        .encode()
        .unwrap();
        let mut child = tokio::process::Command::new(&executable)
            .env_clear()
            .arg("--bash-launch-v1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().expect("launcher child pid");
        let before = observe_process_identity(pid).expect("launcher identity");
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&payload).await.unwrap();
        // test-timing-allow: withhold EOF for a bounded interval to prove a complete payload does not execute during the held-gate window
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !pid_file.exists(),
            "complete payload must not execute before EOF"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "launcher must remain alive behind the gate"
        );
        drop(stdin);
        let recorded_pid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = contents.parse::<u32>() {
                        break pid;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("command did not write a parsable PID after EOF");
        assert_eq!(recorded_pid, pid, "exec must preserve PID");
        assert_eq!(
            observe_process_identity(pid),
            Ok(before),
            "exec must preserve birth identity"
        );
        std::fs::write(marker, "").unwrap();
        let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .expect("launcher command did not exit")
            .unwrap();
        assert!(status.success());
        assert!(dir.path().join("executed").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_registered_gate_release_failure_preserves_handle_and_marks_uncertainty() {
        if !native_launcher_available() {
            return;
        }

        let scope = scope("native-registered-gate-release-failure");
        let ctx = test_context(&scope);
        let registry = ctx.bash_handle_registry().clone();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("executed");
        let command = format!("printf executed > {}", marker.display());
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let mut reservation = registry.reserve_spawn(&scope).await.unwrap();
        let (handle, spawned) = spawn_child_with_identity_probe_protocol(
            &command,
            None,
            &ctx,
            &context,
            reservation.handle_id().clone(),
            RING_BUFFER_BYTES,
            &BashSpawnMode::Direct,
            SpawnProtocol::Gated,
            |pid| observe_process_identity(pid).ok(),
        )
        .expect("native gated child should launch with verified identity");
        reservation.mark_launched();
        let inserted = registry
            .commit_spawn_gated(&mut reservation, handle.clone())
            .await
            .expect("register gated handle");
        let (mut child, mut launch_gate, scratch) = spawned.into_waiter_parts();
        child.start_kill().expect("terminate direct launcher child");
        child.wait().await.expect("reap direct launcher child");
        // The native child is terminal and reaped. Inject a transport whose write
        // direction is explicitly shut down; child exit alone does not prove all
        // pipe-reader copies are gone during concurrent process creation.
        let (closed_transport, _peer) =
            std::os::unix::net::UnixStream::pair().expect("owned local fault-transport pair");
        closed_transport
            .shutdown(std::net::Shutdown::Write)
            .expect("disable fault-transport writes");
        let closed_fd: std::os::fd::OwnedFd = closed_transport.into();
        let closed_writer =
            tokio::process::ChildStdin::from_std(std::process::ChildStdin::from(closed_fd))
                .expect("register shut-down fault transport");
        launch_gate
            .as_mut()
            .expect("native gated launch has a gate")
            .writer = closed_writer;
        start_io_tasks(
            &inserted,
            scope.clone(),
            child,
            registry.lifecycle_sink(),
            BashTerminalEffect::InventoryAndBranchReconcile,
            scratch,
            None,
        );
        let error = release_registered_gate(
            launch_gate.expect("native gated launch has a gate"),
            reservation,
            &registry,
            handle.clone(),
        )
        .await
        .expect_err("closed child stdin must produce uncertain release error");
        let output = error.into_tool_output();
        let response = response_json(&output);
        assert_eq!(response["error"], "launch_release_unknown");
        assert_eq!(response["handle"], handle.handle_id.as_str());
        assert!(response["error_message"]
            .as_str()
            .unwrap()
            .contains("Do not retry automatically"));
        assert_eq!(
            registry
                .get_by_id(&handle.handle_id)
                .await
                .unwrap()
                .handle
                .handle_id,
            handle.handle_id
        );
        let fence = tokio::time::timeout(Duration::from_secs(3), registry.begin_teardown(&scope))
            .await
            .expect("registered release failure must settle pending custody")
            .expect("owner teardown fence");
        assert_eq!(
            fence.scope_uncertainty().await,
            Some(crate::bash::registry::BashScopeUncertainty::UnregisteredLaunch)
        );
        assert!(!marker.exists(), "the gated command must not execute");
        assert!(matches!(
            registry.reserve_spawn(&scope).await,
            Err(BashHandleError::SpawnFenced)
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn native_gated_delayed_probe_runs_fast_command_with_direct_process_contract() {
        if !native_launcher_available() {
            return;
        }

        let ctx = test_context(&scope("native-gated-delayed-probe"));
        let dir = tempfile::tempdir().unwrap();
        let context = {
            let mut context = spawn_context_for(&ctx);
            context.working_dir = dir.path().to_path_buf();
            context
        };
        let command = "pwd > cwd.txt; printf '%s' \"$PATH\" > path.txt; test -c /dev/fd/0 && printf char > stdin.txt; printf native-fast";
        let probe_record = std::sync::Mutex::new(None);
        let output = run_run_with_identity_probe_mode(
            command,
            None,
            2,
            ReadArgs::default(),
            &ctx,
            &context,
            BashSpawnMode::Direct,
            SpawnProtocol::Gated,
            |pid| {
                // test-timing-allow: delay identity observation to verify the native fast command remains correct across delayed process-identity probing
                std::thread::sleep(Duration::from_millis(75));
                let (observed, record) = rca_probe_record(pid);
                *probe_record.lock().unwrap() = Some((pid, record));
                observed.ok()
            },
        )
        .await;
        let response = settled_native_run_response(&output, &ctx).await;
        let (pid, probe) = probe_record
            .lock()
            .unwrap()
            .take()
            .expect("native probe recorded");
        eprintln!(
            "RCA_SAMPLE phase=native_gated_delayed_probe platform={} pid={} probe={} status={} exit_code={}",
            std::env::consts::OS,
            pid,
            probe,
            response["status"].as_str().unwrap_or("unknown"),
            response["exit_code"].as_i64().map_or_else(|| "none".to_string(), |code| code.to_string())
        );
        assert_eq!(response["status"], "exited");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("cwd.txt")).unwrap(),
            format!("{}\n", dir.path().canonicalize().unwrap().display())
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("path.txt")).unwrap(),
            std::env::var("PATH").unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("stdin.txt")).unwrap(),
            "char"
        );
        assert_eq!(response["lines"][0]["bytes"], "native-fast");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_gated_identity_denial_never_executes_payload() {
        if !native_launcher_available() {
            return;
        }
        let ctx = test_context(&scope("native-gated-identity-denial"));
        let dir = tempfile::tempdir().unwrap();
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let output = run_run_with_identity_probe_mode(
            "printf executed > side-effect.txt",
            None,
            2,
            ReadArgs::default(),
            &ctx,
            &context,
            BashSpawnMode::Direct,
            SpawnProtocol::Gated,
            |_| None,
        )
        .await;
        let response = response_json(&output);
        assert_eq!(response["launch_error"], "identity_unavailable");
        assert!(!dir.path().join("side-effect.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_gated_foreign_registry_denial_settles_without_executing_command() {
        if !native_launcher_available() {
            return;
        }

        let scope = scope("native-foreign-registry-denial");
        let ctx = test_context(&scope);
        let foreign = BashHandleRegistry::new();
        let dir = tempfile::tempdir().unwrap();
        let command = "printf executed > side-effect.txt";
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&scope)
            .await
            .unwrap();
        let (handle, spawned) = spawn_child_with_identity_probe_protocol(
            command,
            None,
            &ctx,
            &context,
            reservation.handle_id().clone(),
            RING_BUFFER_BYTES,
            &BashSpawnMode::Direct,
            SpawnProtocol::Gated,
            |pid| observe_process_identity(pid).ok(),
        )
        .expect("native gated child should launch with verified identity");
        let mut reservation = reservation;
        assert!(matches!(
            foreign.commit_spawn_gated(&mut reservation, handle).await,
            Err(BashHandleError::SpawnFenced)
        ));
        reservation.mark_unregistered();
        let (child, launch_gate, sandbox_scratch_dir) = spawned.into_waiter_parts();
        let output = settle_unregistered_run(
            UnregisteredChild {
                child,
                launch_gate,
                sandbox_scratch_dir,
                started_at: Instant::now(),
            },
            reservation,
            command,
            None,
            RING_BUFFER_BYTES,
            &ReadArgs::default(),
            BashLaunchError::RegistrationFailed,
        )
        .await;
        let response = response_json(&output);
        assert_eq!(response["launch_error"], "registration_failed");
        assert!(matches!(
            response["status"].as_str(),
            Some("unregistered_killed" | "unregistered_exited")
        ));
        assert!(!dir.path().join("side-effect.txt").exists());
        let fence = tokio::time::timeout(
            Duration::from_secs(3),
            ctx.bash_handle_registry().begin_teardown(&scope),
        )
        .await
        .expect("denied registration must settle pending custody")
        .expect("owner teardown fence");
        assert_eq!(
            fence.scope_uncertainty().await,
            Some(crate::bash::registry::BashScopeUncertainty::UnregisteredLaunch)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn require_native_explore_test_support() -> bool {
        if !ExploreSandboxLauncher::supported() {
            assert!(
                std::env::var_os("PHOENIX_BASH_LAUNCH_TEST_REQUIRED").is_none(),
                "required native Explore launch test is unsupported on this host"
            );
            eprintln!(
                "explicitly skipping native Explore launch test: host sandbox is unsupported"
            );
            return false;
        }
        native_launcher_available()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn native_gated_explore_rejects_repository_write_after_release() {
        if !require_native_explore_test_support() {
            return;
        }

        let ctx = test_context(&scope("native-gated-explore-write-denied"));
        let dir = tempfile::tempdir().unwrap();
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let output = run_run_with_identity_probe_mode(
            "printf explore-ok; printf denied > forbidden.txt; exit 7",
            None,
            2,
            ReadArgs::default(),
            &ctx,
            &context,
            BashSpawnMode::ExploreReadOnly,
            SpawnProtocol::Gated,
            |pid| {
                // test-timing-allow: delay identity observation to verify the native explore command still enforces repository-write denial
                std::thread::sleep(Duration::from_millis(75));
                observe_process_identity(pid).ok()
            },
        )
        .await;
        let response = settled_native_run_response(&output, &ctx).await;
        assert_eq!(response["status"], "exited");
        assert_eq!(response["exit_code"], 7);
        let combined_output = response["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|line| line["bytes"].as_str())
            .collect::<String>();
        assert!(
            combined_output.contains("explore-ok"),
            "combined stdout/stderr should include command output: {combined_output:?}"
        );
        assert!(!dir.path().join("forbidden.txt").exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn native_gated_explore_identity_denial_never_executes_payload() {
        if !require_native_explore_test_support() {
            return;
        }
        let ctx = test_context(&scope("native-gated-explore-identity-denial"));
        let dir = tempfile::tempdir().unwrap();
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let output = run_run_with_identity_probe_mode(
            "printf explore-preidentity-executed",
            None,
            2,
            ReadArgs::default(),
            &ctx,
            &context,
            BashSpawnMode::ExploreReadOnly,
            SpawnProtocol::Gated,
            |_| None,
        )
        .await;
        let response = response_json(&output);
        assert_eq!(response["launch_error"], "identity_unavailable");
        // Printing is sandbox-permitted, unlike the denied repository write in
        // the success case; the sandbox cannot mask accidental early execution.
        assert!(response["lines"]
            .as_array()
            .expect("captured output window")
            .iter()
            .all(|line| !line["bytes"]
                .as_str()
                .unwrap_or_default()
                .contains("explore-preidentity-executed")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn abort_during_registration_keeps_launch_owned_and_never_group_kills() {
        let ctx = test_context(&scope("cancel-registration"));
        let registry = ctx.bash_handle_registry().clone();
        let table = registry.get_or_create(&ctx.work_scope).await;
        let held_lock = Arc::new(std::sync::Mutex::new(None));
        let lock_slot = held_lock.clone();
        let launched = Arc::new(tokio::sync::Notify::new());
        let launch_signal = launched.clone();
        let dir = tempfile::tempdir().unwrap();
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let command = "printf once >> count; while [ ! -e release ]; do sleep 0.01; done; printf survived > finished";
        let task_ctx = ctx.clone();
        let caller = tokio::spawn(async move {
            run_run_with_identity_probe(
                command,
                None,
                30,
                ReadArgs::default(),
                &task_ctx,
                &context,
                BashSpawnMode::Direct,
                |pid| {
                    let identity = current_process_identity(pid).expect("live child identity");
                    *lock_slot.lock().unwrap() = Some(table.try_write_owned().unwrap());
                    launch_signal.notify_one();
                    Some(identity)
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), launched.notified())
            .await
            .expect("launch did not reach the registration barrier");
        caller.abort();
        let _ = caller.await;
        drop(held_lock.lock().unwrap().take());
        tokio::time::timeout(Duration::from_secs(3), async {
            while registry.owner_handles(&ctx.work_scope).await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("caller cancellation lost registration owner");
        std::fs::write(dir.path().join("release"), "").unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !dir.path().join("finished").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("caller cancellation killed the launched process group");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("count")).unwrap(),
            "once"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unknown_launch_serialization_has_no_control_identity_or_fabricated_exit() {
        let ring = RingBuffer::new(1024);
        let read = read_window_from_ring(&ring, &ReadArgs::default());
        let output = unregistered_tool_output(
            BashUnregisteredRun {
                cmd: "may-have-side-effects".to_string(),
                label: None,
                launch_error: BashLaunchError::IdentityUnavailable,
                descendants: BashUnregisteredDescendants::Unverified,
                output_complete: false,
                duration_ms: 1,
                window: window_to_typed(&read.view, read.partial),
            },
            None,
        );
        let response = response_json(&output);
        assert_eq!(response["status"], "outcome_unknown");
        assert!(response["error_message"]
            .as_str()
            .unwrap()
            .contains("Do not automatically retry"));
        for key in [
            "handle",
            "pid",
            "pgid",
            "exit_code",
            "signal_number",
            "launch_identity",
        ] {
            assert!(
                response.get(key).is_none(),
                "unexpected control/terminal field {key}"
            );
        }
        assert!(matches!(
            serde_json::from_value::<BashResponse>(response).unwrap(),
            BashResponse::OutcomeUnknown(_)
        ));
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn unregistered_capture_stays_open_until_direct_child_terminal() {
        let ctx = test_context(&scope("capture-until-exit"));
        let dir = tempfile::tempdir().unwrap();
        let cmd = "printf ready > ready; read -r release; printf 'late-tail'; exit 4";
        let mut process = tokio::process::Command::new("bash")
            .args(["-c", cmd])
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut release = process.stdin.take().unwrap();
        let child = UnregisteredChild {
            child: process,
            launch_gate: None,
            sandbox_scratch_dir: None,
            started_at: Instant::now(),
        };
        // Keep the executor runnable: OS child progress must not auto-advance Tokio time.
        let watchdog = std::time::Instant::now();
        while !dir.path().join("ready").exists() {
            assert!(
                watchdog.elapsed() < Duration::from_secs(5),
                "child never ready"
            );
            tokio::task::yield_now().await;
        }
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&ctx.work_scope)
            .await
            .unwrap();
        let settlement = tokio::spawn(settle_unregistered_run(
            child,
            reservation,
            cmd,
            None,
            RING_BUFFER_BYTES,
            &ReadArgs::default(),
            BashLaunchError::IdentityUnavailable,
        ));
        tokio::task::yield_now().await;
        // Cross the reader-drain budget while the child is deliberately still live.
        let elapsed = READER_DRAIN_TIMEOUT + Duration::from_millis(1);
        assert!(elapsed < UNREGISTERED_SETTLEMENT_TIMEOUT);
        tokio::time::advance(elapsed).await;
        assert!(!settlement.is_finished());
        release.write_all(b"release\n").await.unwrap();
        let watchdog = std::time::Instant::now();
        while !settlement.is_finished() {
            assert!(
                watchdog.elapsed() < Duration::from_secs(5),
                "settlement stalled"
            );
            tokio::task::yield_now().await;
        }
        let response = response_json(&settlement.await.unwrap());
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 4);
        assert_eq!(response["lines"][0]["bytes"], "late-tail");
        assert_eq!(response["output_complete"], true);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    struct OwnedChild(std::process::Child);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            match self.0.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {}
                Err(error) => eprintln!("RCA child cleanup try_wait failed: {error}"),
            }
            if let Err(error) = self.0.kill() {
                eprintln!("RCA child cleanup kill failed: {error}");
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match self.0.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::yield_now();
                    }
                    Ok(None) => {
                        eprintln!("RCA child cleanup exceeded five-second deadline");
                        return;
                    }
                    Err(error) => {
                        eprintln!("RCA child cleanup wait failed: {error}");
                        return;
                    }
                }
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn native_birth_probe_records_alive_exited_unreaped_and_postmortem_phases() {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};

        let mut child = OwnedChild(
            Command::new("sh")
                .arg("-c")
                .arg("IFS= read -r release; printf RCA_CHILD_COMPLETE; exit 0")
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn owned barrier child"),
        );
        let pid = child.0.id();
        let (_, alive_probe) = rca_probe_record(pid);
        eprintln!(
            "RCA_SAMPLE phase=alive platform={} pid={} probe={} status=held_on_parent_pipe",
            std::env::consts::OS,
            pid,
            alive_probe
        );

        child
            .0
            .stdin
            .take()
            .expect("owned child stdin")
            .write_all(b"release\n")
            .expect("release owned child");

        let expected_pid = libc::pid_t::try_from(pid).expect("child pid fits pid_t");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: pid is this test's unreaped owned child. WNOWAIT retains it.
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    libc::id_t::try_from(pid).expect("child pid fits waitid id_t"),
                    &raw mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            assert_eq!(rc, 0, "waitid failed: {}", std::io::Error::last_os_error());
            if unsafe { info.si_pid() } == expected_pid {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned child did not exit"
            );
            std::thread::yield_now();
        }
        let (_, unreaped_probe) = rca_probe_record(pid);
        eprintln!(
            "RCA_SAMPLE phase=exited_unreaped platform={} pid={} probe={} status=waitid_WEXITED_WNOWAIT",
            std::env::consts::OS,
            pid,
            unreaped_probe
        );

        let status = child.0.wait().expect("reap only the owned child");
        assert_eq!(status.code(), Some(0));
        let mut output = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert_eq!(output, "RCA_CHILD_COMPLETE");
        let (_, postmortem_probe) = rca_probe_record(pid);
        eprintln!(
            "RCA_SAMPLE phase=postmortem_diagnostic_only platform={} pid={} probe={} status=owned_wait_exit0 authority=false pid_reuse_possible=true",
            std::env::consts::OS,
            pid,
            postmortem_probe
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_none_probe_fast_command_records_terminal_and_keeps_public_scope_fence() {
        // The legacy Option probe collapses missing-process and other probe failures;
        // this fixture asserts the observed None path, not a particular kernel cause.
        let ctx = test_context(&scope("legacy-none-fast-fence"));
        let dir = tempfile::tempdir().unwrap();
        let cmd = "printf RCA_LEGACY_SENTINEL; exit 0";
        let mut child = injected_unregistered_child(&ctx, dir.path(), cmd, |_| None);
        let pid = child.child.id().expect("owned legacy child pid");
        let status = tokio::time::timeout(Duration::from_secs(5), child.child.wait())
            .await
            .expect("injected owned child exits before legacy settlement")
            .expect("wait for injected owned child");
        assert_eq!(status.code(), Some(0));
        let registry = ctx.bash_handle_registry();
        let reservation = registry.reserve_spawn(&ctx.work_scope).await.unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                cmd,
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 0);
        assert_eq!(response["lines"][0]["bytes"], "RCA_LEGACY_SENTINEL");
        assert!(registry.reserve_spawn(&ctx.work_scope).await.is_err());
        let fence = registry
            .begin_teardown(&ctx.work_scope)
            .await
            .expect("public owner fence");
        assert_eq!(
            fence.scope_uncertainty().await,
            Some(crate::bash::registry::BashScopeUncertainty::UnregisteredLaunch)
        );
        eprintln!(
            "RCA_SAMPLE phase=legacy_none_probe platform={} pid={} probe=category=none status=unregistered_exited_exit0 public_spawn_rejected=true scope_uncertainty=unregistered_launch",
            std::env::consts::OS,
            pid
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_none_retains_terminal_output_exit_and_executes_once() {
        let ctx = test_context(&scope("identity-none"));
        let dir = tempfile::tempdir().unwrap();
        let cmd = "printf once >> count; printf 'stdout\\n'; printf 'stderr-tail' >&2; exit 23";
        let child = injected_unregistered_child(&ctx, dir.path(), cmd, |_| None);
        let registry = ctx.bash_handle_registry();
        let reservation = registry.reserve_spawn(&ctx.work_scope).await.unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                cmd,
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 23);
        assert_eq!(response["output_complete"], true);
        assert!(response.get("handle").is_none());
        assert!(response.get("error").is_none());
        let lines = response["lines"].as_array().unwrap();
        assert!(lines.iter().any(|line| line["bytes"] == "stdout"));
        assert!(lines.iter().any(|line| line["bytes"] == "stderr-tail"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("count")).unwrap(),
            "once"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unregistered_empty_line_flood_reports_evicted_capture_as_incomplete() {
        let ctx = test_context(&scope("identity-empty-line-flood"));
        let dir = tempfile::tempdir().unwrap();
        let cmd = "printf '\\n%.0s' {1..128}";
        let child = injected_unregistered_child(&ctx, dir.path(), cmd, |_| None);
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&ctx.work_scope)
            .await
            .unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                cmd,
                None,
                8,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 0);
        assert_eq!(response["output_complete"], false);
        assert_eq!(response["truncated_before"], true);
        assert_eq!(response["start_offset"], 120);
        assert_eq!(response["end_offset"], 128);
        let lines = response["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().all(|line| line["bytes"] == ""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_birth_probe_never_adopts_reused_pid_observation() {
        use phoenix_core::process_identity::{process_identity_matches, ProcessIdentity};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let ctx = test_context(&scope("identity-missing-birth"));
        let dir = tempfile::tempdir().unwrap();
        let calls = AtomicUsize::new(0);
        let mut replacement = None;
        let child = injected_unregistered_child(
            &ctx,
            dir.path(),
            "printf once >> count; printf success; exit 0",
            |pid| {
                calls.fetch_add(1, Ordering::SeqCst);
                // Model a later occupant of this numeric PID with a different birth.
                replacement = Some(ProcessIdentity {
                    pid,
                    start_time: u128::MAX,
                });
                None
            },
        );
        let registry = ctx.bash_handle_registry();
        let reservation = registry.reserve_spawn(&ctx.work_scope).await.unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                "success",
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 0);
        assert!(response.get("handle").is_none());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fallback must never reprobe and adopt a later birth"
        );
        assert!(!process_identity_matches(replacement.unwrap()));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("count")).unwrap(),
            "once"
        );
        // The wire contract accepts a terminal result with no handle.
        let decoded: BashResponse = serde_json::from_value(response).unwrap();
        assert!(matches!(decoded, BashResponse::UnregisteredExited(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delayed_missing_probe_preserves_owned_child_wait_status() {
        let ctx = test_context(&scope("identity-delayed"));
        let dir = tempfile::tempdir().unwrap();
        let child = injected_unregistered_child(&ctx, dir.path(), "printf tail; exit 7", |_| {
            // test-timing-allow: inject launch-thread descheduling before the missing identity observation to exercise owned-child wait status retention
            std::thread::sleep(Duration::from_millis(100));
            None
        });
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&ctx.work_scope)
            .await
            .unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                "printf tail; exit 7",
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["exit_code"], 7);
        assert_eq!(response["lines"][0]["bytes"], "tail");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unverified_live_child_is_directly_reaped_and_teardown_is_finite() {
        let mut ctx = test_context(&scope("identity-live"));
        ctx.bash_handles = Arc::new(crate::bash::registry::BashHandleRegistry::with_caps(
            RING_BUFFER_BYTES,
            1,
        ));
        let dir = tempfile::tempdir().unwrap();
        let cmd = "printf once >> count; printf 'before\\n'; while [ ! -e release ]; do sleep 0.01; done; printf after";
        let child = injected_unregistered_child(&ctx, dir.path(), cmd, |_| None);
        let registry = ctx.bash_handle_registry();
        let reservation = registry.reserve_spawn(&ctx.work_scope).await.unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                cmd,
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["status"], "unregistered_killed");
        assert_eq!(response["signal_number"], libc::SIGKILL);
        assert!(response.get("handle").is_none());
        assert!(response.get("pid").is_none());
        assert!(registry.reserve_spawn(&ctx.work_scope).await.is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("count")).unwrap(),
            "once"
        );
        let permit = tokio::time::timeout(
            Duration::from_secs(2),
            registry.begin_retirement(&ctx.work_scope),
        )
        .await
        .expect("settled launch kept teardown pending");
        assert!(permit.exact_process_groups.is_empty());
        let outcome = registry.complete_retirement(&permit).await;
        assert!(outcome.report().kill_failures.is_empty());
        assert_eq!(
            outcome.report().scope_uncertainty,
            Some(crate::bash::registry::BashScopeUncertainty::UnregisteredLaunch)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unverified_descendant_pipe_drain_is_bounded_without_group_signals() {
        let ctx = test_context(&scope("identity-descendant"));
        let dir = tempfile::tempdir().unwrap();
        let cmd = "(sleep 0.8; printf survived > descendant) & printf 'parent\\n'; exit 19";
        let child = injected_unregistered_child(&ctx, dir.path(), cmd, |_| None);
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&ctx.work_scope)
            .await
            .unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            settle_unregistered_run(
                child,
                reservation,
                cmd,
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            ),
        )
        .await
        .expect("descendant held settlement open");
        let response = response_json(&response);
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 19);
        assert_eq!(response["output_complete"], false);
        assert_eq!(response["descendants"], "unverified");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("descendant").exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unverified group was signalled");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn registration_failure_settles_child_without_claiming_spawn_failure() {
        let ctx = test_context(&scope("registration-failed"));
        let registry = ctx.bash_handle_registry();
        let mut reservation = registry.reserve_spawn(&ctx.work_scope).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut context = spawn_context_for(&ctx);
        context.working_dir = dir.path().to_path_buf();
        let (handle, spawned) = spawn_child(
            "while [ ! -e release ]; do sleep 0.01; done; printf registered-once; exit 6",
            None,
            &ctx,
            &context,
            reservation.handle_id().clone(),
            RING_BUFFER_BYTES,
            &BashSpawnMode::Direct,
        )
        .expect("spawn");
        // A registry with no owner table deterministically refuses registration.
        let missing_owner_registry = crate::bash::registry::BashHandleRegistry::new();
        assert!(missing_owner_registry
            .commit_spawn(&mut reservation, handle)
            .await
            .is_err());
        let (child, _launch_gate, sandbox_scratch_dir) = spawned.into_waiter_parts();
        std::fs::write(dir.path().join("release"), "").unwrap();
        let response = response_json(
            &settle_unregistered_run(
                UnregisteredChild {
                    child,
                    launch_gate: None,
                    sandbox_scratch_dir,
                    started_at: Instant::now(),
                },
                reservation,
                "registration test",
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::RegistrationFailed,
            )
            .await,
        );
        assert_eq!(response["launch_error"], "registration_failed");
        assert_eq!(response["status"], "unregistered_exited");
        assert_eq!(response["exit_code"], 6);
        assert_eq!(response["lines"][0]["bytes"], "registered-once");
        assert!(response.get("handle").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_unverified_response_before_poll_keeps_bounded_child_custody() {
        let ctx = test_context(&scope("identity-cancelled"));
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        let cmd = "while [ ! -e release ]; do sleep 0.01; done; printf done > finished";
        let mut child = injected_unregistered_child(&ctx, dir.path(), cmd, |_| None);
        child.sandbox_scratch_dir = Some(scratch.clone());
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&ctx.work_scope)
            .await
            .unwrap();
        let read_args = ReadArgs::default();
        let future = settle_unregistered_run(
            child,
            reservation,
            cmd,
            None,
            RING_BUFFER_BYTES,
            &read_args,
            BashLaunchError::IdentityUnavailable,
        );
        let registry = ctx.bash_handle_registry();
        assert!(matches!(
            registry.reserve_spawn(&ctx.work_scope).await,
            Err(BashHandleError::SpawnFenced)
        ));
        drop(future);
        let permit = tokio::time::timeout(
            Duration::from_secs(3),
            registry.begin_retirement(&ctx.work_scope),
        )
        .await
        .expect("cancelled caller lost bounded custody");
        let outcome = registry.complete_retirement(&permit).await;
        assert!(outcome.report().has_failures());
        assert!(
            scratch.exists(),
            "unverified descendants may still use scratch"
        );
        assert!(!dir.path().join("finished").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn child_with_cached_exit_retains_owned_wait_status() {
        let ctx = test_context(&scope("identity-reuse"));
        let dir = tempfile::tempdir().unwrap();
        let mut child =
            injected_unregistered_child(&ctx, dir.path(), "printf cached; exit 11", |_| None);
        let status = child.child.wait().await.unwrap();
        assert_eq!(status.code(), Some(11));
        assert!(child.child.id().is_none());
        let reservation = ctx
            .bash_handle_registry()
            .reserve_spawn(&ctx.work_scope)
            .await
            .unwrap();
        let response = response_json(
            &settle_unregistered_run(
                child,
                reservation,
                "cached",
                None,
                RING_BUFFER_BYTES,
                &ReadArgs::default(),
                BashLaunchError::IdentityUnavailable,
            )
            .await,
        );
        assert_eq!(response["exit_code"], 11);
        assert!(response.get("handle").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_child_injects_launch_uuid_and_captures_exact_identity() {
        let ctx = test_context(&scope("spawn-launch-identity"));
        let spawn_context = spawn_context_for(&ctx);
        let (handle, spawned) = spawn_child(
            "printf '%s' \"$PHOENIX_BASH_LAUNCH_UUID\"",
            None,
            &ctx,
            &spawn_context,
            HandleId::new("b-launch-identity"),
            RING_BUFFER_BYTES,
            &BashSpawnMode::Direct,
        )
        .expect("spawn child");
        let (mut child, launch_gate, scratch_dir) = spawned.into_waiter_parts();
        drop(launch_gate);
        assert!(scratch_dir.is_none());

        let mut stdout = child.stdout.take().expect("child stdout");
        let mut stderr = child.stderr.take().expect("child stderr");
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let status = child.wait().await.expect("wait child");
        assert!(status.success(), "child failed: {status:?}");
        let stdout = stdout_task
            .await
            .expect("join stdout")
            .expect("read stdout");
        let stderr = stderr_task
            .await
            .expect("join stderr")
            .expect("read stderr");
        assert!(
            stderr.is_empty(),
            "unexpected stderr: {:?}",
            String::from_utf8_lossy(&stderr)
        );
        let launch_uuid = String::from_utf8(stdout).expect("utf8 stdout");

        assert_eq!(launch_uuid, handle.launch_identity.launch_uuid);
        assert_eq!(
            handle.launch_identity.process.pid,
            handle.live_pid().await.expect("live pid")
        );
        assert!(handle.launch_identity.process.start_time > 0);
        assert_eq!(
            handle.launch_identity.process.pid,
            handle.live_pid().await.expect("live pid")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_unregistered_spawn_kills_the_process_group() {
        let ctx = test_context(&scope("drop-process-group"));
        let spawn_context = spawn_context_for(&ctx);
        let (_, mut spawned) = spawn_child(
            "(echo ready; exec sleep 30) & wait",
            None,
            &ctx,
            &spawn_context,
            HandleId::new("b-drop-process-group"),
            RING_BUFFER_BYTES,
            &BashSpawnMode::Direct,
        )
        .expect("spawn process group");
        let pgid = spawned.pgid();
        let scratch_parent = tempfile::tempdir().expect("scratch parent");
        let scratch = scratch_parent.path().join("spawn-scratch");
        std::fs::create_dir(&scratch).expect("create scratch");
        spawned.sandbox_scratch_dir = Some(scratch.clone());
        let stdout = spawned.child_mut().stdout.take().expect("child stdout");
        let mut ready = String::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            BufReader::new(stdout).read_line(&mut ready),
        )
        .await
        .expect("descendant readiness timed out")
        .expect("read descendant readiness");
        assert_eq!(ready.trim(), "ready");

        drop(spawned);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let result = unsafe { libc::kill(-pgid, 0) };
                if result != 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("process group survived guard drop");
        assert!(!scratch.exists(), "sandbox scratch survived guard drop");
    }

    fn test_context(work_scope: &ResourceScopeKey) -> ToolContext {
        let worktree_path = None;
        let conversation_id = match work_scope {
            ResourceScopeKey::Work(id) => id.as_str().to_string(),
            ResourceScopeKey::Unattached(conversation_id) => conversation_id.clone(),
            ResourceScopeKey::Coordinator => "coordinator".to_string(),
            ResourceScopeKey::GlobalTerminal => "conv-global".to_string(),
        };
        let mut ctx = crate::ToolContext::new(
            tokio_util::sync::CancellationToken::new(),
            conversation_id,
            std::env::current_dir().expect("cwd"),
            crate::browser::session::BrowserSessionManager::new(),
            Arc::new(crate::bash::registry::BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::default(),
            Arc::new(crate::tmux::registry::TmuxRegistry::new()),
            worktree_path,
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
        )
        .with_root_conversation_id("root-process".to_string())
        .with_tool_use_id("tool-process");
        if matches!(
            work_scope,
            ResourceScopeKey::Coordinator | ResourceScopeKey::GlobalTerminal
        ) {
            ctx.work_scope = work_scope.clone();
        }
        ctx
    }

    fn live_handle() -> Arc<Handle> {
        Handle::new_live(
            scope("conv-ops"),
            HandleId::new("b-1"),
            "emitter".into(),
            None,
            std::path::PathBuf::from("/tmp"),
            1234,
            1234,
            RING_BUFFER_BYTES,
        )
    }

    #[test]
    fn progress_lease_deactivates_reporter_on_drop() {
        let sink: Arc<dyn crate::BashProgressSink> = Arc::new(|_| {});
        let reporter = Arc::new(LiveBashProgressReporter::new(sink));
        {
            let _lease = LiveBashProgressLease(reporter.clone());
            assert!(reporter.is_active());
        }
        assert!(!reporter.is_active());
    }

    #[test]
    fn progress_projection_reports_byte_budget_truncation() {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let captured = emitted.clone();
        let sink: Arc<dyn crate::BashProgressSink> = Arc::new(move |progress| {
            captured.lock().expect("progress lock").push(progress);
        });
        let reporter = LiveBashProgressReporter::new(sink);
        let handle = live_handle();
        let mut ring = RingBuffer::new(RING_BUFFER_BYTES);
        ring.append(format!("{}\n", "x".repeat(LIVE_PROGRESS_MAX_BYTES + 100)).as_bytes());

        reporter.maybe_emit(&handle, &ring, true);

        let snapshots = emitted.lock().expect("progress lock");
        let snapshot = snapshots.last().expect("progress snapshot");
        assert!(snapshot.truncated_before);
        assert!(
            snapshot
                .lines
                .iter()
                .map(|line| line.text.len())
                .sum::<usize>()
                <= LIVE_PROGRESS_MAX_BYTES
        );
    }

    #[test]
    fn progress_projection_reports_line_limit_truncation() {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let captured = emitted.clone();
        let sink: Arc<dyn crate::BashProgressSink> = Arc::new(move |progress| {
            captured.lock().expect("progress lock").push(progress);
        });
        let reporter = LiveBashProgressReporter::new(sink);
        let handle = live_handle();
        let mut ring = RingBuffer::new(RING_BUFFER_BYTES);
        for index in 0..=LIVE_PROGRESS_MAX_LINES {
            ring.append(format!("line {index}\n").as_bytes());
        }

        reporter.maybe_emit(&handle, &ring, true);

        let snapshots = emitted.lock().expect("progress lock");
        let snapshot = snapshots.last().expect("progress snapshot");
        assert!(snapshot.truncated_before);
        assert_eq!(snapshot.lines.len(), LIVE_PROGRESS_MAX_LINES);
    }

    #[test]
    fn exit_code_128_is_not_signal_zero() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 128")
            .status()
            .expect("spawn shell");
        assert!(matches!(
            exit_status_to_cause(status),
            FinalCause::Exited {
                exit_code: Some(128)
            }
        ));
    }

    #[test]
    fn exit_code_137_maps_to_signal_nine() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 137")
            .status()
            .expect("spawn shell");
        assert!(matches!(
            exit_status_to_cause(status),
            FinalCause::Killed {
                exit_code: Some(137),
                signal_number: Some(9)
            }
        ));
    }

    #[test]
    fn read_window_from_ring_exposes_live_partial() {
        let mut ring = RingBuffer::new(RING_BUFFER_BYTES);
        ring.append(b"done\nin-progress");
        let read = read_window_from_ring(&ring, &ReadArgs::default());
        // Complete line is in `view.lines`; the un-newlined tail is `partial`.
        assert_eq!(read.view.lines.len(), 1);
        assert_eq!(read.partial.as_deref(), Some("in-progress"));
        // window_to_typed threads the partial onto the wire window.
        let window = window_to_typed(&read.view, read.partial);
        assert_eq!(window.partial.as_deref(), Some("in-progress"));
        assert_eq!(window.lines.len(), 1);
    }

    #[test]
    fn read_window_from_ring_partial_none_when_no_trailing_fragment() {
        let mut ring = RingBuffer::new(RING_BUFFER_BYTES);
        ring.append(b"complete\n");
        let read = read_window_from_ring(&ring, &ReadArgs::default());
        assert!(read.partial.is_none());
    }

    #[tokio::test]
    async fn tombstone_read_window_has_no_partial() {
        // EOF flushes the partial to a complete line before the tombstone is
        // written, so a tombstone read carries partial: None.
        let h = live_handle();
        {
            let state = h.state().await;
            let HandleState::Live(live) = state.as_ref() else {
                panic!("live");
            };
            let mut ring = live.ring.lock().await;
            ring.append(b"line\nno-nl");
            ring.flush_partial(); // simulate EOF flush
        }
        h.transition_to_terminal(
            FinalCause::Exited { exit_code: Some(0) },
            Duration::from_millis(1),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            crate::bash::handle::TOMBSTONE_TAIL_LINES,
        )
        .await;
        let state = h.state().await;
        let HandleState::Tombstoned(t) = state.as_ref() else {
            panic!("tombstoned");
        };
        let view = read_window_from_tombstone(t, &ReadArgs::default());
        let window = window_to_typed(&view, None);
        assert!(window.partial.is_none());
        // The flushed "no-nl" fragment is now a complete line in the tail.
        assert!(window.lines.iter().any(|l| l.bytes == "no-nl"));
    }

    /// A reader that yields one chunk, then stalls forever (`Poll::Pending`).
    /// Models a process that emits an un-newlined fragment and goes quiet.
    struct OneChunkThenStall {
        chunk: Option<Vec<u8>>,
    }

    impl AsyncRead for OneChunkThenStall {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(chunk) = self.chunk.take() {
                buf.put_slice(&chunk);
                Poll::Ready(Ok(()))
            } else {
                // Never wakes — the only thing that can fire now is the
                // reader loop's idle-flush timer.
                Poll::Pending
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn idle_flush_emits_partial_as_line_via_reader_timeout() {
        let h = live_handle();
        let reader = OneChunkThenStall {
            chunk: Some(b"stuck-no-newline".to_vec()),
        };
        let h2 = h.clone();
        let task = tokio::spawn(async move {
            read_pipe_to_ring(reader, h2, None).await;
        });

        // Let the chunk land in `partial` before any timer fires.
        tokio::task::yield_now().await;
        {
            let state = h.state().await;
            let HandleState::Live(live) = state.as_ref() else {
                panic!("live");
            };
            let ring = live.ring.lock().await;
            assert_eq!(ring.len(), 0, "no complete line yet — held as partial");
            assert!(ring.has_partial());
        }

        // Advance past the idle-flush threshold; the select! timer wins.
        tokio::time::advance(Duration::from_secs(PARTIAL_IDLE_FLUSH_SECONDS + 1)).await;
        tokio::task::yield_now().await;

        {
            let state = h.state().await;
            let HandleState::Live(live) = state.as_ref() else {
                panic!("live");
            };
            let ring = live.ring.lock().await;
            assert_eq!(ring.len(), 1, "idle timer flushed the partial as a line");
            assert!(!ring.has_partial());
            assert_eq!(ring.since(0).lines[0].bytes, b"stuck-no-newline");
        }

        task.abort();
    }

    /// A reader whose completion is gated by a oneshot signal the test
    /// fires explicitly, rather than by real elapsed time. Models the
    /// kernel-buffer race directly: the final chunk (including an
    /// unterminated tail with no trailing newline) is already "in the
    /// pipe" when the gate is constructed, standing in for bytes the
    /// kernel buffered before the child exited; the test controls
    /// exactly when the reader is allowed to observe it.
    struct GatedReader {
        gate: Option<tokio::sync::oneshot::Receiver<()>>,
        chunk: Option<Vec<u8>>,
    }

    impl AsyncRead for GatedReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(gate) = self.gate.as_mut() {
                match Pin::new(gate).poll(cx) {
                    Poll::Ready(_) => self.gate = None,
                    Poll::Pending => return Poll::Pending,
                }
            }
            if let Some(chunk) = self.chunk.take() {
                buf.put_slice(&chunk);
            }
            // Gate released and chunk (if any) already delivered: EOF.
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn waiter_cannot_terminalize_before_reader_drains_unterminated_tail() {
        use std::future::Future as _;
        let handle = live_handle();
        let marker = b"final-marker-unterminated".to_vec();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let reader = GatedReader {
            gate: Some(release_rx),
            chunk: Some(marker.clone()),
        };

        let reader_handle = handle.clone();
        let stdout_join = tokio::spawn(async move {
            read_pipe_to_ring(reader, reader_handle, None).await;
        });

        let context = WaiterContext {
            owner_scope: scope("conv-ops"),
            lifecycle_sink: None,
            terminal_effect: BashTerminalEffect::InventoryOnly,
            sandbox_scratch_dir: None,
            progress_reporter: None,
        };
        let mut finalize = Box::pin(run_waiter(
            handle.clone(),
            std::future::ready(Ok(std::process::ExitStatus::from_raw(0))),
            Some(stdout_join),
            None,
            context,
        ));
        let pending = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(finalize.as_mut().poll(cx).is_pending())
        })
        .await;
        assert!(pending, "finalization must wait for the gated reader");
        assert!(!handle.state().await.is_terminal());
        release_tx.send(()).expect("reader still awaits its gate");
        finalize.await;

        let state = handle.state().await;
        let HandleState::Tombstoned(tomb) = state.as_ref() else {
            panic!("expected Tombstoned after drain, got {state:?}");
        };
        assert!(
            tomb.final_tail
                .iter()
                .any(|l| l.bytes.ends_with(marker.as_slice())),
            "tombstone tail missing final unterminated marker; tail: {:?}",
            tomb.final_tail
                .iter()
                .map(|l| String::from_utf8_lossy(&l.bytes).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn waiter_bounds_reader_drain_when_a_descendant_keeps_pipe_open() {
        use std::future::Future as _;
        let handle = live_handle();
        let (release, gate) = tokio::sync::oneshot::channel();
        let reader_handle = handle.clone();
        let reader = tokio::spawn(async move {
            read_pipe_to_ring(
                GatedReader {
                    gate: Some(gate),
                    chunk: Some(b"late".to_vec()),
                },
                reader_handle,
                None,
            )
            .await;
        });
        let context = WaiterContext {
            owner_scope: scope("conv-ops"),
            lifecycle_sink: None,
            terminal_effect: BashTerminalEffect::InventoryOnly,
            sandbox_scratch_dir: None,
            progress_reporter: None,
        };
        let mut waiter = Box::pin(run_waiter(
            handle.clone(),
            std::future::ready(Ok(std::process::ExitStatus::from_raw(0))),
            Some(reader),
            None,
            context,
        ));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(
                waiter.as_mut().poll(cx).is_pending()
            ))
            .await
        );
        assert!(!handle.state().await.is_terminal());
        tokio::time::advance(READER_DRAIN_TIMEOUT).await;
        waiter.await;
        assert!(handle.state().await.is_terminal());
        drop(release);
    }

    #[tokio::test]
    async fn background_run_returns_live_handle() {
        let ctx = test_context(&scope("conv-process"));
        let output = run_run(
            "sleep 10",
            None,
            0,
            ReadArgs::default(),
            &ctx,
            &spawn_context_for(&ctx),
            BashSpawnMode::Direct,
        )
        .await;
        assert!(output.is_success(), "{}", output.output());
        let value = output.display_data().cloned().expect("display data");
        assert_eq!(value["status"], "still_running");
        let handle_id = value["handle"].as_str().expect("background handle");
        let provider_value: Value = serde_json::from_str(output.output()).expect("provider JSON");
        assert_eq!(provider_value["handle"], handle_id);
        let peek = run_peek(handle_id, ReadArgs::default(), &ctx).await;
        assert!(peek.is_success(), "{}", peek.output());
    }

    #[tokio::test]
    async fn synchronous_run_returns_terminal_output() {
        let ctx = test_context(&scope("conv-process"));
        let output = run_run(
            "printf hi",
            None,
            5,
            ReadArgs::default(),
            &ctx,
            &spawn_context_for(&ctx),
            BashSpawnMode::Direct,
        )
        .await;
        assert!(output.is_success(), "{}", output.output());
        let value: serde_json::Value = serde_json::from_str(output.output()).expect("json");
        assert_eq!(value["status"], "exited");
    }

    #[test]
    fn explicit_target_handles_are_visible_across_scope_actors() {
        let mode = BashSpawnMode::ExplicitTarget(ValidatedBashSpawnTarget {
            working_dir: std::path::PathBuf::from("/tmp"),
            lifecycle_scope: phoenix_core::work_scope::WorkScopeId::parse("work-scope")
                .expect("work scope"),
        });
        assert_eq!(
            mode.authority(),
            phoenix_core::work_scope::ResourceAuthority::Work
        );
        assert_eq!(
            BashSpawnMode::ExploreReadOnly.authority(),
            phoenix_core::work_scope::ResourceAuthority::Restricted
        );
        let successor = phoenix_core::work_scope::EffectiveResourceAccess::new(
            "coordinator-successor",
            mode.authority(),
        );
        assert!(successor.can_control("coordinator-predecessor", mode.authority()));
    }

    #[tokio::test]
    async fn coordinator_background_run_preserves_handle() {
        let ctx = test_context(&ResourceScopeKey::Coordinator);
        let output = run_run(
            "sleep 10",
            None,
            0,
            ReadArgs::default(),
            &ctx,
            &SpawnContext {
                working_dir: ctx.working_dir().to_path_buf(),
                lifecycle_scope: ResourceScopeKey::Work(
                    phoenix_core::work_scope::WorkScopeId::parse("coordinator-target").unwrap(),
                ),
                terminal_effect: BashTerminalEffect::InventoryOnly,
            },
            BashSpawnMode::Direct,
        )
        .await;

        assert!(output.is_success(), "{}", output.output());
        let value: Value = serde_json::from_str(output.output()).expect("json");
        assert_eq!(value["status"], "still_running");
        assert!(value["handle"].as_str().is_some());
    }

    #[tokio::test]
    async fn global_scope_is_rejected() {
        let ctx = test_context(&ResourceScopeKey::GlobalTerminal);
        let output = run_run(
            "sleep 10",
            None,
            0,
            ReadArgs::default(),
            &ctx,
            &spawn_context_for(&ctx),
            BashSpawnMode::Direct,
        )
        .await;
        assert!(!output.is_success());
        assert!(output
            .output()
            .contains("bash processes require a conversation-scoped owner"));
    }

    #[tokio::test]
    async fn background_run_preserves_handle() {
        let ctx = test_context(&scope("conv-process"));
        let output = run_run(
            "sleep 10",
            None,
            0,
            ReadArgs::default(),
            &ctx,
            &spawn_context_for(&ctx),
            BashSpawnMode::Direct,
        )
        .await;
        assert!(output.is_success(), "{}", output.output());
        let value: serde_json::Value = serde_json::from_str(output.output()).expect("json");
        assert_eq!(value["status"], "still_running");
    }

    #[tokio::test]
    async fn run_cancellation_preserves_background_handle() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let mut ctx = test_context(&scope("conv-process"));
        ctx.cancel = cancel;
        let output = run_run(
            "sleep 10",
            None,
            10,
            ReadArgs::default(),
            &ctx,
            &spawn_context_for(&ctx),
            BashSpawnMode::Direct,
        )
        .await;
        assert!(output.is_success(), "{}", output.output());
        let value: serde_json::Value = serde_json::from_str(output.output()).expect("json");
        assert_eq!(value["status"], "still_running");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn kill_pending_emits_non_reconciling_phase_before_true_terminal() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let socket_dir = tempfile::tempdir().expect("socket dir");
        let registry =
            Arc::new(crate::bash::registry::BashHandleRegistry::with_lifecycle_sink(Some(tx)));
        let ctx = crate::ToolContext::new(
            tokio_util::sync::CancellationToken::new(),
            "conv-kill-pending".to_string(),
            std::env::current_dir().expect("cwd"),
            crate::browser::session::BrowserSessionManager::new(),
            registry,
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::default(),
            Arc::new(crate::tmux::registry::TmuxRegistry::with_socket_dir(
                socket_dir.path().to_path_buf(),
            )),
            None,
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
        );

        // The `trap '' TERM` sets SIGTERM to SIG_IGN, inherited by the child
        // `sleep`, so the whole process group ignores TERM. The kill below
        // must arrive AFTER the trap builtin has run, or SIGTERM hits a
        // group with the default disposition and kills it — producing a fast
        // exit and a `Terminal` edge instead of `KillPendingKernel`. Announce
        // readiness on stdout after installing the trap and wait for the
        // marker rather than betting on a fixed sleep (which races under load).
        let run = run_run(
            "trap '' TERM; echo trap-installed; sleep 3600",
            None,
            0,
            ReadArgs::default(),
            &ctx,
            &spawn_context_for(&ctx),
            BashSpawnMode::Direct,
        )
        .await;
        let value: serde_json::Value = serde_json::from_str(run.output()).expect("run json");
        let handle_id = value["handle"].as_str().expect("handle id").to_string();

        let spawned = rx.recv().await.expect("spawned event");
        assert_eq!(spawned.phase, BashLifecyclePhase::Spawned);
        assert_eq!(spawned.owner, ctx.work_scope);
        assert_eq!(
            spawned.handle_id.as_ref().map(HandleId::as_str),
            Some(handle_id.as_str())
        );
        assert_eq!(spawned.terminal_effect, None);

        // Wait for the trap-installed marker to appear in the ring. Deterministic
        // proof the trap is in place; no wall-clock assumption about bash startup.
        let ready_handle = lookup_handle(&ctx, &handle_id)
            .await
            .expect("lookup handle for readiness");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                {
                    let state = ready_handle.state().await;
                    if let HandleState::Live(live) = state.as_ref() {
                        let ring = live.ring.lock().await;
                        if ring
                            .since(0)
                            .lines
                            .iter()
                            .any(|l| l.bytes == b"trap-installed")
                        {
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("trap-installed marker did not appear within 10s");

        // The process ignores TERM, so `exit_rx` can never fire on this kill —
        // the kill-response timeout branch always wins. A short timeout drives
        // it to `KillPendingKernel` in milliseconds instead of the production
        // 30s, with no loss of determinism (nothing races the timeout here).
        let kill_task = tokio::spawn({
            let ctx = ctx.clone();
            let handle_id = handle_id.clone();
            async move {
                run_kill(
                    &handle_id,
                    KillSignal::Term,
                    Duration::from_millis(500),
                    &ctx,
                )
                .await
            }
        });

        let kill_pending = rx.recv().await.expect("kill-pending event");
        assert_eq!(kill_pending.phase, BashLifecyclePhase::KillPendingKernel);
        assert_eq!(kill_pending.owner, ctx.work_scope);
        assert_eq!(
            kill_pending.handle_id.as_ref().map(HandleId::as_str),
            Some(handle_id.as_str())
        );
        assert_eq!(kill_pending.terminal_effect, None);

        let kill_output = kill_task.await.expect("kill task");
        let kill_value: serde_json::Value =
            serde_json::from_str(kill_output.output()).expect("kill json");
        assert_eq!(kill_value["status"], "kill_pending_kernel");

        let handle = lookup_handle(&ctx, &handle_id)
            .await
            .expect("lookup handle");
        let live_state = handle.state().await;
        assert!(matches!(live_state.as_ref(), HandleState::Live(_)));

        // SIGKILL cannot be ignored, so the process exits and the `exited`
        // branch wins well before any timeout — keep the production timeout so
        // this call never emits a spurious KillPendingKernel ahead of Terminal.
        let _ = run_kill(
            &handle_id,
            KillSignal::Kill,
            Duration::from_secs(KILL_RESPONSE_TIMEOUT_SECONDS),
            &ctx,
        )
        .await;
        let terminal = rx.recv().await.expect("terminal event");
        assert_eq!(terminal.phase, BashLifecyclePhase::Terminal);
        assert_eq!(terminal.owner, ctx.work_scope);
        assert_eq!(
            terminal.handle_id.as_ref().map(HandleId::as_str),
            Some(handle_id.as_str())
        );
        assert_eq!(
            terminal.terminal_effect,
            Some(BashTerminalEffect::InventoryAndBranchReconcile)
        );
    }
}
