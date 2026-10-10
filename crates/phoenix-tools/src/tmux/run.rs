//! Pit-of-success helper for running inspectable shell commands in tmux.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::backend::TmuxBackend;
use super::invoke::{truncate_pair, TMUX_TOOL_MAX_WAIT_SECONDS};
use super::TmuxError;
use crate::{Tool, ToolContext, ToolOutput};

use super::parse_last_exit_marker;

const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TMUX_RUN_CAPTURE_START: &str = "-2000";

pub struct TmuxRunTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TmuxRunInput {
    cmd: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_keep_open_on_exit")]
    keep_open_on_exit: bool,
    #[serde(default)]
    readiness: Readiness,
}

fn default_keep_open_on_exit() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Readiness {
    ReturnImmediately {},
    WaitForText { text: String, timeout_seconds: u64 },
}

impl Default for Readiness {
    fn default() -> Self {
        Self::ReturnImmediately {}
    }
}

#[derive(Debug)]
struct CapturedOutput {
    stdout: String,
    stderr: String,
    truncated: bool,
}

#[derive(Debug)]
struct RunObservation {
    captured_output: CapturedOutput,
    exit_code: Option<i32>,
    readiness_seen: bool,
}

struct TmuxRunTarget {
    window_name: String,
    window_id: String,
}

#[async_trait]
impl Tool for TmuxRunTool {
    // clearable: re-queryable read — see specs/stale-tool-results (REQ-STR-002).
    fn clearable(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "tmux_run"
    }

    fn description(&self) -> String {
        "Run a shell command in this conversation's shared tmux surface. Use this for dev servers, watchers, REPLs, and commands the user may want to inspect later. Phoenix starts the command in the current project/worktree automatically. The command runs via bash -lc, prints a standardized exit-code marker, and the pane stays inspectable after exit by default. Use the returned window_id with the raw tmux tool for later capture-pane, send-keys, or kill-window operations.".to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["cmd"],
            "additionalProperties": false,
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "Shell command to run via bash -lc, e.g. ./dev.py up"
                },
                "name": {
                    "type": "string",
                    "description": "Optional tmux window name. If omitted, Phoenix derives a short stable name from the command."
                },
                "keep_open_on_exit": {
                    "type": "boolean",
                    "default": true,
                    "description": "Keep the pane inspectable after the command exits. Defaults to true."
                },
                "readiness": {
                    "description": "Optional readiness behavior. Defaults to return_immediately.",
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["mode"],
                            "additionalProperties": false,
                            "properties": {
                                "mode": { "const": "return_immediately" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["mode", "text", "timeout_seconds"],
                            "additionalProperties": false,
                            "properties": {
                                "mode": { "const": "wait_for_text" },
                                "text": {
                                    "type": "string",
                                    "minLength": 1,
                                    "description": "Non-empty text to wait for in the tmux pane output."
                                },
                                "timeout_seconds": {
                                    "type": "integer",
                                    "minimum": 1,
                                    "maximum": TMUX_TOOL_MAX_WAIT_SECONDS,
                                    "description": "Seconds to wait for the text to appear."
                                }
                            }
                        }
                    ]
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: TmuxRunInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => {
                return error_envelope("invalid_input", &format!("invalid tmux_run input: {e}"))
            }
        };

        let cmd = parsed.cmd.trim();
        if cmd.is_empty() {
            return error_envelope("empty_command", "cmd must be non-empty after trimming");
        }

        let readiness = match validate_readiness(parsed.readiness) {
            Ok(r) => r,
            Err(out) => return out,
        };
        let requested_name = match parsed.name {
            Some(name) => match normalize_window_name(&name) {
                Ok(n) => n,
                Err(out) => return out,
            },
            None => derived_window_name(cmd),
        };

        let cwd = effective_file_root(&ctx);
        let server = match ctx.tmux().await {
            Ok(server) => server,
            Err(TmuxError::BinaryUnavailable) => {
                return error_envelope(
                    "tmux_binary_unavailable",
                    "the tmux binary is not installed on this host",
                )
            }
            Err(e) => return error_envelope("tmux_server_unavailable", &e.to_string()),
        };
        let (config_path, socket_path) = {
            let server = server.read().await;
            (
                ctx.tmux_registry().config_path(),
                server.socket_path.clone(),
            )
        };
        let wait_for_readiness = matches!(readiness, ValidReadiness::WaitForText { .. });
        let backend = ctx.tmux_registry().backend();
        let target = match start_tmux_window(
            backend,
            &config_path,
            &socket_path,
            &cwd,
            &requested_name,
            cmd,
            parsed.keep_open_on_exit,
            wait_for_readiness && !parsed.keep_open_on_exit,
        )
        .await
        {
            Ok(name) => name,
            Err(out) => return out,
        };

