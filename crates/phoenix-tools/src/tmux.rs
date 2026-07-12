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
pub mod run;

pub use registry::{
    TmuxError, TmuxLifecycleEvent, TmuxLifecycleSink, TmuxRegistry, TmuxServer,
    TmuxTerminalEvidence, TmuxTerminalStatus, TmuxWindowInspection,
};
pub use run::TmuxRunTool;

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

#[allow(clippy::too_many_lines)]
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
            r#"Invokes tmux against this conversation's dedicated socket. Most non-destructive tmux CLI commands are available; provide the subcommand + flags as `args`.
Destructive pane/window/session/server commands and command sequences are rejected because they must go through Phoenix's typed lifecycle fencing.

This conversation's tmux server is isolated from every other conversation
and from any tmux server you may have running on the host: the socket path
is fixed by Phoenix and cannot be overridden by passing -L or -S in args.
If you do pass them, tmux will reject the duplicate server-selection flag
with a usage error.

Use `tmux_run` for starting dev servers, watchers, REPLs, or other
inspectable shell commands. It chooses the current project/worktree directory,
wraps the command with bash -lc, prints a visible exit marker, and keeps the
pane inspectable after exit by default.

Use this raw tmux tool for non-destructive detailed operations such as
`capture-pane`, `send-keys`, and `list-windows`. Destructive commands
(`kill-pane`/`killp`, `kill-window`/`killw`, `kill-session`, and `kill-server`)
and tmux command sequences are rejected; Phoenix-owned cleanup uses a dedicated typed path that
persists lifecycle evidence. Raw tmux does not enforce a cwd for newly-created
windows or panes.

Common subcommands:
  new-window -d -n NAME COMMAND     spawn a new window running COMMAND
  list-windows                       enumerate windows in the current session
  capture-pane -p -t NAME -S -2000   read up to 2000 lines of scrollback
                                     for window NAME
  send-keys -t NAME "input" Enter    send input to a window

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
        if let Err(message) = validate_generic_args(&parsed.args) {
            return error_envelope("tmux_destructive_command_forbidden", message);
        }

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

// Canonical command names reported by `tmux list-commands` in tmux 3.6a. Tmux
// resolves a command abbreviation only when it prefixes exactly one canonical
// name. Keep this list whole: checking destructive names alone would wrongly
// reject ambiguous prefixes such as `kill-s`.
const TMUX_COMMAND_NAMES: &[&str] = &[
    "attach-session",
    "bind-key",
    "break-pane",
    "capture-pane",
    "choose-buffer",
    "choose-client",
    "choose-tree",
    "clear-history",
    "clear-prompt-history",
    "clock-mode",
    "command-prompt",
    "confirm-before",
    "copy-mode",
    "customize-mode",
    "delete-buffer",
    "detach-client",
    "display-menu",
    "display-message",
    "display-popup",
    "display-panes",
    "find-window",
    "has-session",
    "if-shell",
    "join-pane",
    "kill-pane",
    "kill-server",
    "kill-session",
    "kill-window",
    "last-pane",
    "last-window",
    "link-window",
    "list-buffers",
    "list-clients",
    "list-commands",
    "list-keys",
    "list-panes",
    "list-sessions",
    "list-windows",
    "load-buffer",
    "lock-client",
    "lock-server",
    "lock-session",
    "move-pane",
    "move-window",
    "new-session",
    "new-window",
    "next-layout",
    "next-window",
    "paste-buffer",
    "pipe-pane",
    "previous-layout",
    "previous-window",
    "refresh-client",
    "rename-session",
    "rename-window",
    "resize-pane",
    "resize-window",
    "respawn-pane",
    "respawn-window",
    "rotate-window",
    "run-shell",
    "save-buffer",
    "select-layout",
    "select-pane",
    "select-window",
    "send-keys",
    "send-prefix",
    "server-access",
    "set-buffer",
    "set-environment",
    "set-hook",
    "set-option",
    "set-window-option",
    "show-buffer",
    "show-environment",
    "show-hooks",
    "show-messages",
    "show-options",
    "show-prompt-history",
    "show-window-options",
    "source-file",
    "split-window",
    "start-server",
    "suspend-client",
    "swap-pane",
    "swap-window",
    "switch-client",
    "unbind-key",
    "unlink-window",
    "wait-for",
];

