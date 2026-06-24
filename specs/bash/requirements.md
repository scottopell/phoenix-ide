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

**Persistence boundary:** handles are in-memory only, owned by the `WorkScope`
that spawned them (REQ-BASH-WS-001). They survive arbitrarily long within a
single Phoenix process — including across a continuation boundary, because a
continuation inherits its predecessor's `WorkScope` — but a Phoenix restart
drops them: the agent will see `handle_not_found` on a previously-known handle.
Persistence across Phoenix restart is what `tmux` is for.

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

WHEN agent calls `bash(op="run", cmd=<command>, wait_seconds=N)`
THE SYSTEM SHALL block up to N seconds for the command to exit

WHEN the command exits within N seconds
THE SYSTEM SHALL return `status: "exited"` with `exit_code`, `duration_ms`, and
the ring buffer contents (subject to the peek shape, REQ-BASH-004)

WHEN N seconds elapse before the command exits
THE SYSTEM SHALL return `status: "still_running"` with `handle`, `waited_ms`,
`end_offset`, and a tail of the ring buffer
AND keep the process running, accepting subsequent peek/wait/kill operations on
the handle

WHEN agent calls bash with `op="run"` and `wait_seconds=0`
THE SYSTEM SHALL spawn the process and return immediately with `status:
"still_running"` and a handle, without waiting for any output

WHEN `wait_seconds` is omitted
THE SYSTEM SHALL apply a default of 30 seconds

WHEN `wait_seconds` exceeds `MAX_WAIT_SECONDS` (default 900)
THE SYSTEM SHALL reject the call with `error: "wait_seconds_out_of_range"`
AND state the bound in the error

WHEN agent supplies `label=<string>` on the run call
THE SYSTEM SHALL attach the label to the handle
AND echo it on every response that carries the handle (`still_running`,
`exited`, `tombstoned`, `kill_pending_kernel`) and on each entry of the
`live_handles[]` array in the `handle_cap_reached` error response

THE tool description SHALL state explicitly that `wait_seconds` is **NOT** a
process-kill timeout: the process is **never** killed when `wait_seconds`
elapses; it keeps running and the agent receives a handle. This negation is
load-bearing — language models trained on POSIX `timeout(1)` and similar APIs
default to the kill-on-timeout intuition; affirmative descriptions get
pattern-matched into that prior, and explicit negations override it.

THE tool description SHALL similarly state explicitly that `op="run"` does
**NOT** detach: when the command finishes within `wait_seconds`, the agent
receives a normal synchronous result. The handle is minted only when
`wait_seconds` elapses first. This negation overrides the `fork(2)` /
fire-and-forget prior that the previous name `spawn` invoked; without it,
models treat `run` as if it always backgrounded.

**Rationale:** The renamed parameter (`wait_seconds`, replacing `timeout`)
removes the "kill" connotation that the old name carried. The hard distinction
between `status: "exited"` and `status: "still_running"` makes the
"timed-out-but-process-still-running" case unmistakable to the agent — pit of
success on the read side. The `MAX_WAIT_SECONDS` cap exists so the agent cannot
inadvertently park a request for hours: long-running operations should yield a
handle and resume via `wait` calls. The explicit-negation rules in the tool
description were added cumulatively across revisions: the rename of `timeout`
→ `wait_seconds` was insufficient signal alone (revision 2 added the
load-bearing "NOT a timeout" wording), and the rename of `op=spawn` →
`op=run` similarly required an explicit "does NOT detach" line (revision 3)
because the new name still co-exists with the fire-and-forget prior in
training data.

The optional `label` field exists because agents juggling concurrent
handles cannot otherwise distinguish them across responses: `b-3` and
`b-4` are opaque, while `"dev-server"` and `"test-runner"` are not.
Labels surface in the cap-reached error so the agent has the information
it needs to choose which handle to retire.

---

### REQ-BASH-003: Handle Operations (Peek, Wait, Kill)

WHEN agent calls `bash(peek=<handle>, ...)`
THE SYSTEM SHALL return the current state of the handle, including:
- `status`:
  - `"running"` — process is alive, no kill signal sent
  - `"still_running"` — used only on run/wait responses when the
    wait window elapsed; not a peek response
  - `"kill_pending_kernel"` — Phoenix sent a kill signal but the
    response timer expired before exit (D-state hang). The process is
    still alive.
  - `"tombstoned"` — process has finished; the response is served from
    a tombstone record. Carries `final_cause` and (when applicable)
    `exit_code` and `signal_number`.
- `final_cause` (only when `status = "tombstoned"`): `"exited"` |
  `"killed"`. Tells the agent how the handle reached its terminal state.
