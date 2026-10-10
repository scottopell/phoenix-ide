//! Tmux pass-through agent tool.
//!
//! REQ-TMUX-003 (pure pass-through), REQ-TMUX-009 (description text),
//! REQ-TMUX-010 (cancellation/output limits), REQ-TMUX-011 (Phoenix-
//! injected `-S` first), REQ-TMUX-012 (response shape), REQ-TMUX-013
//! (`ToolContext::tmux()` accessor).
//!
//! See `specs/tmux-integration/{requirements,design}.md` and
//! `specs/tmux-integration/tmux-integration.allium` for the
//! authoritative behavioural specification.

pub mod backend;
#[cfg(any(test, feature = "test-support"))]
pub mod fake_backend;
pub mod invoke;
pub mod probe;
pub mod registry;
pub mod run;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const EXIT_MARKER_SENTINEL: &str = "__PHOENIX_EXIT__";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxExitMarker {
    pub exit_code: i32,
    pub occurred_at: SystemTime,
}

#[must_use]
pub fn format_exit_marker(exit_code: i32, occurred_at: SystemTime) -> String {
    let occurred_at_ms = occurred_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis();
    format!("{EXIT_MARKER_SENTINEL} exit_code={exit_code} occurred_at_ms={occurred_at_ms}")
}

#[must_use]
pub fn parse_exit_marker(line: &str) -> Option<TmuxExitMarker> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix(EXIT_MARKER_SENTINEL)?;
    let mut exit_code = None;
    let mut occurred_at_ms = None;
    for part in rest.split_whitespace() {
        if let Some(value) = part.strip_prefix("exit_code=") {
            exit_code = value.parse::<i32>().ok();
        } else if let Some(value) = part.strip_prefix("occurred_at_ms=") {
            occurred_at_ms = value.parse::<u64>().ok();
        }
    }
    Some(TmuxExitMarker {
        exit_code: exit_code?,
        occurred_at: UNIX_EPOCH.checked_add(Duration::from_millis(occurred_at_ms?))?,
    })
}

#[must_use]
pub fn parse_last_exit_marker(output: &str) -> Option<TmuxExitMarker> {
    output.lines().rev().find_map(parse_exit_marker)
}

pub use registry::{TmuxError, TmuxLifecycleEvent, TmuxLifecycleSink, TmuxRegistry, TmuxServer};
pub use run::TmuxRunTool;

// `cascade_tmux_on_delete`, `socket_path_for`, `CascadeReport`, and
// `ServerStatus` exist on the registry for task 02696 (bedrock hard-
// delete cascade orchestrator) and task 02697 (wire types). Until
// those land they're allow(dead_code) at the definition site rather
// than re-exported here.

use std::process::Stdio;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Tool, ToolContext, ToolOutput};
use invoke::{
    truncate_pair, TMUX_OUTPUT_MAX_BYTES, TMUX_TOOL_DEFAULT_WAIT_SECONDS,
    TMUX_TOOL_MAX_WAIT_SECONDS,
};
use phoenix_core::domain::tool_wire::{TmuxErrorResponse, TmuxToolResponse};

/// Pass-through tmux tool.
///
/// Stateless dispatcher; per-conversation state lives in
/// [`TmuxRegistry`], reached through [`ToolContext::tmux`]. A single
/// instance is registered once and reused across conversations
/// (REQ-TMUX-013).
pub struct TmuxTool;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TmuxInput {
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    wait_seconds: Option<u64>,
}

#[async_trait]
impl Tool for TmuxTool {
    // clearable: re-queryable read — see specs/stale-tool-results (REQ-STR-002).
    fn clearable(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "tmux"
    }

