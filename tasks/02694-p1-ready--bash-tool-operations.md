---
created: 2026-05-01
priority: p1
status: ready
artifact: src/tools/bash.rs
---

Implement the `BashTool` dispatch and the four agent-facing operations (`spawn`, `peek`, `wait`, `kill`) on top of the foundation from task 02693.

## In scope

- `src/tools/bash.rs` rewrite: `BashTool` struct, `Tool` trait impl, JSON schema (`oneOf` over `cmd` / `peek` / `wait` / `kill`), tool description with the negation-based `wait_seconds` framing.
- Spawn flow:
  - `bash_check::check(cmd)` pre-call; on reject, return `command_safety_rejected` without spawning.
  - Cap check via the registry; on reject, return `handle_cap_reached` with the live-handles list.
  - Spawn child as `bash -c "exec <cmd>"` with `pre_exec(setpgid(0, 0))`, stdin null, stdout/stderr piped, working-dir from `ctx.working_dir`. The `exec` wrapping is load-bearing for signal-info propagation (see REQ-BASH-006 rationale + design.md waiter task).
  - Reader tasks (stdout, stderr) feeding the ring; waiter task running `Child::wait()` and feeding the watch channel via `transition_to_terminal`.
  - `tokio::select!` race between `exit_observer.changed()`, `sleep(wait_seconds)`, and `ctx.cancel.cancelled()`.
- Peek operation: live snapshot read or tombstone serve; mutual-exclusion check on `lines` / `since`; `kill_pending_kernel` peek serves the live ring with the kill-attempt fields.
- Wait operation: same select! pattern as spawn but on an existing handle; SAME handle_id returned on re-timeout (no proliferation); tombstone fast-path for already-terminal handles.
- Kill operation: signal sent EXACTLY ONCE; `tokio::select!` race between exit observer and `KILL_RESPONSE_TIMEOUT_SECONDS` sleep; on timeout, transition to `kill_pending_kernel` (via the helper from task 02693) and respond; the waiter task survives so a late exit can demote `kill_pending_kernel → exited|killed`.
- Response shaping: `status: "exited" | "killed" | "still_running" | "kill_pending_kernel" | "tombstoned"` per the consolidated representation; tombstoned responses carry `final_cause` + `exit_code` + `signal_number`.
- Error envelope: stable IDs (`handle_not_found`, `handle_cap_reached`, `wait_seconds_out_of_range`, `peek_args_mutually_exclusive`, `command_safety_rejected`, `spawn_failed`, `mutually_exclusive_modes`); the dual-pass `mode + wait_seconds` case returns `mutually_exclusive_modes` with `conflicting_args` + `recommended_action` fields.
- `mode` deprecation alias: when supplied alone (no `wait_seconds`), map to the corresponding wait-seconds value and include a `deprecation_notice` field (no underscore prefix) in the response.
- `handle_not_found` hint: include the tmux-pointer message for handles that look like they predate the current process.

## Out of scope

- Wire types and UI rendering (task 02697 / Migration plumbing).
- Cascade integration on conversation hard-delete (task 02696). This task only touches the bash tool surface.

## Specs to read first

- `specs/bash/requirements.md`: REQ-BASH-001 through REQ-BASH-011, REQ-BASH-015.
- `specs/bash/design.md` sections "Tool Surface", "Spawn Flow", "Peek", "Wait", "Kill", "Error Envelope", "Output Capture and Display", "Command Safety Checks".
- `specs/bash/bash.allium`: spawn / peek / wait / kill rules and the response-shaping rules.

## Dependencies

- 02693 (Bash foundation) must land first — this task uses `BashHandleRegistry`, the lock-ordering helper, and `ToolContext::bash_handles()`.

## Done when

- `./dev.py check` passes.
- Integration tests cover:
  - Spawn → exits within `wait_seconds` → `status: "exited"` with exit code.
  - Spawn → wait_seconds elapses → `status: "still_running"` with handle.
  - Repeated `wait` on same handle returns the same handle id on each re-timeout.
  - `kill` with TERM that takes within timeout → `status: "tombstoned"`, `final_cause: "killed"`, `signal_number: 15`, no auto-escalation.
  - `kill` with TERM that doesn't take → `status: "kill_pending_kernel"`. Subsequent `kill` with `signal: KILL` escalates explicitly.
  - `kill` on already-terminal handle → `status: "tombstoned"`, no signal sent.
  - External `kill -9` (oom-killer simulation) → `status: "tombstoned"`, `final_cause: "killed"`, `signal_number: 9` reaches the response thanks to `bash -c "exec ..."`.
  - Cap rejection returns the structured live-handles list.
  - Cross-conversation handle access returns `handle_not_found`.
  - `mode + wait_seconds` returns `mutually_exclusive_modes` with `conflicting_args: ["mode", "wait_seconds"]`.
- All 42 existing safety-check unit tests still pass and the 4 integration tests verifying the check runs before spawn still pass.