- `exit_code` (when status is `tombstoned` and `final_cause = "exited"`,
  or when an exit code is otherwise available; null when the process
  was killed by signal with no status code)
- `signal_number` (optional; when status is `tombstoned` and a signal
  is known to have terminated the process — either Phoenix-sent or
  external such as oom-killer): the signal number for log readability
- ring buffer contents per the offset/lines parameters (REQ-BASH-004)

WHEN agent calls `bash(wait=<handle>, wait_seconds=N)`
THE SYSTEM SHALL block up to N seconds for the handle's process to exit
AND return the same response shape as REQ-BASH-002 (`status: "exited"` on
completion, `status: "still_running"` on re-timeout)
AND on re-timeout, return the *same* handle (not a new one)

WHEN agent calls `bash(kill=<handle>, signal=<TERM|KILL>)`
THE SYSTEM SHALL send the specified signal (default `TERM`) to the process group
AND wait up to `KILL_RESPONSE_TIMEOUT_SECONDS` (default 30) for the process to
exit
AND return the response shape with the final state and `signal_sent`

WHEN the process does not exit within `KILL_RESPONSE_TIMEOUT_SECONDS` after the
signal is sent (typical cause: `D`-state on a frozen mount or kernel-level
uninterruptible sleep)
THE SYSTEM SHALL return `status: "kill_pending_kernel"` with `signal_sent`,
`waited_ms`, and the ring buffer tail
AND leave the kill task in the registry so a subsequent kill / wait / peek can
observe the eventual exit

WHEN agent calls `kill` with `signal=TERM` and the process is one the agent
expects to require graceful shutdown (e.g., a database with WAL flush)
AND TERM does not take effect within the agent's chosen response window
THE agent SHALL call `kill` again with `signal=KILL` to escalate explicitly

WHEN agent calls peek/wait/kill on a handle that does not exist in the live
table or in the in-memory tombstone store
THE SYSTEM SHALL return `error: "handle_not_found"`

THE SYSTEM SHALL NOT auto-escalate from TERM to KILL. A kill call sends exactly
the requested signal once. Agents that want escalation must request it
explicitly with a second call.

**Rationale:** Three operations cover the lifecycle of a backgrounded handle.
Auto-escalation TERM → KILL was removed in revision 2: a model trained on POSIX
sends `signal: TERM` because it specifically wants the process to clean up
gracefully (flush logs, close DB connections, write final state); silently
upgrading to KILL after a fixed grace period defeats the agent's intent and
routinely corrupts services with legitimately long shutdown paths (Postgres,
Elasticsearch, anything with a WAL flush). The agent already has the primitives
to escalate explicitly; making it explicit keeps the agent in control.

The `kill_pending_kernel` status covers the kernel-uninterruptible-sleep case
(SIGKILL is uncatchable but does not guarantee exit when the process is in
`D`-state on a frozen mount). The kill response returns rather than hanging
forever; subsequent calls can observe the eventual transition.

Returning the same handle on `wait` re-timeout (rather than minting a new one)
is the pit-of-success choice: the agent never accumulates handles across
re-waits.

---

### REQ-BASH-004: Ring Buffer and Read Semantics

WHEN a handle's process produces output on stdout or stderr
THE SYSTEM SHALL append the bytes to a per-handle ring buffer bounded by
`RING_BUFFER_BYTES` (default 4 MB)
AND split incoming bytes on newline boundaries to assign each complete line a
monotonically increasing offset (line 0, 1, 2, ... since spawn)

WHEN the ring buffer's retained complete-line bytes reach `RING_BUFFER_BYTES`
and new content arrives
THE SYSTEM SHALL evict the oldest lines until the new content fits
AND advance `start_offset` to the offset of the oldest still-retained line

THE SYSTEM SHALL track a monotonic count of total bytes the process has
written — incremented on every append, inclusive of bytes currently held as
an un-newlined trailing partial, and never decremented by eviction. This
count is the reported output size (`output_bytes`), distinct from the
retained-byte count that drives eviction. It is defined in every state
(0 at spawn), surfaced on the observability inventory and process-inspection
wire as `output_bytes`, and persisted into the tombstone so a terminal handle
reports the same total a final live read would have.

WHEN incoming bytes have no trailing newline
THE SYSTEM SHALL hold them as a partial line bounded by `MAX_PARTIAL_BYTES`
(default 64 KiB): when an append would grow the partial past that bound, the
partial is flushed immediately as a complete line (receiving the next
monotonic offset) so a process that never emits a newline cannot grow the
partial without bound