    fn description(&self) -> String {
        // Mirrors specs/tmux-integration/design.md §"Description
        // Template", with the configured byte limit interpolated.
        let max_kb = TMUX_OUTPUT_MAX_BYTES / 1024;
        format!(
            r#"Invokes tmux against this conversation's dedicated socket. The full tmux CLI
is available; provide the subcommand + flags as `args`.

This conversation's tmux server is isolated from every other conversation
and from any tmux server you may have running on the host: the socket path
is fixed by Phoenix and cannot be overridden by passing -L or -S in args.
If you do pass them, tmux will reject the duplicate server-selection flag
with a usage error.

Use `tmux_run` for starting dev servers, watchers, REPLs, or other
inspectable shell commands. It chooses the current project/worktree directory,
wraps the command with bash -lc, prints a visible exit marker, and keeps the
pane inspectable after exit by default.

Use this raw tmux tool for detailed tmux operations: `capture-pane`,
`send-keys`, `list-windows`, `kill-window`, or lower-level tmux commands.
Raw tmux is pass-through except for Phoenix's socket/config injection; it does
not enforce a cwd for newly-created windows or panes.

Common subcommands:
  new-window -d -n NAME COMMAND     spawn a new window running COMMAND
  list-windows                       enumerate windows in the current session
  capture-pane -p -t NAME -S -2000   read up to 2000 lines of scrollback
                                     for window NAME
  send-keys -t NAME "input" Enter    send input to a window
  kill-window -t NAME                terminate a window
  kill-server                        terminate this conversation's tmux server
                                     (rare; conversation hard-delete does
                                      this automatically)

Use bash for one-shot non-interactive commands.

Note: this tool's response shape differs from the bash tool. Bash returns
status/handle/exit_code/lines; this tool returns
status/exit_code/duration_ms/stdout/stderr/truncated. stdout and stderr
are kept SEPARATE here because tmux subcommands emit structured CLI
output where the distinction matters (capture-pane to stdout, warnings
to stderr).

Combined stdout+stderr beyond {max_kb} KB is middle-truncated.

Persistence is across Phoenix restart only, not system reboot. After a
host reboot, this server's state is lost; the next operation creates a
fresh server."#
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["args"],
            "properties": {
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Subcommand and its arguments, e.g. [\"new-window\", \"-d\", \"-n\", \"serve\", \"./serve\"]"
                },
                "wait_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 900,
                    "description": "Max seconds to block on the subprocess (default 30)"
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: TmuxInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return error_envelope("invalid_input", &format!("invalid tmux input: {e}")),
        };

        let wait_seconds = parsed
            .wait_seconds
            .unwrap_or(TMUX_TOOL_DEFAULT_WAIT_SECONDS);
        if wait_seconds == 0 || wait_seconds > TMUX_TOOL_MAX_WAIT_SECONDS {
            return error_envelope(
                "wait_seconds_out_of_range",
                &format!(
                    "wait_seconds must be in 1..={TMUX_TOOL_MAX_WAIT_SECONDS}; got {wait_seconds}"
                ),
            );
        }

        // Resolve the conversation's tmux server. Errors here are a
        // structural failure of the registry, not a tmux exit; they get
        // their own error ids.
        let server_arc = match ctx.tmux().await {
            Ok(arc) => arc,
            Err(TmuxError::BinaryUnavailable) => {
                return error_envelope(
                    "tmux_binary_unavailable",
                    "the tmux binary is not installed on this host",
                );
            }
            Err(e) => {
                return error_envelope("tmux_server_unavailable", &e.to_string());
            }
        };
        let socket_path = {
            let server = server_arc.read().await;
            server.socket_path.clone()
        };
        let config_path = ctx.tmux_registry().config_path();

        // `-f` only loads when tmux must spawn a fresh server. For a
        // running server the flag is benign; we include it so any
        // auto-spawn path uses the Phoenix config.
        let full_args = invocation_args(&config_path, &socket_path, parsed.args);

        let started = Instant::now();
        let mut cmd = tokio::process::Command::new("tmux");
        cmd.args(&full_args)
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tracing::debug!(argv = ?full_args, "tmux pass-through invocation");

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return error_envelope(
                    "tmux_spawn_failed",
                    &format!("failed to spawn tmux subprocess: {e}"),
                );
            }
        };

        run_with_timeout(child, wait_seconds, started, ctx).await
    }
}

fn invocation_args(
    config_path: &std::path::Path,
    socket_path: &std::path::Path,
    args: Vec<String>,
) -> Vec<String> {
    let mut full_args = vec![
        "-f".into(),
        config_path.to_string_lossy().into_owned(),
        "-S".into(),
        socket_path.to_string_lossy().into_owned(),
    ];
    full_args.extend(args);
    full_args
}

enum RunOutcome {
    Cancelled,
    TimedOut,
    Exited(std::io::Result<std::process::ExitStatus>),
}

