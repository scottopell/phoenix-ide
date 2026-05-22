//! Pit-of-success helper for running inspectable shell commands in tmux.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::invoke::{truncate_pair, TMUX_TOOL_MAX_WAIT_SECONDS};
use super::TmuxError;
use crate::tools::{Tool, ToolContext, ToolOutput};

const EXIT_MARKER_PREFIX: &str = "[phoenix] process exited with code ";
const TMUX_RUN_SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(10);
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
}

#[async_trait]
impl Tool for TmuxRunTool {
    fn name(&self) -> &'static str {
        "tmux_run"
    }

    fn description(&self) -> String {
        "Run a shell command in this conversation's shared tmux surface. Use this for dev servers, watchers, REPLs, and commands the user may want to inspect later. Phoenix starts the command in the current project/worktree automatically. The command runs via bash -lc, prints a standardized exit-code marker, and the pane stays inspectable after exit by default. Use the returned window_name with the raw tmux tool for later capture-pane, send-keys, or kill-window operations.".to_string()
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
        let (config_path, socket_path) = match resolve_tmux_paths(&ctx, &cwd).await {
            Ok(paths) => paths,
            Err(out) => return out,
        };
        let window_name = match start_tmux_window(
            &config_path,
            &socket_path,
            &cwd,
            &requested_name,
            cmd,
            parsed.keep_open_on_exit,
        )
        .await
        {
            Ok(name) => name,
            Err(out) => return out,
        };

        match readiness {
            ValidReadiness::ReturnImmediately => {
                return_immediately_response(&config_path, &socket_path, &window_name, &cwd, cmd)
                    .await
            }
            ValidReadiness::WaitForText { text, timeout } => {
                wait_for_text_response(
                    &ctx,
                    &config_path,
                    &socket_path,
                    &window_name,
                    &cwd,
                    cmd,
                    &text,
                    timeout,
                )
                .await
            }
        }
    }
}

enum ValidReadiness {
    ReturnImmediately,
    WaitForText { text: String, timeout: Duration },
}

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
        .as_ref()
        .unwrap_or(&ctx.working_dir)
        .clone();
    path.canonicalize().unwrap_or(path)
}

async fn resolve_tmux_paths(
    ctx: &ToolContext,
    cwd: &Path,
) -> Result<(PathBuf, PathBuf), ToolOutput> {
    let server_arc = match ctx
        .tmux_registry()
        .ensure_live(&ctx.conversation_id, ctx.worktree_path.as_deref(), cwd)
        .await
    {
        Ok(arc) => arc,
        Err(TmuxError::BinaryUnavailable) => {
            return Err(error_envelope(
                "tmux_binary_unavailable",
                "the tmux binary is not installed on this host",
            ));
        }
        Err(e) => return Err(error_envelope("tmux_server_unavailable", &e.to_string())),
    };
    let socket_path = {
        let server = server_arc.read().await;
        server.socket_path.clone()
    };
    Ok((ctx.tmux_registry().config_path(), socket_path))
}

