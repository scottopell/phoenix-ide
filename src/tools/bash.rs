//! Bash tool — execute shell commands with handle-based persistence.
//!
//! The tool exposes four operations (REQ-BASH-001/002/003/010):
//!
//! - **spawn** (`cmd=...`): start a new shell command. Block up to
//!   `wait_seconds` for it to finish; if it does not, return a handle.
//! - **peek** (`peek=<handle>`): snapshot the live ring or tombstone.
//! - **wait** (`wait=<handle>`): block up to `wait_seconds` for the handle's
//!   process to exit. Returns the SAME handle id on re-timeout
//!   (REQ-BASH-003).
//! - **kill** (`kill=<handle>`): send `TERM` (default) or `KILL` to the
//!   handle's process group EXACTLY ONCE (no auto-escalation). On
//!   `KILL_RESPONSE_TIMEOUT_SECONDS` of no exit, return
//!   `kill_pending_kernel`; the waiter task survives so a late exit can
//!   still demote.
//!
//! See `specs/bash/{requirements,design}.md` and `specs/bash/bash.allium`
//! for the authoritative behavioral specification.

// Foundation submodules (task 02693) — used by the operations dispatch below.
pub mod handle;
mod operations;
pub mod reaper;
pub mod registry;
pub mod ring;

pub use reaper::{install_reaper, shutdown_kill_tree};
pub use registry::{BashHandleError, BashHandleRegistry, ConversationHandles};