        match readiness {
            ValidReadiness::ReturnImmediately => {
                return_immediately_response(backend, &config_path, &socket_path, &target, &cwd, cmd)
                    .await
            }
            ValidReadiness::WaitForText { text, timeout } => {
                wait_for_text_response(
                    &ctx,
                    backend,
                    &config_path,
                    &socket_path,
                    &target,
                    &cwd,
                    cmd,
                    &text,
                    timeout,
                    !parsed.keep_open_on_exit,
                )
                .await
            }
        }
    }
}

#[derive(Debug)]
enum ValidReadiness {
    ReturnImmediately,
    WaitForText { text: String, timeout: Duration },
}

#[allow(clippy::result_large_err)]
fn validate_readiness(readiness: Readiness) -> Result<ValidReadiness, ToolOutput> {
    match readiness {
        Readiness::ReturnImmediately {} => Ok(ValidReadiness::ReturnImmediately),
        Readiness::WaitForText {
            text,
            timeout_seconds,
        } => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                return Err(error_envelope(
                    "empty_readiness_text",
                    "readiness.text must be non-empty after trimming",
                ));
            }
            if timeout_seconds == 0 || timeout_seconds > TMUX_TOOL_MAX_WAIT_SECONDS {
                return Err(error_envelope(
                    "readiness_timeout_out_of_range",
                    &format!(
                        "readiness.timeout_seconds must be in 1..={TMUX_TOOL_MAX_WAIT_SECONDS}; got {timeout_seconds}"
                    ),
                ));
            }
            Ok(ValidReadiness::WaitForText {
                text: trimmed,
                timeout: Duration::from_secs(timeout_seconds),
            })
        }
    }
}

fn effective_file_root(ctx: &ToolContext) -> PathBuf {
    let path = ctx
        .worktree_path
        .as_deref()
        .unwrap_or_else(|| ctx.working_dir())
        .to_path_buf();
    path.canonicalize().unwrap_or(path)
}

fn new_window_args(
    cwd: &Path,
    requested_name: &str,
    cmd: &str,
    keep_open_on_exit: bool,
    preserve_for_readiness: bool,
) -> Vec<String> {
    let wrapper = shell_wrapper(cmd, keep_open_on_exit, preserve_for_readiness);
    let shell_command = format!("bash -lc {}", shell_quote(&wrapper));
    vec![
        "new-window".to_string(),
        "-d".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{window_id}|#{window_name}".to_string(),
        "-t".to_string(),
        "main".to_string(),
        "-n".to_string(),
        requested_name.to_string(),
        "-c".to_string(),
        cwd.to_string_lossy().into_owned(),
        shell_command,
    ]
}

#[allow(clippy::too_many_arguments)]
async fn start_tmux_window(
    backend: &dyn TmuxBackend,
    config_path: &Path,
    socket_path: &Path,
    cwd: &Path,
    requested_name: &str,
    cmd: &str,
    keep_open_on_exit: bool,
    preserve_for_readiness: bool,
) -> Result<TmuxRunTarget, ToolOutput> {
    let args = new_window_args(
        cwd,
        requested_name,
        cmd,
        keep_open_on_exit,
        preserve_for_readiness,
    );
    let start_output = backend
        .run_cli(config_path, socket_path, &args)
        .await
        .map_err(|e| error_envelope("tmux_run_start_failed", &e))?;
    if !start_output.status.success() {
        let (stdout, stderr, truncated) = truncate_pair(&start_output.stdout, &start_output.stderr);
        return Err(structured_response(
            "start_failed",
            &TmuxRunTarget {
                window_name: requested_name.to_string(),
                window_id: requested_name.to_string(),
            },
            cwd,
            cmd,
            None,
            &CapturedOutput {
                stdout,
                stderr,
                truncated,
            },
            false,
        ));
    }

    let start_stdout = String::from_utf8_lossy(&start_output.stdout);
    let mut parts = start_stdout
        .lines()
        .next()
        .unwrap_or_default()
        .splitn(2, '|')
        .map(str::trim);
    let window_id = parts
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(requested_name)
        .to_string();
    let window_name = parts
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(requested_name)
        .to_string();
    Ok(TmuxRunTarget {
        window_name,
        window_id,
    })
}