WHEN a handle's reader idles with a non-empty partial line for
`PARTIAL_IDLE_FLUSH_SECONDS` (default 10)
THE SYSTEM SHALL flush the partial as a complete line so output a process
emitted without a trailing newline and then stopped producing becomes
visible to `since`/`lines` reads (which return complete lines only) rather
than staying invisible until EOF

THE read window returned on a live peek/wait/run response SHALL carry the
current trailing partial line (lossy UTF-8) as a `partial` field, structurally
distinct from the complete `lines`. A tombstone read carries no `partial`
(the partial was flushed to a complete line on EOF before demotion).

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

EVERY peek/wait/run response SHALL include `start_offset`, `end_offset`, and
`truncated_before` for the lines returned

**Rationale:** Caller-controlled offsets keep the server stateless on read
cursors — a dropped network response, a re-asking agent, or a UI peeker do not
race each other. `truncated_before` makes information loss explicit rather than
silent: the agent can detect when content fell out of the window and decide how
to respond.

The reported output size is a monotonic total of bytes written, not the
retained-line byte count. These are two different values doing two different
jobs: the retained count bounds memory (it falls as lines evict), while the
output total answers "how much has this process produced" (it only rises). A
single field cannot honestly serve both — a no-newline emitter would report
0 forever under a retained-line count, and a heavily-evicted ring would
under-report. The total is partial-inclusive so output a process is mid-way
through emitting still counts, and it is persisted into the tombstone so the
count survives the live-to-terminal transition rather than vanishing.

The partial line is bounded structurally (`MAX_PARTIAL_BYTES`) rather than by
convention: the eviction cap governs complete lines only, so without a partial
bound a process that never emits `\n` is an unbounded memory leak. The idle
flush (`PARTIAL_IDLE_FLUSH_SECONDS`) exists because `since`/`lines` reads
return complete lines only — a prompt or progress fragment emitted without a
newline would otherwise be invisible to the agent until the process exits. The
read window exposes the partial as a field distinct from `lines` so a consumer
that wants the in-progress line (the process inspector) can render it while the
LLM peek, which reasons over complete lines, can ignore it; folding it into
`lines` would give it a fake offset and blur "complete" against "in progress."

---

### REQ-BASH-005: Live Handle Cap

WHEN agent calls `bash(cmd=<command>, ...)` AND the work scope has
`LIVE_HANDLE_CAP` (default 8) live handles (status `running`)
THE SYSTEM SHALL reject the call with:
- `error: "handle_cap_reached"`
- `cap`: the configured value
- `live_handles`: list of `{ handle, cmd, age_seconds, status }` for each live
  handle in the work scope
- `hint`: text directing the agent to kill or wait on a handle, or use the
  `tmux` tool for long-runners

WHEN a handle transitions out of `running` (exit, kill, signal)
THE SYSTEM SHALL decrement the live count
AND a subsequent run in the same work scope MAY succeed if it brings the
live count under the cap

**Rationale:** A hard refusal is the pit-of-success failure mode. LRU eviction
silently kills the very handle the agent was about to peek; soft warnings
permit unbounded accumulation. Refusing with an actionable list of live
handles tells the agent exactly what to do.

---

### REQ-BASH-006: Tombstones and Process Exit

WHEN a handle's process exits (any cause: success, non-zero, signal)
THE SYSTEM SHALL demote the live ring to an *in-memory tombstone* record
containing:
- `handle_id`, `cmd`
- `exit_code` (when the kernel returned a status code; null when killed
  by signal with no code)
- `signal_number` (optional; when a signal terminated the process, this
  is the signal number — derived from `ExitStatus::signal()` when
  `WIFSIGNALED`, or from `(exit_code - 128)` when `bash -c` reports a
  conventional 128+signum exit code)
- `duration_ms`, `exited_at`
- `final_tail`: the last `TOMBSTONE_TAIL_LINES` (default 2000) lines
- `final_cause`: `"exited"` | `"killed"`
AND release the live ring buffer memory
AND set the persisted handle status to `exited` (kernel returned an
exit code) or `killed` (process terminated by signal — whether
Phoenix-sent or external such as oom-killer)

WHEN agent calls `peek` or `wait` on a tombstoned handle
THE SYSTEM SHALL serve the response with `status: "tombstoned"` and the
`final_cause` field carrying the underlying terminal cause
AND return `final_tail` per the same read modifiers as the live ring
(REQ-BASH-004), limited to the tombstoned lines

WHEN agent calls `kill` on a handle that is already terminal
THE SYSTEM SHALL respond with the same `status: "tombstoned"` shape as
peek/wait — no `already_terminal` flag is needed because the
`tombstoned` status conveys it

WHEN a conversation is hard-deleted AND no surviving conversation inherits its
`WorkScope`
THE SYSTEM SHALL kill any of that `WorkScope`'s processes whose handles are
still `running`
AND remove all tombstone records for that `WorkScope`

