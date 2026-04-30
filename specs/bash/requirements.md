# Bash Tool

## User Story

As an LLM agent, I need to execute shell commands reliably so that I can interact
with the file system, run builds, manage processes, and accomplish user tasks. When
a command runs longer than I am willing to block on, I need to keep its output and
keep its process alive so I can pick the work back up later, rather than losing it.

## Background: from kill-on-timeout to job handles

Earlier versions of this tool exposed a `mode` enum (`default` / `slow` /
`background`). All three modes either killed the process when their timeout fired
or detached it and returned a PID + temp-file path. Both shapes have the same
underlying problem: a long-running command produces a binary outcome — wait the
whole way OR lose access — with no middle ground for the common case where the
agent wants to check progress, decide to wait some more, or move on.

This revision replaces that model with **job handles**. The agent specifies how
long it wants to block (`wait_seconds`); when that elapses, the process keeps
running and the agent receives a handle it can use to peek output, wait further,
or kill the process. The tool itself remains pipe-backed and non-interactive —
PTY needs and "I want this to survive Phoenix restart" needs are served by the
separate `tmux` tool (see `specs/tmux-integration/`).

## Requirements

### REQ-BASH-001: Command Execution

WHEN agent calls `bash(cmd=<command>, ...)`
THE SYSTEM SHALL execute the command via `bash -c` in the conversation's working
directory
AND capture combined stdout/stderr into a per-handle ring buffer (REQ-BASH-004)

WHEN the command exits, terminates by signal, or is killed by Phoenix
THE SYSTEM SHALL record exit_code (or signal information) and duration_ms

**Rationale:** The execution mechanism is unchanged from prior revisions — child
process via `tokio::process::Command`, group leader for clean cleanup. What
changes is what happens around it: output goes to a structured ring buffer, not
a single string return; exit observation is separate from "the agent's call
returned."

---

### REQ-BASH-002: Wait Semantics

WHEN agent calls `bash(cmd=<command>, wait_seconds=N)`
THE SYSTEM SHALL block up to N seconds for the command to exit

WHEN the command exits within N seconds
THE SYSTEM SHALL return `status: "exited"` with `exit_code`, `duration_ms`, and
the ring buffer contents (subject to the peek shape, REQ-BASH-004)

WHEN N seconds elapse before the command exits
THE SYSTEM SHALL return `status: "still_running"` with `handle`, `waited_ms`,
`end_offset`, and a tail of the ring buffer
AND keep the process running, accepting subsequent peek/wait/kill operations on
the handle

WHEN agent calls bash with `wait_seconds=0`
THE SYSTEM SHALL spawn the process and return immediately with `status:
"still_running"` and a handle, without waiting for any output

WHEN `wait_seconds` is omitted
THE SYSTEM SHALL apply a default of 30 seconds

WHEN `wait_seconds` exceeds `MAX_WAIT_SECONDS` (default 900)
THE SYSTEM SHALL reject the call with `error: "wait_seconds_out_of_range"`
AND state the bound in the error

**Rationale:** The renamed parameter (`wait_seconds`, replacing `timeout`)
removes the "kill" connotation that the old name carried. The hard distinction
between `status: "exited"` and `status: "still_running"` makes the
"timed-out-but-process-still-running" case unmistakable to the agent — pit of
success on the read side. The `MAX_WAIT_SECONDS` cap exists so the agent cannot
inadvertently park a request for hours: long-running operations should yield a
handle and resume via `wait` calls.

---

### REQ-BASH-003: Handle Operations (Peek, Wait, Kill)

WHEN agent calls `bash(peek=<handle>, ...)`
THE SYSTEM SHALL return the current state of the handle, including:
- `status` (`running` | `exited` | `killed` | `lost_in_restart`)
- `exit_code` (when status is `exited`; null otherwise)
- ring buffer contents per the offset/lines parameters (REQ-BASH-004)
- `tombstone: true` when the response is served from a tombstone record

WHEN agent calls `bash(wait=<handle>, wait_seconds=N)`
THE SYSTEM SHALL block up to N seconds for the handle's process to exit
AND return the same response shape as REQ-BASH-002 (`status: "exited"` on
completion, `status: "still_running"` on re-timeout)
AND on re-timeout, return the *same* handle (not a new one)