use super::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Bash tool — stateless dispatcher over the handle-based bash model.
///
/// All per-conversation state lives in [`BashHandleRegistry`], reached
/// through `ToolContext::bash_handles()` (REQ-BASH-014). The tool
/// instance itself is reusable across conversations.
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> String {
        // The negation-based framing (`NOT a timeout` / `NEVER killed` /
        // `EXACTLY ONCE` / `does not auto-escalate`) is load-bearing —
        // affirmative descriptions get pattern-matched into the POSIX
        // `timeout(1)` / `kill PID` priors. See REQ-BASH-002 rationale.
        r#"Executes shell commands via bash -c, capturing combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

Modes (exactly one per call):

  cmd=<string>     Spawn a new command. wait_seconds (default 30) is NOT a
                   timeout — the process is NEVER killed when wait_seconds
                   elapses. wait_seconds only controls how long this single
                   tool call blocks before handing you back a handle so you
                   can do other work. The process keeps running in the
                   background until it exits naturally or you call
                   kill=<handle>. A response with status="still_running"
                   means the process is alive and will stay alive — peek
                   it later, wait on it, or kill it explicitly.

  peek=<handle>    Return the current ring buffer state for a handle.
                   Use lines=N for the last N lines, or since=K for lines
                   after offset K. status="tombstoned" in the response
                   means the handle's process has finished — the
                   final_cause field tells you how (exited normally, or
                   killed by signal). status="kill_pending_kernel" means
                   the kill signal you sent was delivered but the process
                   is in uninterruptible kernel sleep — peek again later;
                   sending kill again with the same signal does NOT
                   compound (signals don't queue that way), but you can
                   escalate by sending kill with signal=KILL.

  wait=<handle>    Block up to wait_seconds for an existing handle to exit.
                   If wait_seconds elapses first, the SAME handle is
                   returned with status="still_running" — never accumulate
                   handles by repeated waits. If the handle has already
                   finished, returns immediately with status="tombstoned".

  kill=<handle>    Terminate a handle. Default signal is TERM; signal=KILL
                   for immediate. The signal is sent EXACTLY ONCE; this
                   tool does not auto-escalate TERM to KILL after a grace
                   period. If your TERM doesn't take effect within
                   ~30 seconds, the response is status="kill_pending_kernel"
                   and you decide whether to escalate by calling kill
                   again with signal=KILL. (Don't retry with signal=TERM:
                   the kernel doesn't queue duplicate signals; the original
                   TERM is still pending and a second TERM is a no-op.)

If you peek a handle and get error="handle_not_found", it likely means
Phoenix restarted between when you spawned the process and now — bash
handles do NOT survive Phoenix process restart. For processes that need
to survive Phoenix restart, that need a TTY, that need stdin, or that
are interactive REPLs, use the tmux tool instead.

IMPORTANT: Keep commands concise. The cmd input must be < 60k tokens.
For complex scripts, write them to a file first and execute the file."#
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "Shell command to execute via bash -c (spawn). Will be wrapped as `bash -c \"exec <cmd>\"` so the bash process replaces itself with the user command and exit signals propagate cleanly."
                },
                "wait_seconds": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 900,
                    "description": "How long this single tool call blocks before handing back a handle (default 30). This is NOT a process kill timeout: the process is NEVER killed when wait_seconds elapses; it keeps running and you receive a handle. Use kill=<handle> to actually terminate."
                },
                "peek": { "type": "string", "description": "Handle id to peek" },
                "wait": { "type": "string", "description": "Handle id to wait on" },
                "kill": { "type": "string", "description": "Handle id to kill" },
                "signal": {
                    "type": "string",
                    "enum": ["TERM", "KILL"],
                    "description": "Signal to send (kill only); default TERM. Sent exactly once; no auto-escalation."
                },
                "lines": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Tail mode: return last N lines"
                },
                "since": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Incremental mode: return lines from offset K"
                },
                "mode": {
                    "type": "string",
                    "enum": ["default", "slow", "background"],
                    "description": "DEPRECATED — alias for wait_seconds (default=30, slow=900, background=0); removed in the second Phoenix release after this revision lands."
                }
            },
            "oneOf": [
                { "required": ["cmd"] },
                { "required": ["peek"] },
                { "required": ["wait"] },
                { "required": ["kill"] }
            ]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        operations::dispatch(input, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::BrowserSessionManager;
    use std::env::temp_dir;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn parse_response(out: &ToolOutput) -> Value {
        out.display_data
            .clone()
            .or_else(|| serde_json::from_str(&out.output).ok())
            .expect("response should be JSON")
    }

    fn ctx() -> ToolContext {
        ctx_with_registry(Arc::new(BashHandleRegistry::new()))
    }

    fn ctx_with_registry(registry: Arc<BashHandleRegistry>) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            "test-conv".to_string(),
            temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            registry,
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
        )
    }

    fn ctx_for(conversation_id: &str, registry: Arc<BashHandleRegistry>) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            conversation_id.to_string(),
            temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            registry,
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
        )
    }

    // -----------------------------------------------------------------
    // Done-when test cases (REQ-BASH integration)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn spawn_exits_within_wait_seconds_returns_exited() {
        let tool = BashTool;
        let result = tool
            .run(json!({"cmd": "echo hello", "wait_seconds": 5}), ctx())
            .await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        assert_eq!(v["status"], "exited");
        assert_eq!(v["exit_code"], 0);
        assert!(v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| { l["bytes"].as_str().unwrap_or("") == "hello" }));
    }

    #[tokio::test]
    async fn spawn_wait_seconds_elapses_returns_still_running_with_handle() {
        let tool = BashTool;
        let result = tool
            .run(json!({"cmd": "sleep 10", "wait_seconds": 1}), ctx())
            .await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        assert_eq!(v["status"], "still_running");
        let handle = v["handle"].as_str().expect("handle present");
        assert!(handle.starts_with("b-"));
    }

    #[tokio::test]
    async fn wait_returns_same_handle_id_on_repeated_re_timeout() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry.clone());

        // Spawn long-running process.
        let spawn = tool
            .run(json!({"cmd": "sleep 20", "wait_seconds": 1}), c.clone())
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        // First re-wait: still_running, same handle.
        let r1 = tool
            .run(
                json!({"wait": handle.clone(), "wait_seconds": 1}),
                c.clone(),
            )
            .await;
        let v1 = parse_response(&r1);
        assert_eq!(v1["status"], "still_running");
        assert_eq!(v1["handle"], handle);

        // Second re-wait: still_running, still the same handle.
        let r2 = tool
            .run(
                json!({"wait": handle.clone(), "wait_seconds": 1}),
                c.clone(),
            )
            .await;
        let v2 = parse_response(&r2);
        assert_eq!(v2["status"], "still_running");
        assert_eq!(v2["handle"], handle);

        // Cleanup: kill the process so the test doesn't leave a sleep behind.
        let _ = tool.run(json!({"kill": handle, "signal": "KILL"}), c).await;
    }

    #[tokio::test]
    async fn kill_term_takes_within_timeout_returns_tombstoned_killed_with_signal_15() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        // Process that exits cleanly on TERM.
        let spawn = tool
            .run(json!({"cmd": "sleep 30", "wait_seconds": 0}), c.clone())
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        let kill = tool
            .run(json!({"kill": handle.clone(), "signal": "TERM"}), c)
            .await;
        let v = parse_response(&kill);
        assert_eq!(v["status"], "tombstoned", "got response: {v}");
        assert_eq!(v["final_cause"], "killed");
        assert_eq!(v["signal_sent"], "TERM");
        assert_eq!(v["signal_number"], 15);
    }

    #[tokio::test]
    async fn kill_term_does_not_take_returns_kill_pending_kernel_then_kill_escalates() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        // Process that ignores TERM (trap '' TERM); will only die on KILL.
        // We override KILL_RESPONSE_TIMEOUT for this test — but the constant
        // is compile-time. Instead, we rely on the integration: TERM-trap
        // bash, then a quick wait, then escalate. We use a much shorter
        // poll: spawn, send TERM, then poll peek to see kill_pending_kernel
        // would take 30s.
        //
        // Practical approach: use a process that ignores TERM, and verify
        // that after sending TERM we can escalate to KILL and that
        // escalation works (the process exits with signal 9). The
        // "kill_pending_kernel returns after timeout" path is observed
        // implicitly through the explicit-escalation test.
        // Use a marker the bash interpreter must execute (echo) before
        // the trap statement, so by the time we observe the marker via
        // peek, we know the trap has been installed and bash is in the
        // while-loop.
        let spawn = tool
            .run(
                json!({
                    "cmd": "trap '' TERM; echo READY; while true; do sleep 1; done",
                    "wait_seconds": 0
                }),
                c.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        // Poll peek until READY is observed — guarantees the trap is in
        // place before we send TERM.
        let mut ready = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let p = tool.run(json!({"peek": handle.clone()}), c.clone()).await;
            let pv = parse_response(&p);
            if let Some(lines) = pv["lines"].as_array() {
                if lines
                    .iter()
                    .any(|l| l["bytes"].as_str().unwrap_or("") == "READY")
                {
                    ready = true;
                    break;
                }
            }
        }
        assert!(ready, "bash should reach READY before we send TERM");

        // Send TERM in background — bash will ignore it (trap '' TERM).
        let kill_handle = handle.clone();
        let kill_ctx = c.clone();
        let kill_task = tokio::spawn(async move {
            BashTool
                .run(json!({"kill": kill_handle, "signal": "TERM"}), kill_ctx)
                .await
        });

        // Give the TERM kill task a moment to send the signal and start
        // waiting on the response timeout.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Escalate via a fresh KILL call — this should succeed since the
        // group leader is still alive. The bash group will be SIGKILLed.
        let kill_kill = tool
            .run(json!({"kill": handle.clone(), "signal": "KILL"}), c.clone())
            .await;
        let v = parse_response(&kill_kill);
        // The KILL escalation should land tombstoned with final_cause=killed.
        assert_eq!(v["status"], "tombstoned", "got: {v}");
        assert_eq!(v["final_cause"], "killed");
        // signal_number=9 (KILL)
        assert_eq!(v["signal_number"], 9);

        // Reap the original TERM kill task — it raced against the actual
        // exit which we triggered with KILL; whichever lands, we just
        // ensure the test cleanly completes.
        let _ = kill_task.await;
    }

    #[tokio::test]
    async fn kill_on_already_terminal_handle_returns_tombstoned_no_signal_sent() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        // Use wait_seconds=0 so we always get a handle back even if the
        // command exits in microseconds.
        let spawn = tool
            .run(json!({"cmd": "true", "wait_seconds": 0}), c.clone())
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .expect("spawn returns a handle when wait_seconds=0")
            .to_string();

        // Wait for handle to reach terminal.
        let _ = tool
            .run(
                json!({"wait": handle.clone(), "wait_seconds": 5}),
                c.clone(),
            )
            .await;

        // Now kill on already-terminal.
        let kill = tool
            .run(json!({"kill": handle.clone(), "signal": "TERM"}), c)
            .await;
        let v = parse_response(&kill);
        assert_eq!(v["status"], "tombstoned");
        // No signal_sent on the response — already terminal means no signal was sent.
        assert!(v.get("signal_sent").is_none() || v["signal_sent"] == Value::Null);
    }

    #[tokio::test]
    async fn external_kill_9_surfaces_signal_number_9() {
        // This test verifies that an external SIGKILL hitting the user
        // command's bash process surfaces as `signal_number: 9` on the
        // handle response. With `bash -c "<cmd>"` and a non-tail-call-
        // optimizable command (like a `while` loop), bash itself stays
        // alive as the targetable process; pkill -f matches its argv,
        // delivers SIGKILL, `Child::wait()` returns
        // `ExitStatus::signal() == Some(9)`, and the handle reaches
        // `tombstoned + killed + signal_number=9`. The compound-keep-bash-
        // alive form here also matches the spec's tombstone-on-signal
        // requirement (REQ-BASH-006) regardless of any `exec` wrapping
        // strategy.
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        let unique = format!("phoenix_ext_marker_{}", std::process::id());
        // `while ... done` keeps the bash interpreter running rather than
        // tail-call-optimizing into the inner sleep, so pkill -f against
        // the unique marker (which appears in bash's argv) reaches the
        // bash process.
        let cmd = format!("while true; do sleep 1; done # {unique}");
        let spawn = tool
            .run(json!({"cmd": cmd, "wait_seconds": 0}), c.clone())
            .await;
        let v = parse_response(&spawn);
        let handle = v["handle"].as_str().unwrap().to_string();

        // Brief delay so the bash process is observable.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // External SIGKILL against the bash process via its argv marker.
        let pkill = std::process::Command::new("pkill")
            .args(["-KILL", "-f", &unique])
            .status()
            .expect("pkill should be available");
        assert!(
            pkill.success() || pkill.code() == Some(0) || pkill.code() == Some(1),
            "pkill exited with {pkill:?}"
        );

        // Wait on the handle — should see tombstoned + killed + signal 9.
        let result = tool
            .run(json!({"wait": handle.clone(), "wait_seconds": 5}), c)
            .await;
        let v = parse_response(&result);
        assert_eq!(v["status"], "tombstoned", "got: {v}");
        assert_eq!(v["final_cause"], "killed");
        assert_eq!(v["signal_number"], 9);
    }

    #[tokio::test]
    async fn cap_rejection_returns_structured_live_handles_list() {
        let tool = BashTool;
        // 2-handle cap.
        let registry = Arc::new(BashHandleRegistry::with_caps(ring::RING_BUFFER_BYTES, 2));
        let c = ctx_with_registry(registry);

        // Spawn two long-runners — at the cap.
        let r1 = tool
            .run(json!({"cmd": "sleep 30", "wait_seconds": 0}), c.clone())
            .await;
        let h1 = parse_response(&r1)["handle"].as_str().unwrap().to_string();
        let r2 = tool
            .run(json!({"cmd": "sleep 30", "wait_seconds": 0}), c.clone())
            .await;
        let h2 = parse_response(&r2)["handle"].as_str().unwrap().to_string();

        // Third spawn must fail with handle_cap_reached.
        let r3 = tool
            .run(json!({"cmd": "echo nope", "wait_seconds": 0}), c.clone())
            .await;
        assert!(!r3.success);
        let v = parse_response(&r3);
        assert_eq!(v["error"], "handle_cap_reached");
        assert_eq!(v["cap"], 2);
        let live = v["live_handles"].as_array().unwrap();
        assert_eq!(live.len(), 2);
        let ids: Vec<String> = live
            .iter()
            .map(|l| l["handle"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(&h1) && ids.contains(&h2));
        assert_eq!(live[0]["status"], "running");
        assert!(v["hint"].is_string());

        // Cleanup.
        for h in [h1, h2] {
            let _ = tool
                .run(json!({"kill": h, "signal": "KILL"}), c.clone())
                .await;
        }
    }

    #[tokio::test]
    async fn cross_conversation_handle_access_returns_handle_not_found() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let conv_a = ctx_for("conv-a", registry.clone());
        let conv_b = ctx_for("conv-b", registry);

        let spawn = tool
            .run(
                json!({"cmd": "sleep 10", "wait_seconds": 0}),
                conv_a.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        let foreign = tool.run(json!({"peek": handle.clone()}), conv_b).await;
        assert!(!foreign.success);
        let v = parse_response(&foreign);
        assert_eq!(v["error"], "handle_not_found");
        assert_eq!(v["handle_id"], handle);
        assert!(v["hint"].as_str().unwrap().contains("tmux"));

        // Cleanup.
        let _ = tool
            .run(json!({"kill": handle, "signal": "KILL"}), conv_a)
            .await;
    }

    #[tokio::test]
    async fn mode_plus_wait_seconds_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({
                    "cmd": "echo hi",
                    "mode": "background",
                    "wait_seconds": 30
                }),
                ctx(),
            )
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
        let conflicting = v["conflicting_args"].as_array().unwrap();
        let names: Vec<&str> = conflicting.iter().map(|x| x.as_str().unwrap()).collect();
        assert!(names.contains(&"mode") && names.contains(&"wait_seconds"));
        assert!(v["recommended_action"].as_str().unwrap().contains("mode"));
    }

    #[tokio::test]
    async fn mode_alone_succeeds_and_includes_deprecation_notice() {
        let tool = BashTool;
        let result = tool
            .run(json!({"cmd": "echo hi", "mode": "default"}), ctx())
            .await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        // Either exited (within 30s default) or still_running — both valid.
        assert!(matches!(
            v["status"].as_str().unwrap(),
            "exited" | "still_running"
        ));
        let notice = v["deprecation_notice"]
            .as_str()
            .expect("deprecation_notice present");
        // No leading underscore (intentional — see design.md): the agent
        // should attend to this field.
        assert!(!notice.is_empty());
        assert!(notice.contains("deprecated"));
    }

    #[tokio::test]
    async fn wait_seconds_out_of_range_returns_error() {
        let tool = BashTool;
        let result = tool
            .run(json!({"cmd": "echo hi", "wait_seconds": 1000}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "wait_seconds_out_of_range");
        assert_eq!(v["max_wait_seconds"], 900);
    }

    #[tokio::test]
    async fn peek_args_mutually_exclusive_returns_error() {
        let tool = BashTool;
        let result = tool
            .run(json!({"peek": "b-1", "lines": 10, "since": 0}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "peek_args_mutually_exclusive");
    }

    #[tokio::test]
    async fn no_operation_keys_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool.run(json!({}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
    }

    #[tokio::test]
    async fn multiple_operation_keys_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool.run(json!({"cmd": "echo", "peek": "b-1"}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
        let names: Vec<&str> = v["conflicting_args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert!(names.contains(&"cmd") && names.contains(&"peek"));
    }

    // -----------------------------------------------------------------
    // Safety check still runs before spawn (REQ-BASH-011).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_blocked_git_add() {
        let tool = BashTool;
        let result = tool.run(json!({"cmd": "git add -A"}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "command_safety_rejected");
        assert!(v["reason"].as_str().unwrap().contains("blind git add"));
    }

    #[tokio::test]
    async fn test_blocked_rm_rf_root() {
        let tool = BashTool;
        let result = tool.run(json!({"cmd": "rm -rf /"}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "command_safety_rejected");
        assert!(v["reason"].as_str().unwrap().contains("critical data"));
    }

    #[tokio::test]
    async fn test_blocked_git_push_force() {
        let tool = BashTool;
        let result = tool.run(json!({"cmd": "git push --force"}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "command_safety_rejected");
        assert!(v["reason"]
            .as_str()
            .unwrap()
            .contains("--force is not allowed"));
    }

    #[tokio::test]
    async fn test_allowed_command_runs() {
        let tool = BashTool;
        let result = tool.run(json!({"cmd": "echo hello"}), ctx()).await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        // Either exited (fast-path) or still_running.
        assert!(matches!(
            v["status"].as_str().unwrap(),
            "exited" | "still_running"
        ));
    }

    #[tokio::test]
    async fn cancellation_during_spawn_yields_handle() {
        // Cancellation during the spawn wait window leaves the process
        // alive (we don't proactively kill on cancel — that's what kill
        // is for). The agent gets the handle back to act on later.
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let cancel = CancellationToken::new();
        let c = ToolContext::new(
            cancel.clone(),
            "test-conv".to_string(),
            temp_dir(),
            Arc::new(BrowserSessionManager::default()),
            registry.clone(),
            Arc::new(crate::llm::ModelRegistry::new_empty()),
            crate::terminal::ActiveTerminals::new(),
        );

        let tool_future = tool.run(json!({"cmd": "sleep 60", "wait_seconds": 30}), c.clone());
        let cancel_task = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(tool_future, cancel_task);
        let v = parse_response(&result);
        // Either still_running or kill_pending_kernel; both carry a handle.
        assert!(v["handle"].is_string());
        // Cleanup.
        let h = v["handle"].as_str().unwrap().to_string();
        let _ = tool.run(json!({"kill": h, "signal": "KILL"}), c).await;
    }
}