WHEN a conversation is hard-deleted AND a continuation inherits the same
`WorkScope`
THE SYSTEM SHALL leave that `WorkScope`'s handles and tombstones intact for the
inheritor (REQ-BASH-WS-002)

WHEN Phoenix shuts down (gracefully or via crash)
THE SYSTEM SHALL kill all live processes via the reaper machinery
(REQ-BASH-007)
AND make no attempt to persist tombstones across the restart

THE in-memory tombstone store SHALL NOT be backed by SQLite. Tombstones live
only as long as the Phoenix process. A subsequent agent peek on a handle that
predates the current Phoenix process returns `handle_not_found` — the agent
re-runs, or the agent should have used the `tmux` tool if it needed
persistence across restart.

**Rationale:** Demoting the ring to a final-tail tombstone bounds memory while
preserving "any handle the agent was given remains peekable for the lifetime
of the Phoenix process." Tombstones are kilobytes; live rings are megabytes.
Hard-delete is the only event that loses a tombstone within a Phoenix
lifetime.

The "no SQLite shadow store" decision was made in revision 2: the structured
`lost_in_restart` response that v1 originally proposed was not worth the
complexity (a 7-column table per handle, reconciliation logic at startup, and
unbounded growth across restarts because the live-handle cap doesn't apply to
tombstones). Bare `handle_not_found` is exactly what agents already handle
gracefully.

The persisted status variants (`exited`, `killed`, `kill_pending_kernel`)
plus the response-shape `tombstoned` cover the cases the prior draft
spread across five values. The earlier `signaled` status was dropped:
under `bash -c "<user_cmd>"` the bash wrapper exits normally with code
128+signum when the user_cmd is signal-killed, so `Child::wait()`'s
`ExitStatus::signal()` returned None and `signaled` never fired for
its documented case (oom-killer killing the user code). Signal
information is preserved on the `killed` state via the optional
`signal_number` field. D-state hangs after kill remain explicitly
modelled as `kill_pending_kernel` because the kill response cannot
wait forever.

---

### REQ-BASH-007: Child Process Reaper

WHEN Phoenix starts up
THE SYSTEM SHALL call `prctl(PR_SET_CHILD_SUBREAPER, 1)` (Linux 3.4+) at the
process level so that any descendant whose parent dies before reaping it is
reparented to Phoenix rather than init
AND log a warning at startup if the call is unavailable on the host platform

WHEN a bash handle spawns a child
THE SYSTEM SHALL set the child as a process group leader via `pre_exec(setpgid(0, 0))`
AND the kill path SHALL signal the entire process group via `kill(-pgid, signal)`
to catch immediate descendants

WHEN Phoenix is shutting down (graceful or abnormal-but-handler-runnable)
THE SYSTEM SHALL walk the live handle table and send `SIGKILL` to each
handle's process group as a final cleanup pass before exit
AND wait briefly (up to `SHUTDOWN_KILL_GRACE_SECONDS`, default 2) for those
groups to exit before relinquishing control to the OS

THE SYSTEM SHALL NOT rely on parent-death cascades (SIGHUP-on-parent-exit) for
child cleanup. SIGHUP delivers on controlling-terminal hangup, not on parent
process death; Phoenix is not a session leader for these children, so SIGHUP
cascade is not a reliable mechanism.

**Rationale:** This requirement was added in revision 2 after a UNIX
correctness review. The earlier draft assumed `setpgid(0,0)` + kernel SIGHUP
would cascade and clean up descendants when Phoenix died. That assumption is
wrong on Linux: SIGHUP is a TTY-hangup signal, not a parent-death signal, and
Phoenix is not a controlling-terminal session leader. Without
`PR_SET_CHILD_SUBREAPER`, double-forked daemons (`(cmd &) &`, `nohup`, programs
that call `setsid`) and any descendant that resets its own pgid will outlive
Phoenix and leak. With the subreaper bit set, escapees reparent to Phoenix
rather than init, and the shutdown kill-tree pass cleans them up before exit.

`SIGKILL` at shutdown rather than `SIGTERM` because Phoenix is exiting anyway —
no point waiting on graceful shutdown handlers when the parent is leaving.

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
- additional structured fields specific to the error (e.g., `live_handles`
  for cap, `reason` for safety rejection, `conflicting_args` for
  mutually-exclusive cases)

THE SYSTEM SHALL distinguish "command produced an error exit code" from "tool
call could not complete" — the former is a normal tool result with status
"exited"; the latter uses the structured error envelope.