WHEN agent calls `bash(kill=<handle>, signal=<TERM|KILL>)`
THE SYSTEM SHALL send the specified signal (default `TERM`) to the process group
AND wait up to `KILL_GRACE_SECONDS` (default 5) for the process to exit
AND if the process has not exited after grace, escalate to `SIGKILL`
AND surface `signal_sent`, `signal_escalated` (when applicable), and final state
in the response

WHEN agent calls peek/wait/kill on a handle that does not exist in the live
table or tombstone store
THE SYSTEM SHALL return `error: "handle_not_found"`

**Rationale:** Three operations cover the lifecycle of a backgrounded handle.
The `kill` auto-escalation removes a footgun where `TERM` doesn't take and the
agent is left wondering whether to call again with `KILL` — the tool handles
the dance and reports what it did. Returning the same handle on `wait`
re-timeout (rather than minting a new one) is the pit-of-success choice: the
agent never accumulates handles across re-waits.

---

### REQ-BASH-004: Ring Buffer and Read Semantics

WHEN a handle's process produces output on stdout or stderr
THE SYSTEM SHALL append the bytes to a per-handle ring buffer bounded by
`RING_BUFFER_BYTES` (default 4 MB)
AND split incoming bytes on newline boundaries to assign each complete line a
monotonically increasing offset (line 0, 1, 2, ... since spawn)

WHEN the ring buffer reaches `RING_BUFFER_BYTES` and new content arrives
THE SYSTEM SHALL evict the oldest lines until the new content fits
AND advance `start_offset` to the offset of the oldest still-retained line

WHEN agent supplies `peek=<handle>` with `lines=N`
THE SYSTEM SHALL return the last N lines of the ring buffer (or all lines if
fewer than N exist)

WHEN agent supplies `peek=<handle>` with `since=K`
THE SYSTEM SHALL return lines with offset in the range [max(K, start_offset),
end_offset)

WHEN agent supplies `peek=<handle>` with both `lines` and `since`
THE SYSTEM SHALL reject the call with `error:
"peek_args_mutually_exclusive"`

WHEN agent supplies `peek=<handle>` with no read modifiers
THE SYSTEM SHALL return the last `DEFAULT_PEEK_LINES` (default 200) lines

WHEN any read returns and `K` was older than `start_offset` (incremental mode)
or eviction occurred since the agent's prior peek (tail mode)
THE SYSTEM SHALL set `truncated_before: true` in the response
AND otherwise set it to `false`

EVERY peek/wait/spawn response SHALL include `start_offset`, `end_offset`, and
`truncated_before` for the lines returned

**Rationale:** Caller-controlled offsets keep the server stateless on read
cursors — a dropped network response, a re-asking agent, or a UI peeker do not
race each other. `truncated_before` makes information loss explicit rather than
silent: the agent can detect when content fell out of the window and decide how
to respond.

---

### REQ-BASH-005: Live Handle Cap

WHEN agent calls `bash(cmd=<command>, ...)` AND the conversation has
`LIVE_HANDLE_CAP` (default 8) live handles (status `running`)
THE SYSTEM SHALL reject the call with:
- `error: "handle_cap_reached"`
- `cap`: the configured value
- `live_handles`: list of `{ handle, cmd, age_seconds, status }` for each live
  handle in the conversation
- `hint`: text directing the agent to kill or wait on a handle, or use the
  `tmux` tool for long-runners

WHEN a handle transitions out of `running` (exit, kill, restart)
THE SYSTEM SHALL decrement the live count
AND a subsequent spawn from the same conversation MAY succeed if it brings the
live count under the cap

**Rationale:** A hard refusal is the pit-of-success failure mode. LRU eviction
silently kills the very handle the agent was about to peek; soft warnings
permit unbounded accumulation. Refusing with an actionable list of live
handles tells the agent exactly what to do.

---

### REQ-BASH-006: Tombstones and Process Exit

WHEN a handle's process exits (any cause: success, non-zero, signal)
THE SYSTEM SHALL demote the live ring to a *tombstone* record containing:
- `handle_id`, `cmd`, `conversation_id`
- `exit_code` (or signal information)
- `duration_ms`
- `final_tail`: the last `TOMBSTONE_TAIL_LINES` (default 2000) lines
- `exited_at`
AND release the live ring buffer memory
AND mark `status: "exited"` (or `"killed"` if Phoenix sent the kill signal)