async fn start_tmux_window(
    config_path: &Path,
    socket_path: &Path,
    cwd: &Path,
    requested_name: &str,
    cmd: &str,
    keep_open_on_exit: bool,
) -> Result<String, ToolOutput> {
    let wrapper = shell_wrapper(cmd, keep_open_on_exit);
    let shell_command = format!("bash -lc {}", shell_quote(&wrapper));
    let start_output = run_tmux_cli(
        config_path,
        socket_path,
        &[
            "new-window".to_string(),
            "-d".to_string(),
            "-P".to_string(),
            "-F".to_string(),
            "#{window_name}".to_string(),
            "-n".to_string(),
            requested_name.to_string(),
            "-c".to_string(),
            cwd.to_string_lossy().into_owned(),
            shell_command,
        ],
    )
    .await
    .map_err(|e| error_envelope("tmux_run_start_failed", &e))?;
    if !start_output.status.success() {
        let (stdout, stderr, truncated) = truncate_pair(&start_output.stdout, &start_output.stderr);
        return Err(structured_response(
            "start_failed",
            requested_name,
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

    Ok(String::from_utf8_lossy(&start_output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(requested_name)
        .to_string())
}

async fn return_immediately_response(
    config_path: &Path,
    socket_path: &Path,
    window_name: &str,
    cwd: &Path,
    cmd: &str,
) -> ToolOutput {
    let observation = observe_window(config_path, socket_path, window_name)
        .await
        .unwrap_or_else(|stderr| RunObservation {
            captured_output: CapturedOutput {
                stdout: String::new(),
                stderr,
                truncated: false,
            },
            exit_code: None,
        });
    let status = if observation.exit_code.is_some() {
        "exited"
    } else {
        "started"
    };
    structured_response(
        status,
        window_name,
        cwd,
        cmd,
        observation.exit_code,
        &observation.captured_output,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_text_response(
    ctx: &ToolContext,
    config_path: &Path,
    socket_path: &Path,
    window_name: &str,
    cwd: &Path,
    cmd: &str,
    text: &str,
    timeout: Duration,
) -> ToolOutput {
    let deadline = Instant::now() + timeout;
    loop {
        let observation = observe_window(config_path, socket_path, window_name)
            .await
            .unwrap_or_else(|stderr| RunObservation {
                captured_output: CapturedOutput {
                    stdout: String::new(),
                    stderr,
                    truncated: false,
                },
                exit_code: None,
            });
        let ready = observation.captured_output.stdout.contains(text)
            || observation.captured_output.stderr.contains(text);
        let exited = observation.exit_code.is_some();
        let status = if ready {
            Some("ready")
        } else if exited {
            Some("exited")
        } else if Instant::now() >= deadline {
            Some("readiness_timed_out")
        } else {
            None
        };
        if let Some(status) = status {
            return structured_response(
                status,
                window_name,
                cwd,
                cmd,
                observation.exit_code,
                &observation.captured_output,
                true,
            );
        }
        tokio::select! {
            () = ctx.cancel.cancelled() => {
                return error_envelope("cancelled", "tmux_run cancelled while waiting for readiness");
            }
            () = tokio::time::sleep(READINESS_POLL_INTERVAL) => {}
        }
    }
}

fn normalize_window_name(name: &str) -> Result<String, ToolOutput> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(error_envelope(
            "empty_window_name",
            "name must be non-empty after trimming",
        ));
    }
    if trimmed.contains(':') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(error_envelope(
            "invalid_window_name",
            "name must not contain ':', newline, or carriage return",
        ));
    }
    Ok(trimmed.to_string())
}

fn derived_window_name(cmd: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(cmd.as_bytes());
    let digest = h.finalize();
    let prefix = u32::from_be_bytes(digest[..4].try_into().expect("SHA-256 digest has 32 bytes"));
    format!("tmux-run-{prefix:08x}")
}

fn shell_wrapper(cmd: &str, keep_open_on_exit: bool) -> String {
    let after_exit = if keep_open_on_exit {
        "exec ${SHELL:-/bin/bash} -i"
    } else {
        "exit $code"
    };
    format!("({cmd}); code=$?; echo; echo \"{EXIT_MARKER_PREFIX}$code\"; {after_exit}")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

async fn run_tmux_cli(
    config_path: &Path,
    socket_path: &Path,
    args: &[String],
) -> Result<std::process::Output, String> {
    let mut full_args = vec![
        "-f".to_string(),
        config_path.to_string_lossy().into_owned(),
        "-S".to_string(),
        socket_path.to_string_lossy().into_owned(),
    ];
    full_args.extend(args.iter().cloned());

    let output = tokio::time::timeout(TMUX_RUN_SUBPROCESS_TIMEOUT, async move {
        tokio::process::Command::new("tmux")
            .args(&full_args)
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
    })
    .await
    .map_err(|_| "tmux subprocess timed out".to_string())?
    .map_err(|e| format!("failed to spawn tmux subprocess: {e}"))?;
    Ok(output)
}

async fn observe_window(
    config_path: &Path,
    socket_path: &Path,
    window_name: &str,
) -> Result<RunObservation, String> {
    let output = run_tmux_cli(
        config_path,
        socket_path,
        &[
            "capture-pane".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            window_name.to_string(),
            "-S".to_string(),
            "-2000".to_string(),
        ],
    )
    .await?;
    let (stdout, stderr, truncated) = truncate_pair(&output.stdout, &output.stderr);
    let exit_code = parse_exit_marker(&stdout);
    Ok(RunObservation {
        captured_output: CapturedOutput {
            stdout,
            stderr,
            truncated,
        },
        exit_code,
    })
}

fn parse_exit_marker(output: &str) -> Option<i32> {
    output.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix(EXIT_MARKER_PREFIX)
            .and_then(|code| code.parse::<i32>().ok())
    })
}

fn structured_response(
    status: &str,
    window_name: &str,
    cwd: &Path,
    command: &str,
    exit_code: Option<i32>,
    captured_output: &CapturedOutput,
    success: bool,
) -> ToolOutput {
    let value = json!({
        "status": status,
        "window_name": window_name,
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
    use crate::tools::tmux::registry::socket_path_for_worktree;
    use crate::tools::{BashHandleRegistry, BrowserSessionManager, TmuxRegistry};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn skip_unless_tmux() -> bool {
        which::which("tmux").is_err()
    }

    fn parse_response(out: &ToolOutput) -> Value {
        out.display_data()
            .cloned()
            .or_else(|| serde_json::from_str(out.output()).ok())
            .expect("response should be JSON")
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
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
            registry,
            worktree_path,
        )
    }

    async fn kill_socket(socket_path: &Path) {
        let _ = tokio::process::Command::new("tmux")
            .args(["-S", &socket_path.to_string_lossy(), "kill-server"])
            .env_remove("TMUX")
            .status()
            .await;
    }

    #[tokio::test]
    async fn direct_conversations_run_in_immutable_working_dir() {
        if skip_unless_tmux() {
            return;
        }
        let socket_tmp = TempDir::new().unwrap();
        let cwd_tmp = TempDir::new().unwrap();
        let cwd = cwd_tmp.path().canonicalize().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(
            socket_tmp.path().to_path_buf(),
        ));
        let ctx = ctx("tmux-run-direct-cwd", cwd.clone(), registry, None);

        let result = TmuxRunTool
            .run(
                json!({
                    "cmd": "pwd",
                    "name": "tmux-run-direct-cwd",
                    "readiness": {
                        "mode": "wait_for_text",
                        "text": EXIT_MARKER_PREFIX,
                        "timeout_seconds": 5
                    }
                }),
                ctx,
            )
            .await;
        assert!(result.is_success(), "got: {}", result.output());
        let v = parse_response(&result);
        assert_eq!(v["status"], "ready");
        assert_eq!(v["cwd"].as_str().unwrap(), cwd.to_string_lossy());
        assert!(v["captured_output"]["stdout"]
            .as_str()
            .unwrap()
            .contains(&cwd.to_string_lossy().to_string()));

        kill_socket(&socket_tmp.path().join("conv-tmux-run-direct-cwd.sock")).await;
    }

    #[tokio::test]
    async fn worktree_conversations_run_in_worktree_path() {
        if skip_unless_tmux() {
            return;
        }
        let socket_tmp = TempDir::new().unwrap();
        let unrelated_cwd = TempDir::new().unwrap();
        let worktree_tmp = TempDir::new().unwrap();
        let worktree = worktree_tmp.path().canonicalize().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(
            socket_tmp.path().to_path_buf(),
        ));
        let ctx = ctx(
            "tmux-run-worktree-cwd",
            unrelated_cwd.path().canonicalize().unwrap(),
            registry,
            Some(worktree.clone()),
        );

        let result = TmuxRunTool
            .run(
                json!({
                    "cmd": "pwd",
                    "name": "tmux-run-worktree-cwd",
                    "readiness": {
                        "mode": "wait_for_text",
                        "text": EXIT_MARKER_PREFIX,
                        "timeout_seconds": 5
                    }
                }),
                ctx,
            )
            .await;
        assert!(result.is_success(), "got: {}", result.output());
        let v = parse_response(&result);
        assert_eq!(v["cwd"].as_str().unwrap(), worktree.to_string_lossy());
        assert!(v["captured_output"]["stdout"]
            .as_str()
            .unwrap()
            .contains(&worktree.to_string_lossy().to_string()));

        let sock = socket_path_for_worktree(socket_tmp.path(), &worktree);
        kill_socket(&sock).await;
    }

    #[tokio::test]
    async fn quick_failure_leaves_inspectable_output_and_exit_marker() {
        if skip_unless_tmux() {
            return;
        }
        let socket_tmp = TempDir::new().unwrap();
        let cwd_tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir(
            socket_tmp.path().to_path_buf(),
        ));
        let ctx = ctx(
            "tmux-run-quick-failure",
            cwd_tmp.path().canonicalize().unwrap(),
            registry,
            None,
        );

        let result = TmuxRunTool
            .run(
                json!({
                    "cmd": "echo before-failure; exit 7",
                    "name": "tmux-run-quick-failure",
                    "readiness": {
                        "mode": "wait_for_text",
                        "text": EXIT_MARKER_PREFIX,
                        "timeout_seconds": 5
                    }
                }),
                ctx,
            )
            .await;
        assert!(result.is_success(), "got: {}", result.output());
        let v = parse_response(&result);
        assert_eq!(v["window_name"], "tmux-run-quick-failure");
        assert_eq!(v["exit_code"], 7);
        let pane = v["captured_output"]["stdout"].as_str().unwrap();
        assert!(pane.contains("before-failure"), "pane output: {pane}");
        assert!(
            pane.contains("[phoenix] process exited with code 7"),
            "pane output: {pane}"
        );
        assert_eq!(v["captured_output"]["truncated"], false);

        kill_socket(&socket_tmp.path().join("conv-tmux-run-quick-failure.sock")).await;
    }

    #[tokio::test]
    async fn readiness_rejects_empty_text_and_orphan_timeout() {
        let socket_tmp = TempDir::new().unwrap();
        let cwd_tmp = TempDir::new().unwrap();
        let registry = Arc::new(TmuxRegistry::with_socket_dir_and_binary(
            socket_tmp.path().to_path_buf(),
            false,
        ));
        let ctx = ctx(
            "tmux-run-readiness-invalid",
            cwd_tmp.path().to_path_buf(),
            registry,
            None,
        );

        let empty = TmuxRunTool
            .run(
                json!({
                    "cmd": "echo hi",
                    "readiness": {
                        "mode": "wait_for_text",
                        "text": "   ",
                        "timeout_seconds": 5
                    }
                }),
                ctx.clone(),
            )
            .await;
        let v = parse_response(&empty);
        assert_eq!(v["error"], "empty_readiness_text");

        let orphan = TmuxRunTool
            .run(
                json!({
                    "cmd": "echo hi",
                    "readiness": {
                        "mode": "return_immediately",
                        "timeout_seconds": 5
                    }
                }),
                ctx,
            )
            .await;
        let v = parse_response(&orphan);
        assert_eq!(v["error"], "invalid_input");
    }
}