WHEN agent calls `bash(peek=<handle>)` on a handle that predates the
current Phoenix process (typical case: Phoenix restarted between spawn
and peek)
THE SYSTEM SHALL return `error: "handle_not_found"` with a hint
field directing the agent to use the `tmux` tool for processes that
should survive Phoenix restart

**Rationale:** Two distinct concepts that must not be confused: command-level
failure (the command ran and exited non-zero — useful information for the
agent) versus tool-level failure (the call could not be processed). Stable
error identifiers let agents and the eventual error-recovery surfaces match on
codes rather than parsing prose.

The dual-pass case where the agent supplies both the deprecated `mode`
and the canonical `wait_seconds` on the same call (REQ-BASH-010) is
folded into `mutually_exclusive_modes` with structured `conflicting_args`
and `recommended_action` fields, rather than carrying its own error code.
The agent's recovery (drop one of the conflicting args) is the same
shape as the operation-key conflict, so a single stable id with
structured details keeps the surface tight.

The `handle_not_found`-with-tmux-hint pattern keeps the two-tier
persistence model visible to the agent at exactly the moment confusion
is most likely (a previously-known handle is suddenly absent).

---

### REQ-BASH-009: No TTY Attached

WHEN bash tool spawns a command
THE SYSTEM SHALL run the command without a TTY
AND set stdin to `null`
AND establish the child as a process group leader (REQ-BASH-007) for clean
kill on the whole group

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

- `op` (required enum: `run` | `peek` | `wait` | `kill`): operation
  discriminator. The single source of truth for which operation to dispatch.
- `cmd` (optional string): shell command to execute. Required when `op=run`.
- `handle` (optional string): handle id. Required when `op=peek|wait|kill`.
- `label` (optional string): human-readable annotation for the spawned
  handle. Used with `op=run`. Echoed on every response that carries the
  handle (`still_running`, `exited`, `tombstoned`, `kill_pending_kernel`)
  and on each entry of `live_handles[]` in `handle_cap_reached`. Length
  capped at `MAX_LABEL_LENGTH` (default 64); over-cap labels are rejected
  with `error: "label_too_long"`.
- `wait_seconds` (optional integer, default 30): time to block for the
  foreground answer. Range [0, MAX_WAIT_SECONDS]. Used with `op=run` and
  `op=wait`.
- `signal` (optional enum: `TERM` | `KILL`, default `TERM`): used with
  `op=kill`.
- `lines` (optional integer, minimum 1): tail-mode read window — return the
  last N lines. Mutually exclusive with `since`.
- `since` (optional integer, minimum 1): incremental-mode read window —
  return lines after offset K. Mutually exclusive with `lines`.

THE SYSTEM SHALL determine the operation strictly from `op`. WHEN `op` is
absent, malformed, or carries a value outside the advertised enum, THE
SYSTEM SHALL reject the call with `error: "mutually_exclusive_modes"` and
a `recommended_action` directing the agent to set `op` to one of the
advertised operations.

THE SYSTEM SHALL deserialise the input with `deny_unknown_fields`. Top-
level keys outside the advertised schema (notably the retired affordances:
`mode`, `command` as an alias for `cmd`, and bare `peek` / `wait` / `kill`
as legacy operation keys) SHALL surface as a structured parse error rather
than being silently absorbed.

THE SYSTEM SHALL apply two narrow tolerances against current GPT models'
default-fill behaviour on the *current* schema:
- `since=0` SHALL be treated as absent. `0` is below the advertised
  `minimum: 1`; current OpenAI Responses-API models still emit it as a
  default-fill on optional integers. Treating it as absent routes through
  the default `lines` window. The parser SHALL emit a `tracing::debug!`
  line naming the dropped value so the tolerance is auditable in logs.
- WHEN both `lines` and `since` are supplied, the parser SHALL prefer
  `lines` and silently drop `since`. Models on structured-output APIs
  default-fill optional integers with their schema minimums (`lines=1`,
  `since=1`); for short command output, `since=1` returns nothing while
  `lines=200` (the request default) returns the actual tail. The drop
  SHALL emit a `tracing::debug!` line.

THE SYSTEM SHALL include the conversation's working directory in the tool
description, as the prior revision did. The description SHALL lead with a
compact cookbook block (foreground / background / inspect / wait) so the
`wait_seconds=0` "give me a handle now" affordance is discoverable.

