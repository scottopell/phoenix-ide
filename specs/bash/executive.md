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

A per-conversation cap of 8 live handles is enforced with an explicit
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
-> Result<Arc<RwLock<ConversationHandles>>, BashHandleError>`. The handle
registry holds per-conversation maps of live handles and tombstones,
in-memory only — no SQLite shadow store, no cross-restart persistence (the
agent uses `tmux` for that).

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
re-timeout (no handle proliferation). Kill sends a signal to the process
group leader (set via `pre_exec` setpgid), waits up to
`KILL_RESPONSE_TIMEOUT_SECONDS` (30) for exit, and either returns the
terminal status or returns `kill_pending_kernel` for D-state hangs without
holding the response forever. The waiter task survives `kill_pending_kernel`
so a late-arriving exit still demotes correctly.

Command safety checks (`brush-parser` AST walk for blind git-add,
force-push, dangerous rm) and Landlock enforcement for Explore mode are
unchanged from the prior revision; both run on the spawn path only.

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
| **REQ-BASH-010:** Tool Schema and Mutual Exclusion | 🔄 Rewrite | New `cmd`/`peek`/`wait`/`kill` shape with `oneOf`; `mode` deprecated with explicit removal version |
| **REQ-BASH-011:** Command Safety Checks | ✅ Carry-forward | `brush-parser` AST walk unchanged |
| **REQ-BASH-012:** Landlock Enforcement | 🔄 Renumbered | Was REQ-BASH-008; behavior unchanged |
| **REQ-BASH-013:** Graceful Degradation Without Landlock | 🔄 Renumbered | Was REQ-BASH-009; behavior unchanged |
| **REQ-BASH-014:** Stateless Tool with Per-Conversation Handle Registry | 🔄 Rewrite | Was REQ-BASH-010; tool stays stateless, registry reached via `ctx.bash_handles()` matching browser pattern |
| **REQ-BASH-015:** Display Command Simplification | 🔄 Carry-forward + extension | Was REQ-BASH-011; new display labels for peek/wait/kill |

**Progress:** 0 of 15 implemented under the new spec; this revision is a
greenfield rewrite of the runtime portion. Carry-forward items (REQ-BASH-011,
-012, -013, -015) reuse the existing `bash_check.rs`, Landlock integration,
and display simplification logic; rewrite items require new code.

## Bedrock Dependency

The hard-delete cascade in REQ-BASH-006 requires bedrock to emit a
`ConversationHardDeleted` event (or expose a cascade-orchestrator hook) that
this spec — and `specs/tmux-integration/`, `specs/projects/` — can subscribe
to. At the time of this revision, bedrock has neither directly. The cascade
integration is gated on bedrock adding that hook.

## Behavioural Specification

The corresponding Allium spec is `specs/bash/bash.allium`. It models:

- `Handle` entity with `running` → `exited | killed | signaled |
  kill_pending_kernel` transitions, plus the `kill_pending_kernel → killed`
  late-arriving exit path.
- `still_running` and `exited` as response variants on the wait window
  (transitions, not states).
- Reaper rules: `PhoenixSetsSubreaperOnStartup` and
  `PhoenixKillsLiveHandlesOnShutdown` cover the new
  `PR_SET_CHILD_SUBREAPER` / kill-tree machinery.
- Invariants: per-conversation live-handle cap, conversation-scoped handle
  ownership, monotonic line offsets, signal-killed handles have null
  exit_code.
- Surface `AgentBashAccess` with structural conversation scoping and
  guarantees `HandleOwnership`, `NoSilentEviction`, and `NoAutoEscalation`.

The deferred entry `BashHandleCrossRestartPersistence` documents the
explicit decision to drop the SQLite shadow store and `lost_in_restart`
machinery from v1, including the panel-review reasoning that led to it.

Open questions: none. All decisions resolved during the elicitation
conversation dated April 2026 and confirmed by the panel review (see the
`Open questions` block at the bottom of `bash.allium` for the full list).
