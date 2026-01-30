//! Bash tool - executes shell commands
//!
//! REQ-BASH-001: Command Execution
//! REQ-BASH-002: Timeout Management
//! REQ-BASH-003: Background Execution
//! REQ-BASH-004: No TTY Attached
//! REQ-BASH-005: Tool Schema
//! REQ-BASH-006: Error Reporting
//! REQ-BASH-007: Subprocess Termination on Cancellation

use super::{Tool, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
#[allow(unused_imports)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
use nix::sys::signal::{killpg, Signal};
#[cfg(unix)]
use nix::unistd::Pid;

const MAX_OUTPUT_LENGTH: usize = 128 * 1024; // 128KB
const SNIP_SIZE: usize = 4 * 1024; // 4KB each end
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const SLOW_TIMEOUT: Duration = Duration::from_secs(15 * 60); // 15 minutes
#[allow(dead_code)] // For future background task implementation
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours


/// Execution mode for bash commands
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ExecutionMode {
    #[default]
    Default,
    Slow,
    Background,
}

#[derive(Debug, Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    mode: ExecutionMode,
}

/// Bash tool for command execution
pub struct BashTool {
    working_dir: PathBuf,
}

impl BashTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    async fn execute_foreground(
        &self,
        command: &str,
        mode: ExecutionMode,
        cancel: CancellationToken,
    ) -> ToolOutput {
        let timeout_duration = match mode {
            ExecutionMode::Default => DEFAULT_TIMEOUT,
            ExecutionMode::Slow => SLOW_TIMEOUT,
            ExecutionMode::Background => unreachable!(),
        };

        let mut cmd = Command::new("bash");
        cmd.args(["-c", command])
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Set up process group for proper termination
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                // Create new process group with this process as leader
                // This allows us to kill all descendants with kill(-pgid, sig)
                nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))
                    .ok();
                Ok(())
            });
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("Failed to spawn process: {e}")),
        };

        let pid = child.id();

        // Race between: command completion, timeout, and cancellation
        tokio::select! {
            biased;

            // Cancellation requested
            () = cancel.cancelled() => {
                Self::kill_process_group(pid);
                ToolOutput::error("[command cancelled]")
            }

            // Timeout fired
            () = tokio::time::sleep(timeout_duration) => {
                Self::kill_process_group(pid);
                ToolOutput::error(format!("[command timed out after {timeout_duration:?}]"))
            }

            // Command completed
            result = child.wait_with_output() => {
                match result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);

                        // Combine stdout and stderr
                        let combined = if !stderr.is_empty() && !stdout.is_empty() {
                            format!("{stdout}{stderr}")
                        } else if !stderr.is_empty() {
                            stderr.to_string()
                        } else {
                            stdout.to_string()
                        };

                        let formatted = Self::truncate_output(&combined);

                        if output.status.success() {
                            ToolOutput::success(formatted)
                        } else {
                            let exit_code = output.status.code().unwrap_or(-1);
                            ToolOutput::error(format!(
                                "[command failed: exit code {exit_code}]\n{formatted}"
                            ))
                        }
                    }
                    Err(e) => ToolOutput::error(format!("Command execution failed: {e}")),
                }
            }
        }
    }

    /// Kill a process group immediately with SIGKILL.
    #[cfg(unix)]
    fn kill_process_group(pid: Option<u32>) {
        let Some(pid) = pid else { return };
        let pgid = Pid::from_raw(pid.cast_signed());
        tracing::debug!(pgid = pid, "Sending SIGKILL to process group");
        let _ = killpg(pgid, Signal::SIGKILL);
    }

    #[cfg(not(unix))]
    fn kill_process_group(_pid: Option<u32>) {
        // No-op on non-Unix platforms
    }

    fn execute_background(&self, command: &str) -> ToolOutput {
        // Create output file for background process
        let output_file =
            std::env::temp_dir().join(format!("phoenix-bg-{}.log", uuid::Uuid::new_v4()));
        let output_path = output_file.clone();

        let file = match std::fs::File::create(&output_file) {
            Ok(f) => f,
            Err(e) => return ToolOutput::error(format!("Failed to create output file: {e}")),
        };

        // Wrap command to append completion status
        let wrapper_script = format!(
            r#"{{ {}; }} > "{}" 2>&1; echo "" >> "{}"; echo "[background process completed with exit code $?]" >> "{}""#,
            command,
            output_file.display(),
            output_file.display(),
            output_file.display()
        );

        let mut cmd = Command::new("bash");
        cmd.args(["-c", &wrapper_script])
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // Detach from parent process
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                // Create new session (detach from terminal)
                nix::unistd::setsid().ok();
                Ok(())
            });
        }

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id().unwrap_or(0);
                drop(file); // Close file handle

                ToolOutput::success(format!(
                    "<pid>{}</pid>\n<output_file>{}</output_file>\n<reminder>To stop: kill -9 -{}</reminder>",
                    pid,
                    output_path.display(),
                    pid
                ))
            }
            Err(e) => ToolOutput::error(format!("Failed to start background process: {e}")),
        }
    }

    fn truncate_output(output: &str) -> String {
        if output.len() <= MAX_OUTPUT_LENGTH {
            return output.to_string();
        }

        let start = &output[..SNIP_SIZE];
        let end = &output[output.len() - SNIP_SIZE..];

        format!(
            "[output truncated in middle: got {} bytes, max is {} bytes]\n{}\n\n[snip]\n\n{}",
            output.len(),
            MAX_OUTPUT_LENGTH,
            start,
            end
        )
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> String {
        format!(
            r#"Executes shell commands via bash -c, returning combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

With mode="background", returns immediately with output redirected to a file.
Use background for servers/demos that need to stay running.

Use mode="slow" for potentially slow commands: builds, downloads,
installs, tests, or any other substantive operation.

IMPORTANT: Keep commands concise. The command input must be less than 60k tokens.
For complex scripts, write them to a file first and then execute the file.

<pwd>{}</pwd>"#,
            self.working_dir.display()
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute via bash -c"
                },
                "mode": {
                    "type": "string",
                    "enum": ["default", "slow", "background"],
                    "description": "Execution mode: default (30s timeout), slow (15min timeout), background (detached)"
                }
            }
        })
    }

    async fn run(&self, input: Value, cancel: CancellationToken) -> ToolOutput {
        let input: BashInput = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutput::error(format!("Invalid input: {e}")),
        };

        if input.command.is_empty() {
            return ToolOutput::error("Command cannot be empty");
        }

        match input.mode {
            ExecutionMode::Background => self.execute_background(&input.command),
            mode => self.execute_foreground(&input.command, mode, cancel).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[tokio::test]
    async fn test_simple_command() {
        let tool = BashTool::new(temp_dir());
        let result = tool
            .run(json!({"command": "echo hello"}), CancellationToken::new())
            .await;
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_failed_command() {
        let tool = BashTool::new(temp_dir());
        let result = tool
            .run(json!({"command": "exit 1"}), CancellationToken::new())
            .await;
        assert!(!result.success);
        assert!(result.output.contains("exit code 1"));
    }

    #[tokio::test]
    async fn test_output_truncation() {
        let long_output = "x".repeat(200_000);
        let truncated = BashTool::truncate_output(&long_output);
        assert!(truncated.len() < 20_000);
        assert!(truncated.contains("[snip]"));
    }

    #[tokio::test]
    async fn test_slow_mode() {
        let tool = BashTool::new(temp_dir());
        let result = tool
            .run(
                json!({
                    "command": "echo slow",
                    "mode": "slow"
                }),
                CancellationToken::new(),
            )
            .await;
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_cancellation_kills_subprocess() {
        let tool = BashTool::new(temp_dir());
        let cancel = CancellationToken::new();

        // Start a long-running command
        let tool_future = tool.run(
            json!({"command": "sleep 1000"}),
            cancel.clone(),
        );

        // Cancel after a short delay
        let cancel_task = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        };

        // Run both concurrently
        let (result, _) = tokio::join!(tool_future, cancel_task);

        // Should be cancelled, not timeout
        assert!(!result.success);
        assert!(
            result.output.contains("cancelled"),
            "Expected 'cancelled' in output, got: {}",
            result.output
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_cancellation_kills_subprocess_tree() {
        use std::process::Command as StdCommand;

        let tool = BashTool::new(temp_dir());
        let cancel = CancellationToken::new();

        // Use a unique marker so we can search for it
        let marker = format!("phoenix_test_{}", std::process::id());

        // Command that spawns a subprocess: bash spawns another bash which runs sleep
        let cmd = format!(
            "bash -c 'echo {}; sleep 1000' & wait",
            marker
        );

        let tool_future = tool.run(json!({"command": cmd}), cancel.clone());

        // Give the subprocess tree time to start
        let cancel_task = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        };

        let (result, _) = tokio::join!(tool_future, cancel_task);
        assert!(result.output.contains("cancelled"));

        // Give processes time to be killed
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify no orphaned sleep processes with our marker
        let ps_output = StdCommand::new("pgrep")
            .args(["-f", &marker])
            .output()
            .expect("pgrep should work");

        assert!(
            ps_output.stdout.is_empty(),
            "Found orphaned process! pgrep output: {}",
            String::from_utf8_lossy(&ps_output.stdout)
        );
    }
}