**Rationale:** This requirement passed through three revisions. The
original four-sibling shape (`cmd`, `peek`, `wait`, `kill` as parallel
optional strings, runtime mutex) collapsed under OpenAI Responses-API
default-fill of optional strings. Revision 2 introduced the `op`
discriminator and a long list of tolerances (legacy four-sibling
inference, empty-string-as-absent on the legacy keys, `mode` parameter
shim, `command` alias for `cmd`) defending in-flight conversation
history. Revision 3 (this one) retired the in-flight-history tolerances:
LLMs see the current tool definition each turn and conform to the current
schema, so pre-discriminator history is inert text from the model's
perspective. Maintaining those tolerances costs code, tests, and prose
for protection that real call paths don't need; deleting them is a
structural simplification.

The two surviving tolerances (`since=0`-as-absent and `lines+since`
collision resolution) are different in kind: they defend against active
GPT default-fill on the *current* schema, where the model emits values
below the advertised minimums. `tracing::debug!` makes both visible in
logs.

The `op=spawn` rename to `op=run` happened because `spawn` carries a
`fork(2)` / fire-and-forget prior in the model's training data: the
operation is in fact a run-and-optionally-yield-a-handle. "Run" matches
the agent's mental model. The same kind of fix as the prior
`timeout` → `wait_seconds` rename. No legacy alias is kept; `op="spawn"`
is a parse error like any unknown enum value.

The `label` field exists because agents juggling concurrent handles cannot
otherwise distinguish `b-3` from `b-4` without external bookkeeping. The
cap-reached error already shows `cmd` per handle; the label gives the
agent a stable annotation across all responses.

---

### REQ-BASH-011: Command Safety Checks

WHEN a bash `run` command is dispatched
THE permission seam (specs/permissions/, deterministic deny layer) SHALL
parse the command using a shell syntax parser (`brush-parser`) and reject
dangerous patterns BEFORE the bash tool is invoked
AND the bash tool itself SHALL NOT re-check — enforcement is single-homed at
the seam.

THE bash dangerous-pattern catalog the seam's Layer 0 applies is:
- Blind git add: `git add -A`, `git add .`, `git add --all`, `git add *`
- Force push: `git push --force`, `git push -f` (allow `--force-with-lease`)
- Dangerous rm: `rm -rf` on `/`, `~`, `$HOME`, `.git`, `*`, `.*`

WHEN a pattern matches (in a simple command, a `sudo`-prefixed command, or any
pipeline/compound component)
THE seam SHALL reject with `error: "command_safety_rejected"` and a `reason`
describing the matched pattern
AND the command SHALL NOT execute (no handle is created, no tombstone written).

**Rationale:** Safety checks remain UX guardrails, not security boundaries.
Enforcement lives at the permission seam rather than inside the bash tool so
every tool — not just bash — passes one gate; the dangerous-pattern catalog is
bash-domain knowledge the seam's Layer 0 applies. The check covers the `run` op
only; peek/wait/kill operate on already-spawned handles. See specs/permissions/.

---

### REQ-BASH-012: `nono` Enforcement for Explore Mode

WHEN a top-level Explore conversation exposes `bash`
THE SYSTEM SHALL execute `op="run"` commands in a Phoenix-owned child process that
applies a `nono` OS sandbox before execing `bash -c`
AND SHALL NOT call `nono::Sandbox::apply()` in the long-running Phoenix server
process

THE Explore bash sandbox SHALL provide:
- broad filesystem read access matching Explore's existing read-only tool semantics
- read-only Git metadata access sufficient for linked worktree commands such as
  `git status`, `git log`, and `git blame`
- write access only to Phoenix-owned scratch, synthetic home, and writable temp
  locations; task proposal files are created through scoped `patch`/`propose_task`,
  not through sandboxed bash
- a synthetic sandbox home under scratch, exposed as `PHOENIX_SANDBOX_HOME` and
  `HOME`
- `PHOENIX_SANDBOX_SCRATCH` pointing at Phoenix-owned scratch
- platform-compatible temporary directory writes, exposed as `TMPDIR`; when the
  repo itself lives under the platform temp root, `TMPDIR` falls back to a
  Phoenix-owned scratch child so the temp grant cannot cover source files
- inherited `PATH` preservation
- blocked network access
- a reduced environment that strips ambient SCM/OAuth, LLM-provider, and
  cloud/vendor credential variables

THE SYSTEM SHALL remove Phoenix-owned per-command scratch/home directories after
that sandboxed bash command reaches a terminal state

WHEN `nono` blocks an operation in Explore mode
THE SYSTEM SHALL return the kernel error (for example EACCES or EPERM) in the
ring buffer output as the command saw it
AND the tool description SHALL include a clear explanation of sandbox
constraints

WHEN conversation is in Direct, Work, or Branch mode
THE SYSTEM SHALL NOT apply the Explore read-only sandbox to bash
AND bash commands SHALL retain the existing writable behavior for that mode