/// Drive the subprocess to completion, racing against `wait_seconds`
/// and the cancellation token.
///
/// stdout and stderr are taken off the child up-front and drained by
/// concurrent reader tasks. This matters for commands that emit more
/// than the OS pipe buffer (~64 KB on Linux): a pure `child.wait()`
/// would wedge because the child blocks writing while no one reads,
/// then we'd hit `wait_seconds` and report `timed_out` with empty
/// output. With concurrent readers, the child can keep writing past
/// the buffer and we still observe its true exit.
///
/// On wait → readers EOF as the child closes its pipes; we join them.
/// On cancel/timeout → we kill the child, then join the readers (their
/// pipes EOF on kill); whatever bytes the child emitted before death
/// are preserved.
async fn run_with_timeout(
    mut child: tokio::process::Child,
    wait_seconds: u64,
    started: Instant,
    ctx: ToolContext,
) -> ToolOutput {
    let cancel = ctx.cancel.clone();
    let timeout = tokio::time::sleep(Duration::from_secs(wait_seconds));
    tokio::pin!(timeout);

    // Spawn drain tasks BEFORE racing on wait. Once stdout/stderr are
    // taken off `child`, the Child is otherwise unaffected — wait()
    // and start_kill() still work — and we can keep ownership across
    // all select arms.
    let stdout_task = spawn_drain_task(child.stdout.take());
    let stderr_task = spawn_drain_task(child.stderr.take());

    let outcome = tokio::select! {
        biased;
        () = cancel.cancelled() => RunOutcome::Cancelled,
        () = &mut timeout => RunOutcome::TimedOut,
        wait_result = child.wait() => RunOutcome::Exited(wait_result),
    };

    match outcome {
        RunOutcome::Cancelled => {
            // Kill so the readers EOF promptly; ignore output.
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            // Drain readers (bounded — pipes already closed).
            let _ = tokio::time::timeout(Duration::from_secs(1), stdout_task).await;
            let _ = tokio::time::timeout(Duration::from_secs(1), stderr_task).await;
            structured_response(
                "cancelled",
                None,
                started.elapsed().as_millis(),
                "",
                "",
                false,
            )
        }
        RunOutcome::TimedOut => {
            // Kill the child, then capture whatever the readers got
            // before the kill closed the pipes.
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let stdout = collect_drain(stdout_task).await;
            let stderr = collect_drain(stderr_task).await;
            let (so, se, truncated) = truncate_pair(&stdout, &stderr);
            structured_response(
                "timed_out",
                None,
                u128::from(wait_seconds) * 1000,
                &so,
                &se,
                truncated,
            )
        }
        RunOutcome::Exited(Ok(status)) => {
            // Child exited; pipes EOF; readers finish. Join them.
            let stdout = collect_drain(stdout_task).await;
            let stderr = collect_drain(stderr_task).await;
            let (so, se, truncated) = truncate_pair(&stdout, &stderr);
            structured_response(
                "ok",
                status.code(),
                started.elapsed().as_millis(),
                &so,
                &se,
                truncated,
            )
        }
        RunOutcome::Exited(Err(e)) => error_envelope(
            "tmux_wait_failed",
            &format!("failed to wait on tmux subprocess: {e}"),
        ),
    }
}

/// Spawn a tokio task that reads `reader` to EOF, returning the
/// collected bytes via the task's `JoinHandle`. Returns a handle that
/// resolves to an empty `Vec` when the reader is `None`.
fn spawn_drain_task<R>(reader: Option<R>) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let Some(mut r) = reader else {
            return Vec::new();
        };
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf).await;
        buf
    })
}

/// Bounded join on a drain task. The 2-second timeout protects against
/// pathological pipe-fd-leak scenarios (e.g. a tmux child somehow
/// fork-and-keep that holds the write end open after `kill-server`).
/// Under normal operation the join resolves immediately because the
/// pipe has already EOF'd by the time we reach this call.
async fn collect_drain(task: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(2), task).await {
        Ok(Ok(buf)) => buf,
        Ok(Err(error)) if error.is_panic() => {
            tracing::warn!(%error, "tmux output drain task panicked; dropping output");
            Vec::new()
        }
        Ok(Err(error)) => {
            tracing::debug!(%error, "tmux output drain task was cancelled; dropping output");
            Vec::new()
        }
        Err(error) => {
            tracing::debug!(%error, "tmux output drain timed out; dropping output");
            Vec::new()
        }
    }
}

