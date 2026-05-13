//! Bash tool — execute shell commands with handle-based persistence.
//!
//! The tool exposes four operations (REQ-BASH-001/002/003/010):
//!
//! - **run** (`op="run", cmd=...`): start a new shell command. Block up to
//!   `wait_seconds` for it to finish; if it does not, return a handle.
//! - **peek** (`op="peek", handle=<handle>`): snapshot the live ring or tombstone.
//! - **wait** (`op="wait", handle=<handle>`): block up to `wait_seconds` for the handle's
//!   process to exit. Returns the SAME handle id on re-timeout
//!   (REQ-BASH-003).
//! - **kill** (`op="kill", handle=<handle>`): send `TERM` (default) or `KILL` to the
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
pub mod types;

pub use reaper::{install_reaper, shutdown_kill_tree};
pub use registry::{BashHandleError, BashHandleRegistry, ConversationHandles};
pub use types::{BashOp, BashToolInput};

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
        // The negation-based framing (`does NOT detach` / `NOT a timeout` /
        // `is NEVER killed` / `EXACTLY ONCE` / `does not auto-escalate`) is
        // load-bearing — affirmative descriptions get pattern-matched into
        // the POSIX `fork(2)` / `timeout(1)` / `kill PID` priors, and
        // explicit negations override those priors. The cookbook block
        // surfaces the `wait_seconds=0` "give me a handle now" affordance
        // that would otherwise be buried inside the run paragraph. See
        // REQ-BASH-002 / REQ-BASH-010 rationale.
        r#"Executes shell commands via bash -c, capturing combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

Common patterns:
  Run synchronously:    op="run", cmd="...", wait_seconds=30
  Start in background:  op="run", cmd="...", wait_seconds=0   (returns a handle immediately)
  Inspect progress:     op="peek", handle="b-3"
  Wait for completion:  op="wait", handle="b-3", wait_seconds=60