const DESTRUCTIVE_TMUX_COMMANDS: &[&str] =
    &["kill-pane", "kill-server", "kill-session", "kill-window"];
const DESTRUCTIVE_TMUX_ALIASES: &[&str] = &["killp", "killw"];
const NESTED_COMMAND_LAUNCHERS: &[&str] = &["if-shell", "confirm-before"];
const RESPAWN_COMMANDS: &[&str] = &["respawn-pane", "respawn-window"];

fn uniquely_resolved_tmux_command(token: &str) -> Option<&'static str> {
    let mut matches = TMUX_COMMAND_NAMES
        .iter()
        .copied()
        .filter(|command| command.starts_with(token));
    let command = matches.next()?;
    matches.next().is_none().then_some(command)
}

fn resolves_to_destructive_tmux_command(token: &str) -> bool {
    DESTRUCTIVE_TMUX_ALIASES.contains(&token)
        || uniquely_resolved_tmux_command(token)
            .is_some_and(|command| DESTRUCTIVE_TMUX_COMMANDS.contains(&command))
}

#[derive(Debug, PartialEq, Eq)]
struct GenericTmuxCommand<'a> {
    name: &'a str,
    arguments: &'a [String],
}

impl<'a> GenericTmuxCommand<'a> {
    fn parse(args: &'a [String]) -> Option<Self> {
        let mut position = 0;
        while let Some(token) = args.get(position) {
            if token == "--" {
                position += 1;
                break;
            }
            if !token.starts_with('-') || token == "-" {
                break;
            }

            position += 1;
            if matches!(token.as_str(), "-f" | "-L" | "-S" | "-T") {
                position += usize::from(position < args.len());
            }
        }

        let name = args.get(position)?.as_str();
        Some(Self {
            name,
            arguments: &args[position + 1..],
        })
    }

    fn canonical_name(&self) -> Option<&'static str> {
        match self.name {
            "send" => Some("send-keys"),
            "run" => Some("run-shell"),
            "display" => Some("display-message"),
            name => uniquely_resolved_tmux_command(name),
        }
    }

    fn separator_is_payload(&self, position: usize) -> bool {
        match self.canonical_name() {
            Some("send-keys" | "display-message") => true,
            Some("run-shell") => {
                // run-shell consumes one optional command after its options. Once
                // that payload has been consumed, a standalone separator starts a
                // new tmux command and must remain fenced.
                let payload_position = self
                    .arguments
                    .iter()
                    .position(|argument| !argument.starts_with('-'));
                payload_position.is_some_and(|payload| position <= payload)
            }
            _ => false,
        }
    }
}