fn structured_response(
    status: &str,
    exit_code: Option<i32>,
    duration_ms: u128,
    stdout: &str,
    stderr: &str,
    truncated: bool,
) -> ToolOutput {
    let typed = TmuxToolResponse {
        status: status.to_string(),
        exit_code,
        duration_ms: u64::try_from(duration_ms).unwrap_or(u64::MAX),
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        truncated,
    };
    let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::success(serialized).with_display(value)
}

fn error_envelope(error_id: &str, message: &str) -> ToolOutput {
    let typed = TmuxErrorResponse {
        error: error_id.to_string(),
        message: message.to_string(),
    };
    let value = serde_json::to_value(&typed).unwrap_or(Value::Null);
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    ToolOutput::error(serialized).with_display(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BashHandleRegistry, BrowserSessionManager};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn parse_response(out: &ToolOutput) -> Value {
        out.display_data()
            .cloned()
            .or_else(|| serde_json::from_str(out.output()).ok())
            .expect("response should be JSON")
    }

    fn ctx_with_registry(registry: Arc<TmuxRegistry>) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            std::env::temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            registry,
            None,
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
        )
    }

    #[tokio::test]
    async fn collect_drain_returns_successful_output() {
        let task = tokio::spawn(async { b"captured".to_vec() });
        assert_eq!(collect_drain(task).await, b"captured");
    }

    #[tokio::test]
    async fn collect_drain_drops_cancelled_task_output() {
        let task = tokio::spawn(std::future::pending::<Vec<u8>>());
        task.abort();
        assert!(collect_drain(task).await.is_empty());
    }

    #[tokio::test]
    async fn collect_drain_drops_panicked_task_output() {
        let task = tokio::spawn(async { panic!("drain panic test") });
        assert!(collect_drain(task).await.is_empty());
    }

    #[tokio::test]
    async fn binary_unavailable_returns_error_envelope() {
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_backend(
            tmp.path().to_path_buf(),
            crate::tmux::fake_backend::FakeTmuxBackend::unavailable(),
            None,
        ));
        let result = TmuxTool
            .run(
                json!({"args": ["list-sessions"]}),
                ctx_with_registry(registry),
            )
            .await;
        assert!(!result.is_success());
        assert_eq!(parse_response(&result)["error"], "tmux_binary_unavailable");
    }

    #[tokio::test]
    async fn wait_seconds_validation_precedes_registry_resolution() {
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_backend(
            tmp.path().to_path_buf(),
            crate::tmux::fake_backend::FakeTmuxBackend::unavailable(),
            None,
        ));
        let result = TmuxTool
            .run(
                json!({"args": ["list-sessions"], "wait_seconds": 5000}),
                ctx_with_registry(registry),
            )
            .await;
        assert!(!result.is_success());
        assert_eq!(
            parse_response(&result)["error"],
            "wait_seconds_out_of_range"
        );
    }

    #[test]
    fn phoenix_prefixes_authoritative_socket_and_preserves_agent_args() {
        let args = invocation_args(
            std::path::Path::new("/phoenix/config"),
            std::path::Path::new("/phoenix/socket"),
            vec![
                "-L".into(),
                "agent-label".into(),
                "-S/agent/socket".into(),
                "list-sessions".into(),
            ],
        );
        assert_eq!(
            args,
            [
                "-f",
                "/phoenix/config",
                "-S",
                "/phoenix/socket",
                "-L",
                "agent-label",
                "-S/agent/socket",
                "list-sessions"
            ]
        );
    }

    #[tokio::test]
    async fn cancelled_pending_invocation_reports_cancelled() {
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let cancel = CancellationToken::new();
        let ctx = ToolContext::new(
            cancel.clone(),
            "test-conv".to_string(),
            std::env::temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(TmuxRegistry::with_socket_dir(std::env::temp_dir())),
            None,
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
        );
        let cancel_task = tokio::spawn(async move {
            tokio::task::yield_now().await;
            cancel.cancel();
        });
        let result = run_with_timeout(child, 30, Instant::now(), ctx).await;
        cancel_task.await.unwrap();
        assert_eq!(parse_response(&result)["status"], "cancelled");
    }
}
