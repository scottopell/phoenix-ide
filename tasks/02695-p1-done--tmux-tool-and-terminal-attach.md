---
created: 2026-05-01
priority: p1
status: done
artifact: src/tools/tmux.rs
---

Implement the `tmux` pass-through agent tool, its per-conversation server registry, and modify the in-app terminal spawn so it attaches to the conversation's tmux session when the binary is available.

## In scope

- New module subtree `src/tools/tmux/`:
  - `registry.rs` — `TmuxRegistry`, `TmuxServer`, `RwLock<TmuxServer>` per conversation, socket-path resolution to `~/.phoenix-ide/tmux-sockets/conv-<id>.sock`, lazy directory creation with 0700 perms.
  - `probe.rs` — `probe()` function returning `Live` / `DeadSocket` / `NoSocket` via `tmux -S <sock> ls`.
  - `invoke.rs` — `ensure_live()` composite (probe-and-act for the three `ProbeResult` cases) and `spawn_session()` (`tmux -S <sock> new-session -d -s main`).
- `src/tools/tmux.rs` — `TmuxTool` dispatch:
  - JSON schema: required `args` array, optional `wait_seconds` (1..=900).
  - Inject `-S <conv-sock>` as the FIRST args (before agent's args).
  - Subprocess invocation with `env_remove("TMUX")` to avoid outer-tmux nesting refusal.
  - `tokio::select!` race over cancel / `wait_seconds` sleep / `child.wait_with_output()`.
  - Response shape: `status` ∈ `ok` / `timed_out` / `cancelled`, separate `stdout` / `stderr` strings, `truncated` bool.
  - Output truncation: combined-budget middle-truncation per `TMUX_OUTPUT_MAX_BYTES` (default 128 KB).
  - `tmux_binary_unavailable` error when `which("tmux")` failed at registry init.
- `ToolContext::tmux()` accessor returning `Result<Arc<RwLock<TmuxServer>>, TmuxError>` matching the bash + browser pattern.
- `src/terminal/spawn.rs` modification: `build_pty_exec_argv` branches on `registry.binary_available()` — runs `tmux -S <sock> attach -t main` (with `env_remove("TMUX")`) when available, falls back to existing `$SHELL -i` spawn otherwise. The PTY-spawn pipeline (fork, setsid, TIOCSCTTY, dup2) is unchanged; only the argv differs.
- Existing single-attach-per-conversation constraint (REQ-TERM-003) preserved on both paths — multi-attach is deferred.

## Out of scope

- Cascade integration on conversation hard-delete (task 02696). The registry exposes a `cascade_tmux_on_delete` function for that task to call; the call site lives in the bedrock cascade orchestrator, not here.
- Wire types / UI rendering for the tmux response (task 02697 / Migration plumbing).
- Stale-recovery user notification (deferred per `TmuxStaleRecoveryNotification`); silent recreate is the v1 behaviour.

## Specs to read first

- `specs/tmux-integration/requirements.md` and `design.md` in full (~900 lines together — read them once before starting).
- `specs/tmux-integration/tmux-integration.allium` for the lifecycle rules and invariants.
- Skim `specs/terminal/terminal.allium` REQ-TERM-001 / REQ-TERM-003 for the existing PTY-spawn pipeline you're branching on.

## Dependencies

- 02693 (Bash foundation) only for the `ToolContext` extension pattern. The registry shape and accessor here mirror that. Task 02693 must land first; otherwise this task is independent of the bash work.

## Done when

- `./dev.py check` passes.
- Integration tests cover:
  - First operation on a fresh conversation → server spawned, `main` session created, response delivered.
  - Operation after Phoenix-restart simulation → probe sees Live, no spawn.
  - Operation with stale socket → probe sees DeadSocket, file unlinked, fresh server, no breadcrumb in pane.
  - Tool call with `-L` or `-S` in agent args → tmux usage error surfaces (Phoenix did NOT escape the conversation's socket).
  - Output truncation when subprocess emits >128 KB.
  - Cancellation mid-call → status="cancelled".
  - Terminal attach with tmux available → PTY runs `tmux attach`; terminal scrollback survives `./dev.py restart`.
  - Terminal attach without tmux → PTY runs `$SHELL -i`; existing single-attach 409 behaviour preserved.
- The conversation's tmux server is reachable via `tmux -S ~/.phoenix-ide/tmux-sockets/conv-<id>.sock ls` from outside Phoenix during a smoke test (kernel-socket isolation visible to the operator).