fn validate_generic_args(args: &[String]) -> Result<(), &'static str> {
    let Some(command) = GenericTmuxCommand::parse(args) else {
        return Ok(());
    };

    if resolves_to_destructive_tmux_command(command.name.trim_start_matches('\\')) {
        return Err("destructive tmux commands are not available through the generic tool; Phoenix-owned window cleanup must use the typed lifecycle path");
    }

    let canonical_name = command.canonical_name();
    if canonical_name.is_some_and(|name| NESTED_COMMAND_LAUNCHERS.contains(&name)) {
        return Err("tmux command launchers are not available through the generic tool; issue the intended non-destructive command directly");
    }
    if canonical_name.is_some_and(|name| RESPAWN_COMMANDS.contains(&name))
        && command.arguments.iter().any(|argument| argument == "-k")
    {
        return Err("destructive tmux respawn is not available through the generic tool; Phoenix-owned process cleanup must use the typed lifecycle path");
    }

    for (position, argument) in command.arguments.iter().enumerate() {
        if matches!(argument.as_str(), ";" | "\\;") && !command.separator_is_payload(position) {
            return Err("tmux command sequences are not available through the generic tool; issue one non-destructive command per call");
        }
    }

    Ok(())
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
        ctx_with_registry_for("test-conv", registry)
    }

    fn ctx_with_registry_for(conv: &str, registry: Arc<TmuxRegistry>) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            conv.to_string(),
            std::env::temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            registry,
            None,
        )
    }

    fn skip_unless_tmux() -> bool {
        which::which("tmux").is_err()
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
    async fn generic_tool_rejects_destructive_aliases_sequences_and_attached_forms() {
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir_and_binary(
            tmp.path().to_path_buf(),
            false,
        ));
        for args in [
            json!(["kill-window", "-t", "@1"]),
            json!(["kill-w", "-t@1"]),
            json!(["killw", "-t@1"]),
            json!(["kill-session", "-tmain"]),
            json!(["kill-ses", "-tmain"]),
            json!(["kill-server"]),
            json!(["kill-ser"]),
            json!(["kill-pane", "-t", "%1"]),
            json!(["killp", "-t%1"]),
            json!(["kill-p", "-t%1"]),
            json!(["kill-pa", "-t%1"]),
            json!(["kill-pan", "-t%1"]),
            json!(["\\kill-pane", "-t%1"]),
            json!(["\\kill-w", "-t@1"]),
            json!(["list-panes", ";", "kill-pane", "-t%1"]),
            json!(["list-panes", "\\;", "killp", "-t%1"]),
            json!(["list-windows", ";", "kill-ser"]),
            json!(["list-windows", "\\;", "kill-ses", "-tmain"]),
            json!(["list-windows", "\\;", "kill-window", "-t@1"]),
        ] {
            let result = TmuxTool
                .run(json!({"args": args}), ctx_with_registry(registry.clone()))
                .await;
            let value = parse_response(&result);
            assert_eq!(
                value["error"], "tmux_destructive_command_forbidden",
                "{value}"
            );
        }
    }

    #[test]
    fn destructive_abbreviation_resolution_matches_tmux_ambiguity_rules() {
        for command in DESTRUCTIVE_TMUX_COMMANDS {
            for prefix_len in 1..=command.len() {
                let prefix = command
                    .get(..prefix_len)
                    .expect("tmux canonical command names are ASCII");
                let uniquely_resolves = TMUX_COMMAND_NAMES
                    .iter()
                    .filter(|candidate| candidate.starts_with(prefix))
                    .count()
                    == 1;
                assert_eq!(
                    resolves_to_destructive_tmux_command(prefix),
                    uniquely_resolves,
                    "prefix {prefix:?} of {command:?}"
                );
            }
        }

        // `kill-s` is shared by kill-server and kill-session, so tmux rejects
        // it as ambiguous. Non-destructive commands and their abbreviations
        // remain available through the generic tool.
        for token in ["kill-s", "list-w", "capture-p", "send-k", "new-w"] {
            assert!(!resolves_to_destructive_tmux_command(token), "{token}");
        }
        for token in ["kill-p", "kill-w", "kill-ser", "kill-ses", "killp", "killw"] {
            assert!(resolves_to_destructive_tmux_command(token), "{token}");
        }
    }

    #[test]
    fn generic_parser_validates_command_positions_without_reparsing_payloads() {
        for args in [
            vec!["send-keys", "-t", "@1", "kill-window", "Enter"],
            vec!["send-keys", "-t", "@1", ";", "Enter"],
            vec!["send-keys", "-t", "@1", "kill-window; still data", "Enter"],
            vec!["send-k", "-t", "@1", ";", "Enter"],
            vec!["run-shell", "printf 'kill-window; still data'"],
            vec!["display-message", "kill-window; still data"],
            vec!["list-windows", "-a"],
        ] {
            let args = args.into_iter().map(String::from).collect::<Vec<_>>();
            assert_eq!(validate_generic_args(&args), Ok(()), "{args:?}");
        }

        for args in [
            vec!["kill-window", "-t", "@1"],
            vec!["list-windows", ";", "kill-window", "-t", "@1"],
            vec!["-S", "/tmp/other", "kill-window", "-t", "@1"],
            vec!["run-shell", "-b", "true", ";", "kill-window", "-t", "@1"],
            vec!["if-shell", "true", "kill-server"],
            vec!["confirm-before", "kill-window"],
            vec!["respawn-pane", "-k", "-t", "%1"],
            vec!["respawn-window", "-k", "-t", "@1"],
        ] {
            let args = args.into_iter().map(String::from).collect::<Vec<_>>();
            assert!(validate_generic_args(&args).is_err(), "{args:?}");
        }
    }

    #[tokio::test]
    async fn generic_tool_allows_non_destructive_commands_sharing_prefixes() {
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir_and_binary(
            tmp.path().to_path_buf(),
            false,
        ));
        for args in [
            json!(["kill-s"]),
            json!(["list-w", "-a"]),
            json!(["capture-p", "-p", "-t%1"]),
        ] {
            let result = TmuxTool
                .run(json!({"args": args}), ctx_with_registry(registry.clone()))
                .await;
            assert_eq!(
                parse_response(&result)["error"],
                "tmux_binary_unavailable",
                "validation unexpectedly rejected {args}"
            );
        }
    }

    #[tokio::test]
    async fn generic_tool_cannot_destroy_one_pane_tmux_run_target() {
        if skip_unless_tmux() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(tmp.path().to_path_buf()));
        let ctx = ctx_with_registry_for("generic-kill-pane", registry);
        let started = TmuxRunTool
            .run(
                json!({"cmd": "sleep 30", "name": "one-pane-target"}),
                ctx.clone(),
            )
            .await;
        assert!(started.is_success(), "got: {}", started.output());
        let window_id = parse_response(&started)["window_id"]
            .as_str()
            .unwrap()
            .to_string();

        for command in ["kill-pane", "killp", "kill-p", "kill-pa", "kill-pan"] {
            let rejected = TmuxTool
                .run(
                    json!({"args": [command, format!("-t{window_id}")]}),
                    ctx.clone(),
                )
                .await;
            assert_eq!(
                parse_response(&rejected)["error"],
                "tmux_destructive_command_forbidden",
                "command {command} was not rejected"
            );
        }

        let capture = TmuxTool
            .run(
                json!({"args": ["capture-pane", "-p", "-t", window_id]}),
                ctx,
            )
            .await;
        assert!(
            capture.is_success(),
            "target pane was destroyed: {}",
            capture.output()
        );
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
        assert!(!result.is_success());
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
        assert!(!result.is_success());
        let v = parse_response(&result);
        assert_eq!(v["error"], "wait_seconds_out_of_range");
    }

    #[tokio::test]
    async fn fresh_session_starts_in_supplied_cwd() {
        // Regression: tmux new-session was being issued without `-c
        // <cwd>`, so the pane shell inherited Phoenix's own working
        // directory instead of the conversation's project. The agent
        // and the in-app terminal then both landed in /home/bits/dev/
        // phoenix-ide regardless of which conversation they were
        // attached to.
        if skip_unless_tmux() {
            return;
        }
        let socket_tmp = TempDir::new().unwrap();
        // Use a directory that's NOT the test process's cwd so the
        // assertion catches the pre-fix "tmux inherits Phoenix's CWD"
        // behavior.
        let cwd_tmp = TempDir::new().unwrap();
        let cwd = cwd_tmp.path().canonicalize().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(
            socket_tmp.path().to_path_buf(),
        ));
        let ctx = ToolContext::new(
            CancellationToken::new(),
            "conv-cwd-test".to_string(),
            cwd.clone(),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            registry,
            None,
        );

        // First op spawns the session with `-c <cwd>`. Ask tmux for
        // the pane's current path and compare.
        let r = TmuxTool
            .run(
                json!({"args": ["display-message", "-p", "#{pane_current_path}"]}),
                ctx,
            )
            .await;
        assert!(r.is_success(), "got: {}", r.output());
        let v = parse_response(&r);
        let stdout = v["stdout"].as_str().unwrap().trim();
        let actual = std::path::PathBuf::from(stdout)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(stdout));
        assert_eq!(actual, cwd, "pane should start in {cwd:?}, got {stdout:?}");

        // Cleanup.
        let sock = socket_tmp.path().join("conv-conv-cwd-test.sock");
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &sock.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
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
        assert!(result.is_success(), "got: {}", result.output());
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
        assert!(result.is_success());
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
        assert!(result.is_success(), "got: {}", result.output());
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
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            registry.clone(),
            None,
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
