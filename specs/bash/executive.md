# Bash Tool — Executive Summary

## Requirements Summary

The bash tool executes shell commands as pipe-backed children of the Phoenix
process. Commands run via `bash -c` with combined stdout/stderr captured into
a per-handle ring buffer; no TTY is attached. The agent specifies how long it
wants to block via `wait_seconds`; if the command exits in time, the response
carries the exit code and final output. If `wait_seconds` elapses first, the
response returns a **handle** (`status: "still_running"`) and the process keeps
running. The handle supports `peek` (current ring buffer state),
`wait` (block again for the existing process), and `kill` (signal exactly
once, no auto-escalation; a `kill_pending_kernel` response covers the
D-state-hang case). On process exit, the live ring is demoted to a compact
in-memory tombstone retained until the conversation is hard-deleted or the
Phoenix process exits.

A per-WorkScope cap of 8 live handles is enforced with an explicit
`handle_cap_reached` error listing existing handles — no silent eviction.
Persistence across Phoenix or system restart belongs to the separate
`tmux` tool (see `specs/tmux-integration/`); this tool is "cheap and
ephemeral, with a graceful failure mode."

Phoenix sets `PR_SET_CHILD_SUBREAPER` at startup so descendants that escape
their original process group (double-forks, `setsid`-using daemons) reparent
to Phoenix rather than init; at shutdown, a kill-tree pass SIGKILLs every
live handle's process group. This replaces the prior draft's wrong
assumption that SIGHUP cascade would clean up children when Phoenix died.

## Technical Summary

`BashTool` is a stateless `Tool` reached via `ToolContext.bash_handles()`,
which mirrors the existing browser-session pattern: `async fn bash_handles(&self)
-> Result<Arc<RwLock<WorkScopeHandles>>, BashHandleError>`. The handle
registry holds per-`WorkScope` maps of live handles and tombstones — keyed by
`WorkScope` like the browser, tmux, and terminal registries, so a continuation
chain on one worktree shares one table. In-memory only — no SQLite shadow
store, no cross-restart persistence (the agent uses `tmux` for that).

A live handle owns a 4MB byte-bounded ring buffer with monotonic per-line
offsets; reader tasks split incoming pipe bytes on newlines and append to
the ring under a mutex. The waiter task observes process exit, swaps the
handle's `RwLock<Arc<HandleState>>` from `Live` to `Tombstoned` (preserving
the last 2000 lines as `final_tail` and recording `FinalCause` —
distinguishing `Exited`, `Killed` (Phoenix-initiated), `Signaled` (external
signal), and `KillPendingKernel`). A `tokio::sync::watch::channel` carries
the exit signal so in-flight wait calls observe the transition.

Spawn races the wait window against the exit signal in a `tokio::select!`;
peek is a snapshot read of the current state; wait blocks the same way as
spawn but on an existing handle and returns the *same* handle id on
re-timeout (no handle proliferation). The agent operation is `op="run"`;
the internal OS-level fork/exec is still called "spawn" where the
distinction matters. Optional `label` annotation is attached at run time
and echoed on every later response carrying the handle, plus on each
entry of `live_handles[]` in the cap-reached error.
re-timeout (no handle proliferation). Kill sends a signal to the process
group leader (set via `pre_exec` setpgid), waits up to
`KILL_RESPONSE_TIMEOUT_SECONDS` (30) for exit, and either returns the
terminal status or returns `kill_pending_kernel` for D-state hangs without
holding the response forever. The waiter task survives `kill_pending_kernel`
so a late-arriving exit still demotes correctly.