WHEN agent calls `peek` or `wait` on a tombstoned handle
THE SYSTEM SHALL serve the response from the tombstone with `tombstone: true`
AND return `final_tail` per the same read modifiers as the live ring (REQ-BASH-004),
limited to the tombstoned lines

WHEN a conversation is hard-deleted
THE SYSTEM SHALL kill any of that conversation's processes whose handles are
still `running`
AND remove all tombstone records for that conversation

**Rationale:** Demoting the ring to a final-tail tombstone bounds memory while
preserving "any handle the agent was given remains peekable for the lifetime
of the conversation." Tombstones are kilobytes; live rings are megabytes.
Hard-delete is the only event that loses a tombstone — matching the
tmux-integration spec, where `tmux -L <sock> kill-server` runs at the same
moment.

---

### REQ-BASH-007: Phoenix Restart Tombstones

WHEN a handle is created (process spawn)
THE SYSTEM SHALL persist a tombstone shadow record to SQLite with `status:
"running"`, `handle_id`, `cmd`, `conversation_id`, `started_at`

WHEN a handle transitions to `exited` or `killed`
THE SYSTEM SHALL update the SQLite shadow record with the final state, exit
information, and `final_tail`

WHEN Phoenix starts up
THE SYSTEM SHALL read SQLite shadow records
AND for any record still marked `status: "running"` (Phoenix did not write a
final state — graceful shutdown raced or process crashed), update the record
to `status: "lost_in_restart"` with `shutdown_marker_ts: <previous shutdown
timestamp if known, else startup_ts>`
AND start with an empty in-memory live handle table

WHEN agent peeks a handle whose record is `status: "lost_in_restart"`
THE SYSTEM SHALL return `status: "lost_in_restart"` with `shutdown_marker_ts`
and the last known `final_tail` if any was captured before shutdown

**Rationale:** Pit of success applied to errors: the agent sees
`lost_in_restart` (with a timestamp) instead of bare `handle_not_found`, so
it can reason about *why* the handle is gone. Crash-safe: even if Phoenix is
SIGKILLed, the next startup will detect the orphaned `running` records and
relabel them. The OS itself kills the children via SIGHUP cascade when
Phoenix's process group dies.

---

### REQ-BASH-008: Error Reporting

WHEN a command exits with non-zero status
THE SYSTEM SHALL return `status: "exited"` with the non-zero `exit_code` and
ring buffer contents (this is NOT a tool error — it is a successful tool call
that reports a non-zero exit)

WHEN the tool itself fails (handle not found, cap reached, schema validation
failed, safety check rejected, system spawn error)
THE SYSTEM SHALL return a structured error with:
- `error`: stable string identifier (one of `handle_not_found`,
  `handle_cap_reached`, `wait_seconds_out_of_range`,
  `peek_args_mutually_exclusive`, `command_safety_rejected`,
  `spawn_failed`, `mutually_exclusive_modes`)
- `error_message`: human-readable description suitable for the LLM
- additional structured fields specific to the error (e.g., `live_handles` for
  cap, `reason` for safety rejection)

THE SYSTEM SHALL distinguish "command produced an error exit code" from "tool
call could not complete" — the former is a normal tool result with status
"exited"; the latter uses the structured error envelope.

**Rationale:** Two distinct concepts that must not be confused: command-level
failure (the command ran and exited non-zero — useful information for the
agent) versus tool-level failure (the call could not be processed). Stable
error identifiers let agents and the eventual error-recovery surfaces match on
codes rather than parsing prose.

---

### REQ-BASH-009: No TTY Attached

WHEN bash tool spawns a command
THE SYSTEM SHALL run the command without a TTY
AND set stdin to `null`
AND establish the child as a process group leader (for clean kill on the whole
group)

THE SYSTEM SHALL describe in its tool documentation that interactive programs,
TTY-detecting programs (e.g., ones that change behavior under `isatty(stdout)`),
and programs that need to be sent input belong on the `tmux` tool, not bash.

**Rationale:** The tool contract is "non-interactive shell command, captured
output." Pit of success for the agent: the description points clearly at the
correct tool for the case bash cannot serve, removing the temptation to try
to coerce bash into doing something it cannot.