async fn return_immediately_response(
    backend: &dyn TmuxBackend,
    config_path: &Path,
    socket_path: &Path,
    target: &TmuxRunTarget,
    cwd: &Path,
    cmd: &str,
) -> ToolOutput {
    let observation = observe_window(backend, config_path, socket_path, &target.window_id, None)
        .await
        .unwrap_or_else(|stderr| RunObservation {
            captured_output: CapturedOutput {
                stdout: String::new(),
                stderr,
                truncated: false,
            },
            exit_code: None,
            readiness_seen: false,
        });
    let status = if observation.exit_code.is_some() {
        "exited"
    } else {
        "started"
    };
    structured_response(
        status,
        target,
        cwd,
        cmd,
        observation.exit_code,
        &observation.captured_output,
        true,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionCleanup {
    KillWindow(String),
    RestoreExitCleanup(String),
}

fn completion_cleanup(
    close_after_completion: bool,
    exited: bool,
    window_id: &str,
) -> Option<CompletionCleanup> {
    if !close_after_completion {
        None
    } else if exited {
        Some(CompletionCleanup::KillWindow(window_id.to_string()))
    } else {
        Some(CompletionCleanup::RestoreExitCleanup(window_id.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitStatus {
    Ready,
    Exited,
    TimedOut,
    Pending,
}

fn wait_status(readiness_seen: bool, exited: bool, timed_out: bool) -> WaitStatus {
    if readiness_seen {
        WaitStatus::Ready
    } else if exited {
        WaitStatus::Exited
    } else if timed_out {
        WaitStatus::TimedOut
    } else {
        WaitStatus::Pending
    }
}

fn status_name(status: WaitStatus) -> Option<&'static str> {
    match status {
        WaitStatus::Ready => Some("ready"),
        WaitStatus::Exited => Some("exited"),
        WaitStatus::TimedOut => Some("readiness_timed_out"),
        WaitStatus::Pending => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_text_response(
    ctx: &ToolContext,
    backend: &dyn TmuxBackend,
    config_path: &Path,
    socket_path: &Path,
    target: &TmuxRunTarget,
    cwd: &Path,
    cmd: &str,
    text: &str,
    timeout: Duration,
    close_after_completion: bool,
) -> ToolOutput {
    let deadline = Instant::now() + timeout;
    loop {
        let observation = observe_window(
            backend,
            config_path,
            socket_path,
            &target.window_id,
            Some(text),
        )
        .await
        .unwrap_or_else(|stderr| RunObservation {
            captured_output: CapturedOutput {
                stdout: String::new(),
                stderr,
                truncated: false,
            },
            exit_code: None,
            readiness_seen: false,
        });
        let exited = observation.exit_code.is_some();
        let status = status_name(wait_status(
            observation.readiness_seen,
            exited,
            Instant::now() >= deadline,
        ));
        if let Some(status) = status {
            let response = structured_response(
                status,
                target,
                cwd,
                cmd,
                observation.exit_code,
                &observation.captured_output,
                true,
            );
            match completion_cleanup(close_after_completion, exited, &target.window_id) {
                Some(CompletionCleanup::KillWindow(window_id)) => {
                    let _ = kill_window(backend, config_path, socket_path, &window_id).await;
                }
                Some(CompletionCleanup::RestoreExitCleanup(window_id)) => {
                    match restore_exit_cleanup(backend, config_path, socket_path, &window_id).await
                    {
                        Ok(()) => {}
                        Err(error) => {
                            tracing::debug!(window_id = target.window_id, %error, "failed to restore tmux exit cleanup");
                        }
                    }
                }
                None => {}
            }
            return response;
        }
        tokio::select! {
            () = ctx.cancel.cancelled() => {
                let observation = observe_window(backend, config_path, socket_path, &target.window_id, None)
                    .await
                    .unwrap_or_else(|stderr| RunObservation {
                        captured_output: CapturedOutput {
                            stdout: String::new(),
                            stderr,
                            truncated: false,
                        },
                        exit_code: None,
                        readiness_seen: false,
                    });
                if observation.exit_code.is_none() && close_after_completion {
                    let _ = kill_window(backend, config_path, socket_path, &target.window_id).await;
                }
                return error_envelope("cancelled", "tmux_run cancelled while waiting for readiness");
            }
            () = tokio::time::sleep(READINESS_POLL_INTERVAL) => {}
        }
    }
}

#[allow(clippy::result_large_err)]
fn normalize_window_name(name: &str) -> Result<String, ToolOutput> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(error_envelope(
            "empty_window_name",
            "name must be non-empty after trimming",
        ));
    }
    if trimmed.contains(':')
        || trimmed.contains('|')
        || trimmed.contains('\n')
        || trimmed.contains('\r')
    {
        return Err(error_envelope(
            "invalid_window_name",
            "name must not contain ':', '|', newline, or carriage return",
        ));
    }
    Ok(trimmed.to_string())
}

fn derived_window_name(cmd: &str) -> String {
    let mut h = Sha256::new();
    h.update(cmd.as_bytes());
    let digest = h.finalize();
    let prefix = u32::from_be_bytes(digest[..4].try_into().expect("SHA-256 digest has 32 bytes"));
    format!("tmux-run-{prefix:08x}")
}

fn shell_wrapper(cmd: &str, keep_open_on_exit: bool, preserve_for_readiness: bool) -> String {
    let preserve = if preserve_for_readiness {
        "tmux set-option -w -t \"$TMUX_PANE\" remain-on-exit on; "
    } else {
        ""
    };
    let after_exit = if keep_open_on_exit {
        "exec ${SHELL:-/bin/bash} -i"
    } else {
        "exit $code"
    };
    let marker_cmd = "printf '%s000' \"$(date +%s)\"";
    format!(
        "{preserve}(\n{cmd}\n); code=$?; occurred_at_ms=$({marker_cmd}); echo; printf '%s\\n' \"__PHOENIX_EXIT__ exit_code=$code occurred_at_ms=$occurred_at_ms\"; {after_exit}"
    )
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

async fn restore_exit_cleanup(
    backend: &dyn TmuxBackend,
    config_path: &Path,
    socket_path: &Path,
    target: &str,
) -> Result<(), String> {
    let output = backend
        .run_cli(
            config_path,
            socket_path,
            &[
                "if-shell".to_string(),
                "-F".to_string(),
                "-t".to_string(),
                target.to_string(),
                "#{pane_dead}".to_string(),
                format!("kill-window -t {}", shell_quote(target)),
                format!(
                    "set-option -w -t {} remain-on-exit off",
                    shell_quote(target)
                ),
            ],
        )
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

async fn kill_window(
    backend: &dyn TmuxBackend,
    config_path: &Path,
    socket_path: &Path,
    target: &str,
) -> Result<(), String> {
    let output = backend
        .run_cli(
            config_path,
            socket_path,
            &[
                "kill-window".to_string(),
                "-t".to_string(),
                target.to_string(),
            ],
        )
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        let (_, stderr, _) = truncate_pair(&output.stdout, &output.stderr);
        Err(stderr)
    }
}

async fn observe_window(
    backend: &dyn TmuxBackend,
    config_path: &Path,
    socket_path: &Path,
    target: &str,
    readiness_text: Option<&str>,
) -> Result<RunObservation, String> {
    let output = backend
        .run_cli(
            config_path,
            socket_path,
            &[
                "capture-pane".to_string(),
                "-p".to_string(),
                "-t".to_string(),
                target.to_string(),
                "-S".to_string(),
                TMUX_RUN_CAPTURE_START.to_string(),
            ],
        )
        .await?;
    Ok(observation_from_bytes(
        &output.stdout,
        &output.stderr,
        readiness_text,
    ))
}

fn observation_from_bytes(
    stdout_bytes: &[u8],
    stderr_bytes: &[u8],
    readiness_text: Option<&str>,
) -> RunObservation {
    let raw_stdout = String::from_utf8_lossy(stdout_bytes);
    let raw_stderr = String::from_utf8_lossy(stderr_bytes);
    let marker = parse_last_exit_marker(&raw_stdout);
    let exit_code = marker.as_ref().map(|m| m.exit_code);
    let readiness_seen =
        readiness_text.is_some_and(|text| raw_stdout.contains(text) || raw_stderr.contains(text));
    let (stdout, stderr, truncated) = truncate_pair(stdout_bytes, stderr_bytes);
    RunObservation {
        captured_output: CapturedOutput {
            stdout,
            stderr,
            truncated,
        },
        exit_code,
        readiness_seen,
    }
}

fn structured_response(
    status: &str,
    target: &TmuxRunTarget,
    cwd: &Path,
    command: &str,
    exit_code: Option<i32>,
    captured_output: &CapturedOutput,
    success: bool,
) -> ToolOutput {
    let value = json!({
        "status": status,
        "window_name": target.window_name,
        "window_id": target.window_id,
        "cwd": cwd.to_string_lossy(),
        "command": command,
        "exit_code": exit_code,
        "captured_output": {
            "stdout": captured_output.stdout,
            "stderr": captured_output.stderr,
            "truncated": captured_output.truncated,
        }
    });
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    if success {
        ToolOutput::success(serialized).with_display(value)
    } else {
        ToolOutput::error(serialized).with_display(value)
    }
}

fn error_envelope(error_id: &str, message: &str) -> ToolOutput {
    let value = json!({
        "error": error_id,
        "message": message,
    });
    let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    ToolOutput::error(serialized).with_display(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::fake_backend::FakeTmuxBackend;
    use crate::{BashHandleRegistry, BrowserSessionManager, TmuxRegistry};
    use crate::{RegisterWakeInput, RegisteredWake, WakeRegistrar};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn parse_response(out: &ToolOutput) -> Value {
        out.display_data()
            .cloned()
            .or_else(|| serde_json::from_str(out.output()).ok())
            .expect("response should be JSON")
    }

    #[derive(Default)]
    struct MockWakeRegistrar {
        register_calls: AtomicUsize,
    }

    impl MockWakeRegistrar {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn register_calls(&self) -> usize {
            self.register_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl WakeRegistrar for MockWakeRegistrar {
        async fn register(&self, _input: RegisterWakeInput) -> Result<RegisteredWake, String> {
            self.register_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RegisteredWake::Registered {
                workflow_id: phoenix_workflow::WorkflowId(1),
            })
        }

        async fn cancel(&self, _input: crate::CancelWakeInput) -> Result<RegisteredWake, String> {
            Ok(RegisteredWake::CancelStale)
        }
    }

    fn ctx(
        conv: &str,
        working_dir: PathBuf,
        registry: Arc<TmuxRegistry>,
        worktree_path: Option<PathBuf>,
    ) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            conv.to_string(),
            working_dir,
            Arc::new(BrowserSessionManager::default()),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            registry,
            worktree_path,
            phoenix_core::work_scope::WorkScopeId::parse("test-work").unwrap(),
        )
    }

    fn ctx_with_registrar(
        conv: &str,
        working_dir: PathBuf,
        registry: Arc<TmuxRegistry>,
        worktree_path: Option<PathBuf>,
        registrar: Option<Arc<dyn WakeRegistrar>>,
    ) -> ToolContext {
        let mut ctx = ctx(conv, working_dir, registry, worktree_path)
            .with_root_conversation_id("root-tmux-wake".to_string())
            .with_tool_use_id("tool-tmux-wake");
        if let Some(registrar) = registrar {
            ctx = ctx.with_wake_registrar(Some(registrar));
        }
        ctx
    }

    struct FakeTmux {
        fake: Arc<FakeTmuxBackend>,
        registry: Arc<TmuxRegistry>,
        _socket_dir: TempDir,
    }

    impl FakeTmux {
        fn new() -> Self {
            let socket_dir = TempDir::new().unwrap();
            let fake = FakeTmuxBackend::new();
            let registry = Arc::new(TmuxRegistry::with_backend(
                socket_dir.path().to_path_buf(),
                fake.clone(),
                None,
            ));
            Self {
                fake,
                registry,
                _socket_dir: socket_dir,
            }
        }

        async fn socket_path(&self, ctx: &ToolContext) -> PathBuf {
            ctx.tmux().await.unwrap().read().await.socket_path.clone()
        }
    }

    async fn run_tool(ctx: &ToolContext, input: Value) -> Value {
        let result = TmuxRunTool.run(input, ctx.clone()).await;
        assert!(result.is_success(), "got: {}", result.output());
        parse_response(&result)
    }

    /// REQ-TMUX-014: the command starts in the worktree when one is bound,
    /// otherwise in the conversation working directory.
    #[tokio::test]
    async fn tmux_run_starts_in_worktree_when_bound_else_working_dir() {
        let tmux = FakeTmux::new();
        let direct_tmp = TempDir::new().unwrap();
        let worktree_tmp = TempDir::new().unwrap();
        let unrelated = TempDir::new().unwrap();
        let direct = direct_tmp.path().canonicalize().unwrap();
        let worktree = worktree_tmp.path().canonicalize().unwrap();

        let direct_ctx = ctx(
            "tmux-run-direct",
            direct.clone(),
            tmux.registry.clone(),
            None,
        );
        let direct_run = run_tool(&direct_ctx, json!({"cmd": "pwd", "name": "direct"})).await;
        let worktree_ctx = ctx(
            "tmux-run-worktree",
            unrelated.path().canonicalize().unwrap(),
            tmux.registry.clone(),
            Some(worktree.clone()),
        );
        let worktree_run = run_tool(&worktree_ctx, json!({"cmd": "pwd", "name": "worktree"})).await;

        assert_eq!(direct_run["cwd"], direct.to_string_lossy().as_ref());
        assert_eq!(worktree_run["cwd"], worktree.to_string_lossy().as_ref());
        let windows = tmux
            .fake
            .server(&tmux.socket_path(&direct_ctx).await)
            .unwrap()
            .windows;
        let cwd_of = |name: &str| {
            windows
                .iter()
                .find(|window| window.name == name)
                .map(|window| window.cwd.clone())
        };
        assert_eq!(cwd_of("direct"), Some(direct));
        assert_eq!(cwd_of("worktree"), Some(worktree));
    }

    /// REQ-TMUX-014: `wait_for_text` reports `ready` when the text appears
    /// and carries the exit code from Phoenix's exit marker.
    #[tokio::test]
    async fn tmux_run_wait_for_text_reports_ready_with_marker_exit_code() {
        let tmux = FakeTmux::new();
        tmux.fake.set_pane_output(
            "failing",
            "before-failure\n__PHOENIX_EXIT__ exit_code=7 occurred_at_ms=1700000000000\n",
        );
        let cwd = TempDir::new().unwrap();
        let run_ctx = ctx(
            "tmux-run-failure",
            cwd.path().to_path_buf(),
            tmux.registry.clone(),
            None,
        );

        let v = run_tool(
            &run_ctx,
            json!({
                "cmd": "echo before-failure; exit 7",
                "name": "failing",
                "readiness": {"mode": "wait_for_text", "text": "__PHOENIX_EXIT__", "timeout_seconds": 5}
            }),
        )
        .await;

        assert_eq!(v["status"], "ready");
        assert_eq!(v["exit_code"], 7);
        let stdout = v["captured_output"]["stdout"].as_str().unwrap();
        assert!(stdout.contains("before-failure"));
        assert_eq!(v["captured_output"]["truncated"], false);
        let window = &tmux
            .fake
            .server(&tmux.socket_path(&run_ctx).await)
            .unwrap()
            .windows[0];
        assert!(window.command.contains(&shell_quote(&shell_wrapper(
            "echo before-failure; exit 7",
            true,
            false
        ))));
    }

    /// REQ-TMUX-014: `return_immediately` reports `started` for a running
    /// command and never registers a wake.
    #[tokio::test]
    async fn tmux_run_return_immediately_reports_started_without_wake() {
        let tmux = FakeTmux::new();
        let cwd = TempDir::new().unwrap();
        let registrar = MockWakeRegistrar::new();
        let run_ctx = ctx_with_registrar(
            "tmux-run-immediate",
            cwd.path().to_path_buf(),
            tmux.registry.clone(),
            None,
            Some(registrar.clone()),
        );

        let result = TmuxRunTool
            .run(
                json!({"cmd": "echo immediate", "name": "immediate"}),
                run_ctx.clone(),
            )
            .await;

        assert!(result.is_success(), "got: {}", result.output());
        let v = parse_response(&result);
        assert_eq!(v["status"], "started");
        assert!(v.get("wake_registration").is_none());
        let provider: Value = serde_json::from_str(result.output()).unwrap();
        assert!(provider.get("wake_registration").is_none());
        assert_eq!(registrar.register_calls(), 0);
    }

    /// Explicit cleanup kills exactly the window `tmux_run` reported.
    #[tokio::test]
    async fn kill_window_removes_only_the_reported_window() {
        let tmux = FakeTmux::new();
        let cwd = TempDir::new().unwrap();
        let run_ctx = ctx(
            "tmux-run-cleanup",
            cwd.path().to_path_buf(),
            tmux.registry.clone(),
            None,
        );
        let first = run_tool(&run_ctx, json!({"cmd": "sleep 60", "name": "first"})).await;
        let second = run_tool(&run_ctx, json!({"cmd": "sleep 60", "name": "second"})).await;
        let socket_path = tmux.socket_path(&run_ctx).await;

        kill_window(
            tmux.registry.backend(),
            &tmux.registry.config_path(),
            &socket_path,
            first["window_id"].as_str().unwrap(),
        )
        .await
        .unwrap();

        let remaining = tmux.fake.server(&socket_path).unwrap().windows;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, second["window_id"].as_str().unwrap());
    }

    #[test]
    fn parse_exit_marker_matches_tmux_run_marker_line() {
        let output = "bash output\n__PHOENIX_EXIT__ exit_code=17 occurred_at_ms=1700000000000\n$ ";
        let marker = parse_last_exit_marker(output).expect("marker should parse");
        assert_eq!(marker.exit_code, 17);
    }

    #[test]
    fn readiness_only_preservation_uses_tmux_native_remain_on_exit() {
        let wrapper = shell_wrapper("echo READY; sleep 1", false, true);
        assert!(wrapper.starts_with("tmux set-option -w -t \"$TMUX_PANE\" remain-on-exit on; "));
        assert!(wrapper.ends_with("exit $code"));

        let ordinary = shell_wrapper("echo READY", false, false);
        assert!(!ordinary.contains("remain-on-exit"));

        let inspectable = shell_wrapper("echo READY", true, false);
        assert!(inspectable.ends_with("exec ${SHELL:-/bin/bash} -i"));
    }

    #[test]
    fn readiness_matching_uses_raw_output_before_truncation() {
        let mut stdout = vec![b'A'; 70_000];
        stdout.extend_from_slice(b"READY_IN_RAW_MIDDLE");
        stdout.extend(vec![b'B'; 70_000]);

        let observation = observation_from_bytes(&stdout, b"", Some("READY_IN_RAW_MIDDLE"));
        assert!(observation.readiness_seen);
        assert!(observation.captured_output.truncated);
        assert!(
            !observation
                .captured_output
                .stdout
                .contains("READY_IN_RAW_MIDDLE"),
            "test fixture should place readiness text outside the returned snippet"
        );
    }

    #[test]
    fn input_validation_rejects_pipe_and_empty_readiness_and_orphan_fields() {
        let error = normalize_window_name("api|watch").unwrap_err();
        assert_eq!(parse_response(&error)["error"], "invalid_window_name");
        let error = validate_readiness(Readiness::WaitForText {
            text: "   ".to_string(),
            timeout_seconds: 5,
        })
        .unwrap_err();
        assert_eq!(parse_response(&error)["error"], "empty_readiness_text");
        let error = serde_json::from_value::<TmuxRunInput>(json!({
            "cmd": "echo hi",
            "readiness": {"mode": "return_immediately", "timeout_seconds": 5}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn completed_wait_for_text_selects_exact_window_cleanup() {
        let window_id = "@42";
        assert_eq!(
            completion_cleanup(true, true, window_id),
            Some(CompletionCleanup::KillWindow("@42".to_string()))
        );
        assert_eq!(
            completion_cleanup(true, false, window_id),
            Some(CompletionCleanup::RestoreExitCleanup("@42".to_string()))
        );
        assert_eq!(completion_cleanup(false, true, window_id), None);
        assert_eq!(
            completion_cleanup(true, true, "@timeout-window"),
            Some(CompletionCleanup::KillWindow("@timeout-window".to_string()))
        );
        assert_eq!(
            status_name(wait_status(false, false, true)),
            Some("readiness_timed_out")
        );
    }

    #[test]
    fn production_new_window_argv_uses_main_and_immutable_cwd() {
        let cwd = Path::new("/project/worktree");
        let args = new_window_args(cwd, "smoke", "echo hi", false, false);
        assert_eq!(args[0], "new-window");
        assert_eq!(args[6], "main");
        assert_eq!(args[8], "smoke");
        assert_eq!(args[10], "/project/worktree");
        assert!(args[11].starts_with("bash -lc "));
        let wrapper = shell_wrapper("echo hi", false, false);
        assert!(args[11].contains(&shell_quote(&wrapper)));
    }
}