Command safety (`brush-parser` AST walk for blind git-add, force-push,
dangerous rm) is enforced by the permission seam (specs/permissions/) before
the bash tool runs, not inside the tool. In Explore, `SandboxedBashTool` routes
`op="run"` through a Phoenix child process that applies `nono` before execing
bash; filesystem reads are broad, worktree and Git metadata writes are denied,
task proposal writes are denied (task drafts use scoped non-bash proposal tools),
scratch/synthetic-home/platform-temp writes are allowed only when their roots do
not overlap protected repo/Git/Phoenix paths, per-command scratch is reaped at
terminal state, network is blocked, and unsupported hosts omit Explore bash
entirely.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BASH-001:** Command Execution | 🔄 Rewrite | Spawn flow ports forward; capture goes to ring buffer instead of single string |
| **REQ-BASH-002:** Wait Semantics | ❌ New | Replaces kill-on-timeout with `wait_seconds` + `still_running` handle; explicit-negation tool description |
| **REQ-BASH-003:** Handle Operations | ❌ New | `peek` / `wait` / `kill` with handle ids; no auto-escalation; `kill_pending_kernel` for D-state |
| **REQ-BASH-004:** Ring Buffer + Read Semantics | ❌ New | Bytes-bounded ring, per-line offsets, caller-controlled read window |
| **REQ-BASH-005:** Live Handle Cap | ❌ New | Hard refusal with structured live-handles list |
| **REQ-BASH-006:** Tombstones and Process Exit | ❌ New | Demote-on-exit, in-memory only, retained until conv hard-delete or Phoenix exit |
| **REQ-BASH-007:** Child Process Reaper | ❌ New | `PR_SET_CHILD_SUBREAPER` at startup + SIGKILL kill-tree at shutdown |
| **REQ-BASH-008:** Error Reporting | 🔄 Rewrite | Stable error ids, structured envelopes; non-zero exit is not an error |
| **REQ-BASH-009:** No TTY Attached | 🔄 Carry-forward | Existing behavior; tool description points at `tmux` |
| **REQ-BASH-010:** Tool Schema and Mutual Exclusion | 🔄 Rewrite (rev 3) | `op` discriminator with `run`/`peek`/`wait`/`kill`; `label` field added; legacy four-sibling inference, `mode` shim, `command` alias, and empty-string-as-absent tolerance retired (`deny_unknown_fields`); two narrow tolerances retained for active GPT default-fill (`since=0`, `lines+since`) |
| **REQ-BASH-011:** Command Safety Checks | 🔄 Relocated | Enforcement moved to the permission seam (specs/permissions/); `brush-parser` AST walk (`bash_check`) unchanged, now invoked by the seam |
| **REQ-BASH-012:** Explore `nono` Sandbox | ✅ Complete | `SandboxedBashTool` uses a Phoenix child-process launcher; the server never applies the irreversible sandbox to itself |
| **REQ-BASH-013:** Fail-Closed Explore Bash | ✅ Complete | Startup uses `nono::Sandbox::support_info()`; unsupported hosts omit bash from top-level Explore registries |
| **REQ-BASH-014:** Stateless Tool with Per-WorkScope Handle Registry | 🔄 Rewrite | Was REQ-BASH-010; tool stays stateless, registry reached via `ctx.bash_handles()` matching browser pattern |
| **REQ-BASH-WS-001:** Handle Registry Keyed by WorkScope | ❌ New | Registry keyed by `WorkScope`, not conversation id; handles survive the continuation boundary; symmetric with tmux/browser/terminal |
| **REQ-BASH-WS-002:** Hard-Delete Cascade Respects Inheritor Scope | ❌ New | `cascade_bash_on_delete` consults inheritor `WorkScope` and skips teardown on scope match, like `cascade_terminal/browser_on_delete` |
| **REQ-BASH-015:** Display Command Simplification | 🔄 Carry-forward + extension | Was REQ-BASH-011; new display labels for peek/wait/kill |

**Progress:** 0 of 17 implemented under the new spec; this revision is a
greenfield rewrite of the runtime portion. Carry-forward items (-012, -013,
-015) and the relocated REQ-BASH-011 reuse the existing `bash_check.rs`
(now invoked by the permission seam), `nono` launcher, and display
simplification logic; rewrite items require new code. The
`WorkScope`-keying requirements (REQ-BASH-WS-001, -WS-002) re-key the registry
from conversation id to `WorkScope` and bring the hard-delete cascade in line
with the other `WorkScope`-keyed resources.

## Bedrock Dependency

REQ-BASH-006's hard-delete cascade is wired through
`cascade_bash_on_delete`, called directly from the bedrock hard-delete
handler per REQ-BED-032. The cascade receives the deleted conversation's
`WorkScope` and the inheritor's `WorkScope` (REQ-BASH-WS-002) and skips
teardown on scope match — the same signature the terminal and browser
cascades take. The orchestrator runs as a sequence of direct function calls;
there is no event-bus / subscriber-registration pattern. Implementing this
requires REQ-BED-032 to be in place — which it is, as part of the same spec
set under review.

## Behavioural Specification

The corresponding Allium spec is `specs/bash/bash.allium`. It models:

- `Handle` entity with `running` → `exited | killed | kill_pending_kernel`
  transitions, plus the `kill_pending_kernel → exited|killed` late-
  arriving exit paths.
- Response shape: `status: "tombstoned"` for any peek/wait/kill on a
  finished handle, with `final_cause` carrying the underlying terminal
  cause; `status: "still_running"` for run/wait responses where the
  wait window elapsed; `status: "kill_pending_kernel"` for handles
  whose process didn't exit within the kill response window.
- Reaper rules: `PhoenixSetsSubreaperOnStartup` and
  `PhoenixKillsLiveHandlesOnShutdown` cover the new
  `PR_SET_CHILD_SUBREAPER` / kill-tree machinery.
- Invariants: per-`WorkScope` live-handle cap, `WorkScope`-scoped handle
  ownership, monotonic line offsets, kill_pending_kernel implies the
  process is alive (pid/pgid available for re-signalling).
- Surface `AgentBashAccess` with structural `WorkScope` scoping and
  guarantees `HandleOwnership`, `NoSilentEviction`, and
  `NoAutoEscalation`.

The deferred entry `BashHandleCrossRestartPersistence` documents the
explicit decision to drop the SQLite shadow store and `lost_in_restart`
machinery from v1, including the panel-review reasoning that led to it.

Open questions: none.