---

### REQ-BASH-010: Tool Schema and Mutual Exclusion

THE SYSTEM SHALL provide the bash tool schema with these properties:

- `cmd` (optional string): shell command to execute. Required for spawn.
- `wait_seconds` (optional integer, default 30): time to block for the
  foreground answer. Range [0, MAX_WAIT_SECONDS]. Used with `cmd` and
  `wait`.
- `peek` (optional string): handle ID to peek.
- `wait` (optional string): handle ID to wait on.
- `kill` (optional string): handle ID to kill.
- `signal` (optional enum: `TERM` | `KILL`, default `TERM`): used with `kill`.
- `lines` (optional integer): for peek/wait/spawn responses, return last N
  lines. Mutually exclusive with `since`.
- `since` (optional integer): for peek/wait/spawn responses, return lines from
  offset K. Mutually exclusive with `lines`.
- `mode` (optional, deprecated): backward-compatible alias for `wait_seconds`
  values: `default` → 30, `slow` → 900, `background` → 0. Logs a deprecation
  notice in the response. Removed in a future release.

THE SYSTEM SHALL enforce: exactly one of `{cmd, peek, wait, kill}` per call.
WHEN zero or more than one is provided
THE SYSTEM SHALL reject with `error: "mutually_exclusive_modes"` and list the
provided keys.

THE SYSTEM SHALL include the conversation's working directory in the tool
description, as the prior revision did.

**Rationale:** The schema tells the agent what shapes of call are legal. The
mutual-exclusion check turns the tool surface into a disjoint set of operations
rather than a free-form bag of fields. The deprecation alias buys one or two
releases for callers that still pass `mode`.

---

### REQ-BASH-011: Command Safety Checks

WHEN bash command is submitted for execution (spawn path only)
THE SYSTEM SHALL parse the command using a shell syntax parser
(`brush-parser`)
AND check for dangerous patterns before execution

WHEN a dangerous pattern is detected
THE SYSTEM SHALL reject the command with `error:
"command_safety_rejected"` and a `reason` describing the matched pattern
AND NOT execute the command (no handle is created, no tombstone is written)

THE SYSTEM SHALL reject the following patterns:
- Blind git add: `git add -A`, `git add .`, `git add --all`, `git add *`
- Force push: `git push --force`, `git push -f` (allow `--force-with-lease`)
- Dangerous rm: `rm -rf` on `/`, `~`, `$HOME`, `.git`, `*`, `.*`

WHEN command has `sudo` prefix
THE SYSTEM SHALL apply safety checks to the command following sudo

WHEN command appears in a pipeline or compound command
THE SYSTEM SHALL check all command components

**Rationale:** Unchanged from the prior revision. Safety checks remain UX
guardrails, not security boundaries. The check runs only on spawn, not on
peek/wait/kill, since those operate on already-spawned handles.

---

### REQ-BASH-012: Landlock Enforcement for Explore Mode

WHEN conversation is in Explore mode AND Landlock is available (Linux 5.13+)
THE SYSTEM SHALL execute bash commands under Landlock restrictions providing:
- Read-only filesystem access (all writes blocked at kernel level)
- No outbound network (TCP connect/bind blocked)
- Signal scoping (kernel 6.12+): processes cannot signal outside sandbox
- Resource limits via rlimits (memory, CPU time, process count)

WHEN Landlock blocks an operation in Explore mode
THE SYSTEM SHALL return the kernel error (EACCES, EPERM) in the ring buffer
output as the command saw it
AND the tool description SHALL include a clear explanation of sandbox
constraints

WHEN conversation is in Work mode
THE SYSTEM SHALL NOT apply Landlock restrictions
AND bash commands SHALL have write access within the conversation's worktree
directory

**Rationale:** Unchanged from the prior revision. Landlock is defense-in-depth
for Explore mode; primary isolation is git worktrees (see `specs/projects/`).

> **Landlock Feature Matrix:**
> | Kernel | ABI | Features |
> |--------|-----|----------|
> | 6.12+  | v6  | Full protection: filesystem, network, ioctl, signal/socket scoping |
> | 6.10-6.11 | v5 | + Device ioctl blocking |
> | 6.7-6.9 | v4 | Filesystem + network (TCP) |
> | 5.13-6.6 | v1-v3 | Filesystem only |
>
> Recommended: Kernel 6.12+ for full signal scoping; 6.7+ minimum for network
> blocking.

