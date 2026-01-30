---
created: 2026-01-30
priority: p2
status: done
---

# Kill Bash Subprocesses on Cancellation

## Summary

When a bash tool is cancelled, ensure spawned subprocesses are also terminated.

## Context

The current cancellation implementation aborts the Rust task via CancellationToken, but if bash spawned a long-running subprocess (e.g., `sleep 1000` or a build command), that process continues running orphaned.

## Acceptance Criteria

- [x] Bash tool tracks child process PID
- [x] On cancellation, send SIGTERM to process group
- [x] If process doesn't exit within 1s, send SIGKILL
- [x] Add integration test with actual slow subprocess

## Implementation

Based on the approach from [github.com/scottopell/safe-shell](https://github.com/scottopell/safe-shell):

1. **Process group isolation**: Child calls `setpgid(0, 0)` via `pre_exec` to become its own process group leader
2. **CancellationToken plumbing**: Added `CancellationToken` parameter to `Tool::run()` trait method, threaded through `ToolExecutor` and `ToolRegistry`
3. **`tokio::select!`** in bash tool races between: command completion, timeout, and cancellation
4. **Graceful shutdown**: On cancel, sends SIGTERM to process group, waits 1s, then SIGKILL
5. **Tests**: Added `test_cancellation_kills_subprocess` and `test_cancellation_kills_subprocess_tree`

Key code in `src/tools/bash.rs`:
```rust
async fn kill_process_group(pid: Option<u32>, graceful: bool) {
    let Some(pid) = pid else { return };
    let pgid = Pid::from_raw(pid.cast_signed());

    if graceful {
        let _ = killpg(pgid, Signal::SIGTERM);
        tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT).await;
        let _ = killpg(pgid, Signal::SIGKILL);
    } else {
        let _ = killpg(pgid, Signal::SIGKILL);
    }
}
```

## Notes

This is important for CPU-intensive tools like builds or data processing that the user wants to abort immediately.
