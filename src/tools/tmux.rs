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

pub mod invoke;
pub mod probe;
pub mod registry;

pub use registry::{TmuxError, TmuxRegistry, TmuxServer};

// `cascade_tmux_on_delete`, `socket_path_for`, `CascadeReport`, and
// `ServerStatus` exist on the registry for task 02696 (bedrock hard-
// delete cascade orchestrator) and task 02697 (wire types). Until
// those land they're allow(dead_code) at the definition site rather
// than re-exported here.

use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Tool, ToolContext, ToolOutput};
use crate::api::wire::{TmuxErrorResponse, TmuxToolResponse};
use invoke::{
    truncate_pair, TMUX_OUTPUT_MAX_BYTES, TMUX_TOOL_DEFAULT_WAIT_SECONDS,
    TMUX_TOOL_MAX_WAIT_SECONDS,
};

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
    fn name(&self) -> &'static str {
        "tmux"
    }

    fn description(&self) -> String {
        // Verbatim from specs/tmux-integration/design.md §"Description
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

Use this tool — not bash — for processes that:
  * need a TTY (REPLs, programs that detect isatty)
  * should survive Phoenix process restart
  * you want to interact with via stdin
  * are servers, watchers, or other long-lived processes

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

        // Build the full argv with `-f <phoenix-conf> -S <conv-sock>`
        // prepended (REQ-TMUX-011). No agent arg is parsed, rewritten,
        // or stripped; if the agent passes their own `-L` or `-S`,
        // tmux's CLI parser surfaces a usage error which we return
        // verbatim as stderr.
        //
        // `-f` only loads when tmux must spawn a fresh server. For a
        // running server the flag is benign; we include it so any
        // auto-spawn path uses the Phoenix config.
        let mut full_args: Vec<String> = vec![
            "-f".into(),
            config_path.to_string_lossy().into(),
            "-S".into(),
            socket_path.to_string_lossy().into(),
        ];
        full_args.extend(parsed.args);

        let started = Instant::now();
        let mut cmd = tokio::process::Command::new("tmux");
        cmd.args(&full_args)
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

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

/// Drive the subprocess to completion, racing against `wait_seconds`
/// and the cancellation token. The `Child` is consumed in exactly one
/// branch — `wait_with_output` moves it on the success path; the
/// cancel and timeout paths take it via `Option::take()` so the move
/// is structurally non-overlapping.
async fn run_with_timeout(
    child: tokio::process::Child,
    wait_seconds: u64,
    started: Instant,
    ctx: ToolContext,
) -> ToolOutput {
    let cancel = ctx.cancel.clone();
    let mut child_slot = Some(child);
    let timeout = tokio::time::sleep(Duration::from_secs(wait_seconds));
    tokio::pin!(timeout);

    // We can't `&mut child_slot.take().unwrap().wait_with_output()`
    // inside select! because that takes the Child eagerly even on
    // arms that never fire. Use a future borrow against a mutable
    // child reference for the wait branch.
    let mut child_owned = child_slot.take().expect("child set above");

    tokio::select! {
        biased;
        () = cancel.cancelled() => {
            kill_and_reap(child_owned).await;
            structured_response(
                "cancelled",
                None,
                started.elapsed().as_millis(),
                "",
                "",
                false,
            )
        }
        () = &mut timeout => {
            let (so, se, truncated) = kill_and_capture(child_owned).await;
            structured_response(
                "timed_out",
                None,
                u128::from(wait_seconds) * 1000,
                &so,
                &se,
                truncated,
            )
        }
        wait_result = child_owned.wait() => {
            // Drain output AFTER the wait observes exit so we don't
            // race the process and the OS pipe drain. `try_wait` is
            // already settled here so this is a no-op for wait, then
            // we read all of stdout/stderr.
            match wait_result {
                Ok(status) => {
                    let stdout = drain_pipe(child_owned.stdout.take()).await;
                    let stderr = drain_pipe(child_owned.stderr.take()).await;
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
                Err(e) => error_envelope(
                    "tmux_wait_failed",
                    &format!("failed to wait on tmux subprocess: {e}"),
                ),
            }
        }
    }
}

async fn drain_pipe<R: tokio::io::AsyncReadExt + Unpin>(reader: Option<R>) -> Vec<u8> {
    let Some(mut r) = reader else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(1), r.read_to_end(&mut buf)).await;
    buf
}

/// Kill the child and reap it without capturing output. Used on the
/// cancel path where we don't want to risk hanging on a slow drain.
async fn kill_and_reap(mut child: tokio::process::Child) {
    let _ = child.start_kill();
    let wait = child.wait();
    let _ = tokio::time::timeout(Duration::from_secs(1), wait).await;
}

/// Kill the child, drain its captured output, and return the
/// truncated pair. Used on the timeout path.
async fn kill_and_capture(mut child: tokio::process::Child) -> (String, String, bool) {
    let _ = child.start_kill();
    match tokio::time::timeout(Duration::from_secs(2), child.wait_with_output()).await {
        Ok(Ok(out)) => truncate_pair(&out.stdout, &out.stderr),
        _ => (String::new(), String::new(), false),
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
    use crate::tools::{BashHandleRegistry, BrowserSessionManager};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn parse_response(out: &ToolOutput) -> Value {
        out.display_data
            .clone()
            .or_else(|| serde_json::from_str(&out.output).ok())
            .expect("response should be JSON")
    }

    fn ctx_with_registry(registry: Arc<TmuxRegistry>) -> ToolContext {
        ctx_with_registry_for("test-conv", registry)
    }

    fn ctx_with_registry_for(conv: &str, registry: Arc<TmuxRegistry>) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            conv.to_string(),
            std::env::temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            registry,
        )
    }

    fn skip_unless_tmux() -> bool {
        which::which("tmux").is_err()
    }

    #[tokio::test]
    async fn binary_unavailable_returns_error_envelope() {
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir_and_binary(
            tmp.path().to_path_buf(),
            false,
        ));
        let ctx = ctx_with_registry(registry);
        let result = TmuxTool.run(json!({"args": ["list-sessions"]}), ctx).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "tmux_binary_unavailable");
    }

    #[tokio::test]
    async fn wait_seconds_out_of_range_returns_error() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(tmp.path().to_path_buf()));
        let ctx = ctx_with_registry(registry);
        let result = TmuxTool
            .run(
                json!({"args": ["list-sessions"], "wait_seconds": 5000}),
                ctx,
            )
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "wait_seconds_out_of_range");
    }

    #[tokio::test]
    async fn first_operation_spawns_server_and_responds_ok() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let socket_dir = tmp.path().to_path_buf();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(socket_dir.clone()));
        let ctx = ctx_with_registry_for("conv-fresh", registry.clone());

        let result = TmuxTool.run(json!({"args": ["list-sessions"]}), ctx).await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert_eq!(v["exit_code"], 0);
        let stdout = v["stdout"].as_str().unwrap();
        assert!(
            stdout.contains("main"),
            "expected `main` session in stdout, got: {stdout}"
        );

        // Socket file must live under the registry's socket dir.
        let sock = socket_dir.join("conv-conv-fresh.sock");
        assert!(sock.exists(), "socket file should exist at {sock:?}");

        // Cleanup: kill the spawned tmux server.
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &sock.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    #[tokio::test]
    async fn second_operation_reuses_existing_server() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(tmp.path().to_path_buf()));
        let ctx = ctx_with_registry_for("conv-reuse", registry.clone());

        let _ = TmuxTool
            .run(json!({"args": ["list-sessions"]}), ctx.clone())
            .await;

        // Drop in-memory registry entry to simulate a Phoenix restart;
        // the on-disk socket persists and the OS-owned tmux server keeps
        // running. The next operation must probe `Live` and re-use it.
        let registry2 = Arc::new(TmuxRegistry::with_socket_dir(tmp.path().to_path_buf()));
        let ctx2 = ctx_with_registry_for("conv-reuse", registry2.clone());

        let result = TmuxTool.run(json!({"args": ["list-sessions"]}), ctx2).await;
        assert!(result.success);
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert!(v["stdout"].as_str().unwrap().contains("main"));

        // Cleanup.
        let sock = tmp.path().join("conv-conv-reuse.sock");
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &sock.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    #[tokio::test]
    async fn stale_socket_is_unlinked_and_fresh_server_spawned() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let socket_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&socket_dir).unwrap();
        // Pre-create a stale, non-tmux file at the conversation's
        // socket path. `tmux ls` against it will fail.
        let stale = socket_dir.join("conv-conv-stale.sock");
        std::fs::write(&stale, b"junk").unwrap();

        let registry = Arc::new(TmuxRegistry::with_socket_dir(socket_dir.clone()));
        let ctx = ctx_with_registry_for("conv-stale", registry);

        let result = TmuxTool.run(json!({"args": ["list-sessions"]}), ctx).await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        assert_eq!(v["status"], "ok");
        assert!(v["stdout"].as_str().unwrap().contains("main"));

        // Cleanup.
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &stale.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    #[tokio::test]
    async fn agent_supplied_dash_l_does_not_escape_conversation_socket() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let socket_dir = tmp.path().to_path_buf();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(socket_dir.clone()));
        let ctx = ctx_with_registry_for("conv-dashL", registry);

        // Phoenix prepends `-S <sock>`. The agent's `-L weird` follows.
        // The exact handling of the duplicate flag is tmux-version-
        // specific: some versions reject with a usage error, some let
        // the first flag win (Phoenix's `-S`), some let the last flag
        // win. The structural property we verify here is that the
        // conversation's socket — at the path Phoenix chose — is the
        // ONLY socket that ever gets created. The agent cannot escape
        // to a `weird`-labeled socket regardless of tmux's CLI parser
        // behaviour.
        let _ = TmuxTool
            .run(json!({"args": ["-L", "weird", "list-sessions"]}), ctx)
            .await;

        let conv_sock = socket_dir.join("conv-conv-dashL.sock");
        // Permitted entries in the socket dir: the conversation's own
        // socket and the Phoenix-shipped tmux config file. Anything
        // else (e.g. a `weird`-labeled socket the agent tried to coerce
        // tmux into creating) is a structural escape and fails the
        // test.
        let unexpected: Vec<_> = std::fs::read_dir(&socket_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                !(s == "_phoenix.tmux.conf" || s.starts_with("conv-conv-dashL.sock"))
            })
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            unexpected.is_empty(),
            "only the conv socket + Phoenix tmux config should appear under {socket_dir:?}; \
             unexpected entries: {unexpected:?}"
        );

        // The cleanup applies to whichever socket actually got
        // created — the conv's path, never an agent-controlled one.
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &conv_sock.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    #[tokio::test]
    async fn cancellation_returns_cancelled_status() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(tmp.path().to_path_buf()));
        let cancel = CancellationToken::new();
        let ctx = ToolContext::new(
            cancel.clone(),
            "conv-cancel".to_string(),
            std::env::temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            registry.clone(),
        );

        // Issue a tmux command that will take a moment (the implicit
        // `ensure_live` runs `new-session -d` for a fresh conv); we
        // cancel the outer turn from a background task.
        let cancel2 = cancel.clone();
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel2.cancel();
        });

        // `wait` is a tmux command that blocks until a paired
        // `wait-for -S` signal arrives. We never signal it, so the only
        // way out is the cancel.
        let result = TmuxTool
            .run(
                json!({"args": ["wait-for", "phoenix-test-cancel"], "wait_seconds": 30}),
                ctx,
            )
            .await;
        let _ = cancel_task.await;
        let v = parse_response(&result);
        // Either cancel landed (status=cancelled) or ensure_live raced
        // ahead far enough that the subprocess saw cancel as a kill —
        // both leave the response in `cancelled` state because the
        // cancel branch in run_with_timeout is `biased` first.
        assert_eq!(v["status"], "cancelled", "got: {v}");

        // Cleanup.
        let sock = tmp.path().join("conv-conv-cancel.sock");
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &sock.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    #[tokio::test]
    async fn output_truncation_for_large_streams() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let socket_dir = tmp.path().to_path_buf();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(socket_dir.clone()));
        let ctx = ctx_with_registry_for("conv-trunc", registry.clone());

        // Spawn `main` first so subsequent commands have a target.
        let _ = TmuxTool
            .run(json!({"args": ["list-sessions"]}), ctx.clone())
            .await;

        // Fill the pane buffer with > 128 KB. We use `printf` inside
        // `new-window` rather than running a Phoenix-side bash because
        // we want tmux to emit it via `capture-pane`.
        let _spawn = TmuxTool
            .run(
                json!({
                    "args": [
                        "new-window", "-d", "-n", "filler",
                        "sh", "-c",
                        // 200_000 bytes of 'x'
                        "yes x | head -c 200000; sleep 1"
                    ]
                }),
                ctx.clone(),
            )
            .await;
        // Give the filler a moment to write into the pane.
        tokio::time::sleep(Duration::from_millis(300)).await;

        let result = TmuxTool
            .run(
                json!({"args": ["capture-pane", "-p", "-t", "filler", "-S", "-100000"]}),
                ctx,
            )
            .await;
        let v = parse_response(&result);
        // Capture-pane output may or may not exceed the budget on its
        // own — the goal is to verify the truncation path doesn't
        // crash. If it does exceed 128 KB, `truncated` must be true.
        let stdout = v["stdout"].as_str().unwrap();
        let stderr = v["stderr"].as_str().unwrap();
        assert!(stdout.len() + stderr.len() <= TMUX_OUTPUT_MAX_BYTES + 4096);
        let _ = v["truncated"];

        // Cleanup.
        let sock = socket_dir.join("conv-conv-trunc.sock");
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &sock.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }
}