---

### REQ-BASH-013: Graceful Degradation Without Landlock

WHEN Landlock is unavailable (non-Linux OS or Linux kernel < 5.13)
THE SYSTEM SHALL detect this at startup
AND log a warning that Explore mode read-only enforcement is advisory only
AND continue to enforce read-only semantics at the application layer

WHEN running on a non-Linux operating system
THE SYSTEM SHALL enforce Explore mode read-only constraints at the tool level
only
AND indicate to users that kernel-level enforcement is unavailable
AND note that physical worktree isolation (REQ-PROJ-005) still provides
write-write isolation between conversations on all platforms

WHEN degraded mode is active
THE SYSTEM SHALL still apply command safety checks (REQ-BASH-011)
AND the absence of Landlock SHALL NOT prevent Work mode from functioning

**Rationale:** Unchanged from prior revision.

---

### REQ-BASH-014: Stateless Tool with Per-Conversation Handle Registry

WHEN bash tool is invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext` parameter
AND derive working directory from `ToolContext.working_dir`
AND use `ToolContext.cancel` for cancellation handling
AND access the bash handle registry via `ctx.bash_handles()` (analogous to the
existing `ctx.browser()` pattern for browser sessions)

WHEN bash tool is constructed
THE SYSTEM SHALL NOT store per-conversation state on the tool itself
AND tool instance SHALL be reusable across conversations

THE handle registry SHALL be per-conversation; calls in one conversation cannot
peek, wait, or kill handles owned by another conversation. A `handle_not_found`
is the response if a handle ID from one conversation is presented in another.

**Rationale:** The bash tool itself remains stateless — instance reusable,
context flows through `ToolContext`. The handle table is a shared service
(like the browser session manager), reached through the context, scoped to the
conversation. This preserves the "no stale state on the tool" property while
acknowledging that handles are inherently per-conversation state managed
elsewhere.

---

### REQ-BASH-015: Display Command Simplification

WHEN bash tool result is displayed in the UI
THE SYSTEM SHALL simplify the command for display by removing boilerplate
prefixes
AND provide a `display` field alongside the original `cmd`

WHEN command contains `cd <path> && <rest>`
AND `<path>` matches the conversation's working directory
THE SYSTEM SHALL display only `<rest>` (strip the redundant cd)

WHEN command contains `cd <path> && <rest>`
AND `<path>` does NOT match the conversation's working directory
THE SYSTEM SHALL display the full command unchanged

WHEN command contains `cd <path>; <rest>` (semicolon separator)
AND `<path>` matches the conversation's working directory
THE SYSTEM SHALL display only `<rest>`

WHEN command contains `||` (or operator)
THE SYSTEM SHALL preserve the full command including fallback
AND NOT strip any prefix before `||`

WHEN command contains mixed operators like `cd <path> && cmd || fallback`
AND `<path>` matches the conversation's working directory
THE SYSTEM SHALL display `cmd || fallback` (strip only the matching cd)

WHEN displaying handle operations (peek/wait/kill)
THE SYSTEM SHALL show the operation kind and handle ID (e.g., `peek b-7`,
`kill b-7 (TERM)`) rather than attempting to display a fictitious command
string

**Rationale:** Unchanged for spawn calls. Extended for the new handle
operations so the UI has a sensible display for non-spawn calls.

---

## Configuration Constants

| Name | Default | Description |
|---|---|---|
| `MAX_WAIT_SECONDS` | 900 | Upper bound on `wait_seconds` per call |
| `RING_BUFFER_BYTES` | 4 MB | Per-handle live ring buffer size |
| `LIVE_HANDLE_CAP` | 8 | Per-conversation cap on `running` handles |
| `KILL_GRACE_SECONDS` | 5 | Grace period after `TERM` before escalating to `KILL` |
| `TOMBSTONE_TAIL_LINES` | 2000 | Lines retained in tombstone after exit |
| `DEFAULT_PEEK_LINES` | 200 | Lines returned when peek has no read modifier |
| `DEFAULT_WAIT_SECONDS` | 30 | Default `wait_seconds` when omitted |