**Rationale:** Explore bash is useful for local code investigation (`git log`,
`git blame`, `rg`, `cat`) only if the read-only promise is enforced below the
application layer. Explore is a read-only/network-blocked mode, not a
confidentiality boundary: existing Explore read tools can already read arbitrary
user-selected paths, so sandboxed bash follows the same broad-read model. Readable
credential files, Phoenix data files, procfs process environments, and other
ordinary readable filesystem content are part of that accepted read model; protecting
sensitive reads is a separate feature with its own threat model. The sandbox
constrains writes, network, and the ambient environment
it directly passes to the child process. `nono` is the sandbox abstraction; it
uses the platform's supported backend (Landlock on Linux, Seatbelt on macOS) and
reports support at startup.

---

### REQ-BASH-013: Fail-Closed Explore Bash Availability

WHEN `nono::Sandbox::support_info()` reports that no enforceable sandbox backend
with network-block support is available
THE SYSTEM SHALL detect this at startup
AND SHALL NOT expose `bash` in top-level Explore mode
AND SHALL continue to expose the read-only/planning Explore tool set

WHEN degraded mode is active
THE SYSTEM SHALL still apply command safety checks (REQ-BASH-011) to modes that
expose bash
AND the absence of Explore bash SHALL NOT prevent Direct, Work, or Branch mode
from functioning

**Rationale:** A tool-level read-only convention is not a security boundary. If
Phoenix cannot enforce the Explore bash policy at the OS boundary, Explore mode
fails closed by withholding bash rather than presenting a writable shell with an
advisory label.

---

### REQ-BASH-014: Stateless Tool with Per-WorkScope Handle Registry

WHEN bash tool is invoked
THE SYSTEM SHALL receive all execution context via a `ToolContext` parameter
AND derive working directory from `ToolContext.working_dir`
AND use `ToolContext.cancel` for cancellation handling
AND access the bash handle registry via `ctx.bash_handles()`, which SHALL
resolve the table for `ToolContext.work_scope` and return
`Result<Arc<RwLock<WorkScopeHandles>>, BashHandleError>` matching
the existing `ctx.browser()` accessor's `async + Result + Arc<RwLock<...>>`
shape

WHEN bash tool is constructed
THE SYSTEM SHALL NOT store per-WorkScope state on the tool itself
AND tool instance SHALL be reusable across conversations and work scopes

THE handle registry SHALL be keyed by `WorkScope`; calls in one work scope
cannot peek, wait, or kill handles owned by another work scope. Conversations
that resolve to the same `WorkScope` — a continuation chain on one worktree —
share a single handle table. A `handle_not_found` is the response if a handle
ID from one work scope is presented in another.

**Rationale:** The bash tool itself remains stateless — instance reusable,
context flows through `ToolContext`. The handle table is a shared service
(like the browser session manager), reached through the context, scoped to the
`WorkScope`. The accessor signature matches the `browser()` shape so all
`WorkScope`-keyed tool registries (browser, tmux, terminal, bash) share one
pattern (`async fn foo(&self) -> Result<Arc<RwLock<T>>, FooError>`); a
lifetime-bound `BashHandleScope<'_>` alternative does not compose with
`ToolContext: Clone`.

---

### REQ-BASH-WS-001: Handle Registry Keyed by WorkScope

THE handle registry SHALL key its per-scope handle tables by `WorkScope`, not
by conversation id — matching the terminal, browser, and tmux registries
(REQ-TERM-WS-001, REQ-BROWSER-WS-001).

WHEN two conversations resolve to the same `WorkScope` (a continuation chain on
one worktree)
THE SYSTEM SHALL give them the same handle table, so a handle spawned before a
continuation boundary remains addressable for peek/wait/kill after it
AND count both conversations' live handles against the one per-`WorkScope`
`LIVE_HANDLE_CAP` (REQ-BASH-005)

WHEN a handle id owned by one `WorkScope` is presented in a call running under a
different `WorkScope`
THE SYSTEM SHALL return `error: "handle_not_found"` (no cross-scope leakage of
handle existence)