Pick one operation via `op`:

  op="run"    Run a shell command. If it finishes within wait_seconds you
              get its full output and exit code — same as if you'd run it
              in a shell. If wait_seconds elapses first, the process keeps
              running and you receive a handle to peek/wait/kill later.
              op="run" does NOT detach: the handle is minted only when
              wait_seconds elapses; for short commands you'll just get the
              result. wait_seconds is NOT a process kill timeout: the
              process is NEVER killed when wait_seconds elapses; it keeps
              running and you receive a handle. Use op="kill" to actually
              terminate. Set wait_seconds=0 to start a process and get its
              handle back immediately without waiting for output. Pass an
              optional label=<string> to annotate the handle (echoed on
              every later response and visible in the cap-reached error).

  op="peek"   Return the current ring buffer state for a handle. Required:
              handle=<id>. Use lines=N for the last N lines, or since=K
              for lines after offset K. status="tombstoned" in the
              response means the handle's process has finished — the
              final_cause field tells you how (exited normally, or killed
              by signal). status="kill_pending_kernel" means the kill
              signal you sent was delivered but the process is in
              uninterruptible kernel sleep — peek again later; sending
              kill again with the same signal does NOT compound (signals
              don't queue that way), but you can escalate by sending
              op="kill" with signal=KILL.

  op="wait"   Block up to wait_seconds for an existing handle to exit.
              Required: handle=<id>. If wait_seconds elapses first, the
              SAME handle is returned with status="still_running" — never
              accumulate handles by repeated waits. If the handle has
              already finished, returns immediately with
              status="tombstoned".

  op="kill"   Terminate a handle. Required: handle=<id>. Default signal
              is TERM; signal=KILL for immediate. The signal is sent
              EXACTLY ONCE; this tool does not auto-escalate TERM to
              KILL after a grace period. If your TERM doesn't take effect
              within ~30 seconds, the response is
              status="kill_pending_kernel" and you decide whether to
              escalate by calling op="kill" again with signal=KILL.
              (Don't retry with signal=TERM: the kernel doesn't queue
              duplicate signals; the original TERM is still pending and
              a second TERM is a no-op.)

If you peek a handle and get error="handle_not_found", it likely means
Phoenix restarted between when you ran the command and now — bash
handles do NOT survive Phoenix process restart. For processes that need
to survive Phoenix restart, that need a TTY, that need stdin, or that
are interactive REPLs, use the tmux tool instead.

IMPORTANT: Keep commands concise. The cmd input must be < 60k tokens.
For complex scripts, write them to a file first and execute the file."#
            .to_string()
    }

    fn input_schema(&self) -> Value {
        // Single discriminator (`op`) plus per-op value fields. The
        // four-sibling-string pattern was retired alongside the `op=run`
        // rename: the LLM sees the current schema each turn and conforms
        // to it; pre-discriminator history is inert text from the model's
        // POV. `deny_unknown_fields` on the input deserializer makes any
        // caller still emitting the retired affordances (`mode`, `command`
        // alias, four-sibling op keys) surface as a structured parse error
        // rather than silent acceptance — see specs/bash/requirements.md
        // REQ-BASH-010 rationale (revision 3).
        json!({
            "type": "object",
            "description": "Set `op` to run|peek|wait|kill. For op=run set `cmd` (and optionally `label`); for op=peek|wait|kill set `handle`.",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["run", "peek", "wait", "kill"],
                    "description": "Operation to perform."
                },
                "cmd": {
                    "type": "string",
                    "description": "Shell command to execute via `bash -c` (op=run). The bash wrapper stays alive as the parent of the user command; signal info propagates either via `WIFSIGNALED` directly or via the 128+signum exit-code convention."
                },
                "handle": {
                    "type": "string",
                    "description": "Handle id (op=peek|wait|kill)."
                },
                "label": {
                    "type": "string",
                    "maxLength": 64,
                    "description": "Optional human-readable annotation for the spawned handle (op=run). Echoed on every response carrying the handle and on each entry of `live_handles[]` in the cap-reached error."
                },
                "wait_seconds": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 900,
                    "description": "How long this single tool call blocks before handing back a handle (default 30; op=run|wait). NOT a process kill timeout: the process is NEVER killed when wait_seconds elapses; it keeps running and you receive a handle. Use op=kill to actually terminate."
                },
                "signal": {
                    "type": "string",
                    "enum": ["TERM", "KILL"],
                    "description": "Signal to send (op=kill only); default TERM. Sent exactly once; no auto-escalation."
                },
                "lines": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Tail mode: return last N lines (default 200)."
                },
                "since": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Incremental mode: return lines after offset K. Mutually exclusive with lines."
                }
            },
            "required": ["op"]
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
            Arc::new(crate::tools::TmuxRegistry::new()),
            None,
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
            Arc::new(crate::tools::TmuxRegistry::new()),
            None,
        )
    }

    // -----------------------------------------------------------------
    // Happy paths (REQ-BASH-002, REQ-BASH-003 integration)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn run_exits_within_wait_seconds_returns_exited() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({"op": "run", "cmd": "echo hello", "wait_seconds": 5}),
                ctx(),
            )
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fast_exit_preserves_trailing_output_no_reader_race() {
        // Regression for the codex review: the waiter used to call
        // transition_to_terminal as soon as child.wait() resolved, but the
        // stdout reader task could still be holding kernel-buffered bytes
        // that hadn't yet been appended to the live ring. Once tombstoned,
        // the next reader append silently dropped those bytes.
        //
        // Payload: ~30 KB of "hello\n" lines + a unique trailing
        // unterminated marker. With the reader's 4 KB read buffer (see
        // read_pipe_to_ring), that's ~7 read iterations per invocation.
        // 50 iterations × 7 windows = 350 race chances.
        let tool = BashTool;
        for i in 0..50 {
            let registry = Arc::new(BashHandleRegistry::new());
            let c = ctx_with_registry(registry);
            let marker = format!("final-marker-{i}");
            let cmd = format!("yes hello | head -n 5000; printf '{marker}'");
            let r = tool
                .run(json!({"op": "run", "cmd": &cmd, "wait_seconds": 5}), c)
                .await;
            let v = parse_response(&r);
            assert_eq!(v["status"], "exited", "iter {i}: got {v}");
            let lines: Vec<String> = v["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["bytes"].as_str().unwrap_or("").to_string())
                .collect();
            assert!(
                lines.iter().any(|l| l.contains(&marker)),
                "iter {i}: response missing final unterminated marker; \
                 last 5 lines: {:?}",
                &lines[lines.len().saturating_sub(5)..]
            );
        }
    }

    #[tokio::test]
    async fn run_wait_seconds_elapses_returns_still_running_with_handle() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({"op": "run", "cmd": "sleep 10", "wait_seconds": 1}),
                ctx(),
            )
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

        let spawn = tool
            .run(
                json!({"op": "run", "cmd": "sleep 20", "wait_seconds": 1}),
                c.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        let r1 = tool
            .run(
                json!({"op": "wait", "handle": handle.clone(), "wait_seconds": 1}),
                c.clone(),
            )
            .await;
        let v1 = parse_response(&r1);
        assert_eq!(v1["status"], "still_running");
        assert_eq!(v1["handle"], handle);

        let r2 = tool
            .run(
                json!({"op": "wait", "handle": handle.clone(), "wait_seconds": 1}),
                c.clone(),
            )
            .await;
        let v2 = parse_response(&r2);
        assert_eq!(v2["status"], "still_running");
        assert_eq!(v2["handle"], handle);

        let _ = tool
            .run(json!({"op": "kill", "handle": handle, "signal": "KILL"}), c)
            .await;
    }

    #[tokio::test]
    async fn kill_term_takes_within_timeout_returns_tombstoned_killed_with_signal_15() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        let spawn = tool
            .run(
                json!({"op": "run", "cmd": "sleep 30", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        let kill = tool
            .run(
                json!({"op": "kill", "handle": handle.clone(), "signal": "TERM"}),
                c,
            )
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

        let spawn = tool
            .run(
                json!({
                    "op": "run",
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
            let p = tool
                .run(json!({"op": "peek", "handle": handle.clone()}), c.clone())
                .await;
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
                .run(
                    json!({"op": "kill", "handle": kill_handle, "signal": "TERM"}),
                    kill_ctx,
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        let kill_kill = tool
            .run(
                json!({"op": "kill", "handle": handle.clone(), "signal": "KILL"}),
                c.clone(),
            )
            .await;
        let v = parse_response(&kill_kill);
        assert_eq!(v["status"], "tombstoned", "got: {v}");
        assert_eq!(v["final_cause"], "killed");
        assert_eq!(v["signal_number"], 9);

        let _ = kill_task.await;
    }

    #[tokio::test]
    async fn kill_on_already_terminal_handle_returns_tombstoned_no_signal_sent() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        let spawn = tool
            .run(
                json!({"op": "run", "cmd": "true", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .expect("run with wait_seconds=0 returns a handle")
            .to_string();

        let _ = tool
            .run(
                json!({"op": "wait", "handle": handle.clone(), "wait_seconds": 5}),
                c.clone(),
            )
            .await;

        let kill = tool
            .run(
                json!({"op": "kill", "handle": handle.clone(), "signal": "TERM"}),
                c,
            )
            .await;
        let v = parse_response(&kill);
        assert_eq!(v["status"], "tombstoned");
        assert!(v.get("signal_sent").is_none() || v["signal_sent"] == Value::Null);
    }

    #[tokio::test]
    async fn external_kill_9_surfaces_signal_number_9() {
        // External SIGKILL hitting the user command's bash process surfaces
        // as `signal_number: 9` (REQ-BASH-006).
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        let unique = format!("phoenix_ext_marker_{}", std::process::id());
        let cmd = format!("while true; do sleep 1; done # {unique}");
        let spawn = tool
            .run(
                json!({"op": "run", "cmd": cmd, "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let v = parse_response(&spawn);
        let handle = v["handle"].as_str().unwrap().to_string();

        tokio::time::sleep(Duration::from_millis(300)).await;

        let pkill = std::process::Command::new("pkill")
            .args(["-KILL", "-f", &unique])
            .status()
            .expect("pkill should be available");
        assert!(
            pkill.success() || pkill.code() == Some(0) || pkill.code() == Some(1),
            "pkill exited with {pkill:?}"
        );

        let result = tool
            .run(
                json!({"op": "wait", "handle": handle.clone(), "wait_seconds": 5}),
                c,
            )
            .await;
        let v = parse_response(&result);
        assert_eq!(v["status"], "tombstoned", "got: {v}");
        assert_eq!(v["final_cause"], "killed");
        assert_eq!(v["signal_number"], 9);
    }

    #[tokio::test]
    async fn inner_process_signal_surfaces_via_128_plus_signum_convention() {
        // REQ-BASH-006: when bash itself stays alive and only its child
        // gets signal-killed, bash exits NORMALLY with code 128+signum.
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry.clone());

        let cmd = "sleep 8128 && echo ok";
        let spawn = tool
            .run(
                json!({"op": "run", "cmd": cmd, "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let v = parse_response(&spawn);
        let handle = v["handle"].as_str().unwrap().to_string();

        let bash_pid = {
            use crate::tools::bash::handle::HandleId;
            let conv = registry.get_or_create("test-conv").await;
            let h = conv
                .read()
                .await
                .get(&HandleId::new(handle.clone()))
                .expect("handle should be live");
            h.live_pid().await.expect("bash should be live")
        };

        let inner_pid = loop {
            let out = std::process::Command::new("pgrep")
                .args(["-P", &bash_pid.to_string()])
                .output()
                .expect("pgrep should be available");
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = stdout.lines().next() {
                if let Ok(p) = line.trim().parse::<u32>() {
                    break p;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let kill = std::process::Command::new("kill")
            .args(["-KILL", &inner_pid.to_string()])
            .status()
            .expect("kill should be available");
        assert!(kill.success(), "kill exited with {kill:?}");

        let result = tool
            .run(
                json!({"op": "wait", "handle": handle.clone(), "wait_seconds": 5}),
                c,
            )
            .await;
        let v = parse_response(&result);
        assert_eq!(v["status"], "tombstoned", "got: {v}");
        assert_eq!(v["final_cause"], "killed", "got: {v}");
        assert_eq!(v["signal_number"], 9, "got: {v}");
        assert_eq!(v["exit_code"], 137, "got: {v}");
    }

    #[tokio::test]
    async fn cap_rejection_returns_structured_live_handles_list() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::with_caps(ring::RING_BUFFER_BYTES, 2));
        let c = ctx_with_registry(registry);

        let r1 = tool
            .run(
                json!({"op": "run", "cmd": "sleep 30", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let h1 = parse_response(&r1)["handle"].as_str().unwrap().to_string();
        let r2 = tool
            .run(
                json!({"op": "run", "cmd": "sleep 30", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let h2 = parse_response(&r2)["handle"].as_str().unwrap().to_string();

        let r3 = tool
            .run(
                json!({"op": "run", "cmd": "echo nope", "wait_seconds": 0}),
                c.clone(),
            )
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

        for h in [h1, h2] {
            let _ = tool
                .run(
                    json!({"op": "kill", "handle": h, "signal": "KILL"}),
                    c.clone(),
                )
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
                json!({"op": "run", "cmd": "sleep 10", "wait_seconds": 0}),
                conv_a.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();

        let foreign = tool
            .run(json!({"op": "peek", "handle": handle.clone()}), conv_b)
            .await;
        assert!(!foreign.success);
        let v = parse_response(&foreign);
        assert_eq!(v["error"], "handle_not_found");
        assert_eq!(v["handle_id"], handle);
        assert!(v["hint"].as_str().unwrap().contains("tmux"));

        let _ = tool
            .run(
                json!({"op": "kill", "handle": handle, "signal": "KILL"}),
                conv_a,
            )
            .await;
    }

    // -----------------------------------------------------------------
    // Schema + tolerance (REQ-BASH-010, revision 3)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn wait_seconds_out_of_range_returns_error() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({"op": "run", "cmd": "echo hi", "wait_seconds": 1000}),
                ctx(),
            )
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "wait_seconds_out_of_range");
        assert_eq!(v["max_wait_seconds"], 900);
    }

    /// `since` alone (no `lines`) routes to incremental mode.
    #[tokio::test]
    async fn peek_with_since_only_routes_to_incremental_mode() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({"op": "peek", "handle": "b-nonexistent", "since": 5}),
                ctx(),
            )
            .await;
        let v = parse_response(&result);
        assert_eq!(v["error"], "handle_not_found");
    }

    /// REQ-BASH-010 (rev 3) surviving tolerance: `lines` + `since` both
    /// supplied — prefer `lines`, drop `since` silently with a debug log.
    #[tokio::test]
    async fn peek_with_lines_and_since_drops_since_silently() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);
        let spawn = tool
            .run(
                json!({"op": "run", "cmd": "echo hi; sleep 5", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let peek = tool
            .run(
                json!({"op": "peek", "handle": handle.clone(), "lines": 10, "since": 5}),
                c.clone(),
            )
            .await;
        assert!(peek.success, "got: {}", peek.output);
        let _ = tool
            .run(json!({"op": "kill", "handle": handle, "signal": "KILL"}), c)
            .await;
    }

    /// REQ-BASH-010 (rev 3) surviving tolerance: `since=0` is below the
    /// schema's advertised `minimum: 1` but current GPT models still emit
    /// it as a default-fill. Treat as absent.
    #[tokio::test]
    async fn peek_with_since_zero_is_treated_as_absent() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);
        let spawn = tool
            .run(
                json!({"op": "run", "cmd": "echo hi; sleep 5", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        let handle = parse_response(&spawn)["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let peek = tool
            .run(
                json!({"op": "peek", "handle": handle.clone(), "since": 0}),
                c.clone(),
            )
            .await;
        assert!(peek.success, "got: {}", peek.output);
        let _ = tool
            .run(json!({"op": "kill", "handle": handle, "signal": "KILL"}), c)
            .await;
    }

    #[tokio::test]
    async fn missing_op_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool.run(json!({}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
        assert!(v["recommended_action"].as_str().unwrap().contains("run"));
    }

    #[tokio::test]
    async fn unknown_op_value_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool
            .run(json!({"op": "frobnicate", "cmd": "echo hi"}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
    }

    /// REQ-BASH-010 (rev 3): `op="spawn"` is no longer accepted. The
    /// rename is hard with no alias.
    #[tokio::test]
    async fn op_spawn_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool
            .run(json!({"op": "spawn", "cmd": "echo hi"}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
    }

    /// REQ-BASH-010 (rev 3): the legacy four-sibling shape is retired.
    /// Top-level `peek` / `wait` / `kill` keys are unknown to the parser
    /// and `deny_unknown_fields` rejects them.
    #[tokio::test]
    async fn legacy_top_level_peek_key_returns_parse_error() {
        let tool = BashTool;
        let result = tool.run(json!({"peek": "b-3"}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
        assert!(v["error_message"]
            .as_str()
            .unwrap()
            .contains("unknown field"));
    }

    /// REQ-BASH-010 (rev 3): the `mode` shim is retired.
    #[tokio::test]
    async fn legacy_mode_field_returns_parse_error() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({"op": "run", "cmd": "echo hi", "mode": "default"}),
                ctx(),
            )
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
    }

    /// REQ-BASH-010 (rev 3): the `command` alias for `cmd` is retired.
    #[tokio::test]
    async fn legacy_command_alias_returns_parse_error() {
        let tool = BashTool;
        let result = tool
            .run(json!({"op": "run", "command": "echo hi"}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
    }

    /// REQ-BASH-010 (rev 3): bare `cmd` with no `op` is no longer accepted.
    #[tokio::test]
    async fn bare_cmd_without_op_returns_mutually_exclusive_modes() {
        let tool = BashTool;
        let result = tool.run(json!({"cmd": "echo hi"}), ctx()).await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "mutually_exclusive_modes");
    }

    // -----------------------------------------------------------------
    // Label round-trip (REQ-BASH-002 / REQ-BASH-010)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn label_round_trips_through_run_peek_wait_tombstone() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::new());
        let c = ctx_with_registry(registry);

        let run = tool
            .run(
                json!({
                    "op": "run",
                    "cmd": "sleep 1",
                    "wait_seconds": 0,
                    "label": "dev-server"
                }),
                c.clone(),
            )
            .await;
        let v = parse_response(&run);
        assert_eq!(v["status"], "still_running");
        assert_eq!(v["label"], "dev-server");
        let handle = v["handle"].as_str().unwrap().to_string();

        let peek = tool
            .run(json!({"op": "peek", "handle": handle.clone()}), c.clone())
            .await;
        assert_eq!(parse_response(&peek)["label"], "dev-server");

        let wait = tool
            .run(
                json!({"op": "wait", "handle": handle.clone(), "wait_seconds": 5}),
                c.clone(),
            )
            .await;
        let wv = parse_response(&wait);
        assert_eq!(wv["status"], "tombstoned");
        assert_eq!(wv["label"], "dev-server");

        let kill = tool
            .run(json!({"op": "kill", "handle": handle, "signal": "TERM"}), c)
            .await;
        assert_eq!(parse_response(&kill)["label"], "dev-server");
    }

    #[tokio::test]
    async fn label_appears_on_cap_reached_live_handles() {
        let tool = BashTool;
        let registry = Arc::new(BashHandleRegistry::with_caps(ring::RING_BUFFER_BYTES, 1));
        let c = ctx_with_registry(registry);

        let r1 = tool
            .run(
                json!({
                    "op": "run",
                    "cmd": "sleep 30",
                    "wait_seconds": 0,
                    "label": "first-job"
                }),
                c.clone(),
            )
            .await;
        let h1 = parse_response(&r1)["handle"].as_str().unwrap().to_string();

        let r2 = tool
            .run(
                json!({"op": "run", "cmd": "echo nope", "wait_seconds": 0}),
                c.clone(),
            )
            .await;
        assert!(!r2.success);
        let v = parse_response(&r2);
        assert_eq!(v["error"], "handle_cap_reached");
        let live = v["live_handles"].as_array().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0]["label"], "first-job");

        let _ = tool
            .run(json!({"op": "kill", "handle": h1, "signal": "KILL"}), c)
            .await;
    }

    #[tokio::test]
    async fn label_over_cap_returns_label_too_long() {
        let tool = BashTool;
        let oversized = "x".repeat(65);
        let result = tool
            .run(
                json!({
                    "op": "run",
                    "cmd": "echo hi",
                    "label": oversized
                }),
                ctx(),
            )
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "label_too_long");
        assert_eq!(v["max_label_length"], 64);
    }

    #[tokio::test]
    async fn empty_label_is_treated_as_absent() {
        let tool = BashTool;
        let result = tool
            .run(
                json!({
                    "op": "run",
                    "cmd": "echo hi",
                    "label": "",
                    "wait_seconds": 5
                }),
                ctx(),
            )
            .await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        assert!(v.get("label").is_none());
    }

    // -----------------------------------------------------------------
    // Safety check still runs before run (REQ-BASH-011).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_blocked_git_add() {
        let tool = BashTool;
        let result = tool
            .run(json!({"op": "run", "cmd": "git add -A"}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "command_safety_rejected");
        assert!(v["reason"].as_str().unwrap().contains("blind git add"));
    }

    #[tokio::test]
    async fn test_blocked_rm_rf_root() {
        let tool = BashTool;
        let result = tool
            .run(json!({"op": "run", "cmd": "rm -rf /"}), ctx())
            .await;
        assert!(!result.success);
        let v = parse_response(&result);
        assert_eq!(v["error"], "command_safety_rejected");
        assert!(v["reason"].as_str().unwrap().contains("critical data"));
    }

    #[tokio::test]
    async fn test_blocked_git_push_force() {
        let tool = BashTool;
        let result = tool
            .run(json!({"op": "run", "cmd": "git push --force"}), ctx())
            .await;
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
        let result = tool
            .run(json!({"op": "run", "cmd": "echo hello"}), ctx())
            .await;
        assert!(result.success, "got: {}", result.output);
        let v = parse_response(&result);
        assert!(matches!(
            v["status"].as_str().unwrap(),
            "exited" | "still_running"
        ));
    }

    #[tokio::test]
    async fn cancellation_during_run_yields_handle() {
        // Cancellation during the run wait window leaves the process alive
        // (we don't proactively kill on cancel — that's what kill is for).
        // The agent gets the handle back to act on later.
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
            Arc::new(crate::tools::TmuxRegistry::new()),
            None,
        );

        let tool_future = tool.run(
            json!({"op": "run", "cmd": "sleep 60", "wait_seconds": 30}),
            c.clone(),
        );
        let cancel_task = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(tool_future, cancel_task);
        let v = parse_response(&result);
        assert!(v["handle"].is_string());
        let h = v["handle"].as_str().unwrap().to_string();
        let _ = tool
            .run(json!({"op": "kill", "handle": h, "signal": "KILL"}), c)
            .await;
    }
}