**Rationale:** A backgrounded process is a `WorkScope`-level resource, like the
tmux server and browser session that share its worktree. Conversation-keying
orphans a live process at every continuation boundary: the process keeps
running but becomes unaddressable from the continuation, so the agent sees the
runtime silently forget half its in-flight work. Keying by `WorkScope` makes
bash symmetric with the other three runtime resources and makes "the work scope
owns its processes" structural rather than conventional. The scope is derived
from the persisted `ConvMode::worktree_path()`, the single authority every
DB-facing path (inventory, hard-delete cascade, work-scope SSE routing) also
resolves from — so the handle-table keying and the inventory/cleanup keying
cannot diverge. Direct-mode conversations and Explore sub-agents (which persist
`worktree_path: None`) resolve to `WorkScope::Conversation(id)`, for which this
is exactly one conversation's handles; worktree-backed chains and Work-mode
sub-agents (which inherit the parent's worktree-bearing `conv_mode`) resolve to
`WorkScope::Worktree(path)` and so share one table — the former across a
continuation boundary, the latter with the live parent, both toward survival.

---

### REQ-BASH-WS-002: Hard-Delete Cascade Preserves a Scope Still Owned by a Live Conversation

WHEN a conversation is hard-deleted AND a non-terminal conversation OTHER THAN
the one being deleted resolves to the same `WorkScope` — whether a continuation
that inherits it, or a sibling such as a Work-mode sub-agent that shares its
parent's scope
THE SYSTEM SHALL NOT kill that `WorkScope`'s running processes or drop its
tombstones — the surviving owner keeps them

WHEN a conversation is hard-deleted AND no non-terminal conversation other than
the one being deleted resolves to its `WorkScope`
THE SYSTEM SHALL kill every running process in that `WorkScope`'s handle table
AND drop the `WorkScope`'s handles and tombstones

THE cascade SHALL receive the deleted conversation's `WorkScope` and a
preservation signal that is true iff the scope is still owned, and SHALL skip
teardown when the signal is true, mirroring `cascade_terminal_on_delete` /
`cascade_browser_on_delete` (REQ-TERM-WS-002, REQ-BROWSER-WS-002).

THE preservation signal SHALL exclude the conversation being deleted when
enumerating live owners. The cascade runs before the deleted conversation's
terminal-state write, so it still reads non-terminal; excluding it is what lets
the scope tear down when it is the last live owner.

A live owner is a conversation, RECORDED IN THE DATABASE, that is BOTH
non-terminal in state AND not `archived`. Liveness SHALL be determined from
the persisted conversation rows, NOT from the set of live runtime handles: a
conversation can be non-terminal in the DB yet hold no runtime handle (after a
server restart or runtime eviction), and such a conversation is still a live
owner. Enumerating only handles would let the cascade tear down a scope whose
surviving owner is a handle-less, non-terminal conversation — destroying its
shared worktree, branch, and processes. An archived conversation SHALL NOT
count as a live owner even while its state row still reads non-terminal:
archiving a chain archives its earlier members before the leaf's cleanup
cascade runs, so counting one as live would preserve the shared `WorkScope`
and leak its processes and tombstones.

**Rationale:** A continuation is not the only way a scope outlives one
conversation. A Work-mode sub-agent inherits its parent's `conv_mode` and so
resolves to the parent's `WorkScope`, but has no continuation. Preserving only
on a continuation SIGKILLs the still-open parent's processes (and tears down its
tmux server, terminal, and browser session, which share the call site) the
instant the sub-agent is deleted — an asymmetry the agent experiences as the
runtime randomly forgetting work. The correct signal is "is the scope still
owned by a live conversation other than this one," which a continuation
satisfies as one case among several.

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

**Rationale:** Unchanged for run calls. Extended for the new handle
operations so the UI has a sensible display for non-run calls.

---

## Configuration Constants

| Name | Default | Description |
|---|---|---|
| `MAX_WAIT_SECONDS` | 900 | Upper bound on `wait_seconds` per call |
| `RING_BUFFER_BYTES` | 4 MB | Per-handle live ring buffer size (bounds retained complete-line bytes, not the reported `output_bytes` total) |
| `MAX_PARTIAL_BYTES` | 64 KiB | Bound on the trailing un-newlined partial line; an append that would exceed it flushes the partial as a complete line |
| `PARTIAL_IDLE_FLUSH_SECONDS` | 10 | Reader idle interval after which a non-empty partial line is flushed as a complete line |
| `LIVE_HANDLE_CAP` | 8 | Per-WorkScope cap on `running` handles |
| `KILL_RESPONSE_TIMEOUT_SECONDS` | 30 | After signal sent, wait this long for exit before returning `kill_pending_kernel` |
| `SHUTDOWN_KILL_GRACE_SECONDS` | 2 | Time Phoenix waits at shutdown for SIGKILL'd groups to exit |
| `TOMBSTONE_TAIL_LINES` | 2000 | Lines retained in `final_tail` after exit demotion |
| `DEFAULT_PEEK_LINES` | 200 | Lines returned when peek has no read modifier |
| `MAX_LABEL_LENGTH` | 64 | Soft cap on `label` length; over-cap labels rejected with `error: "label_too_long"` |
| `DEFAULT_WAIT_SECONDS` | 30 | Default `wait_seconds` when omitted |
