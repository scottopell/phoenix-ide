# Bash Tool — Executive Summary

## Requirements Summary

The bash tool executes shell commands as pipe-backed children of the Phoenix
process. Commands run via `bash -c` with combined stdout/stderr captured into
a per-handle ring buffer; no TTY is attached. The agent specifies how long it
wants to block via `wait_seconds`; if the command exits in time, the response
carries the exit code and final output. If `wait_seconds` elapses first, the
response returns a **handle** (`status: "still_running"`) and the process keeps
running. The handle supports `peek` (current ring buffer state),
`wait` (block again for the existing process), and `kill` (signal-and-wait,
with automatic TERM→KILL escalation). On process exit, the live ring is
demoted to a compact tombstone retained until the conversation is hard-deleted.
On Phoenix process restart, in-flight handles are persisted as
`lost_in_restart` tombstones via SQLite; the agent receives a structured
"handle was lost at <timestamp>" response rather than a bare not-found.

A per-conversation cap of 8 live handles is enforced with an explicit
`handle_cap_reached` error listing existing handles — no silent eviction.
Persistence across Phoenix or system restart belongs to the separate
`tmux` tool (see `specs/tmux-integration/`); this tool is "cheap and
ephemeral, with a graceful failure mode."

## Technical Summary

`BashTool` is a stateless `Tool` reached via `ToolContext.bash_handles()`,
mirroring the existing browser-session pattern. The handle registry holds
per-conversation maps of live handles and tombstones; a SQLite shadow table
(`bash_tombstones`) records every handle's lifecycle and is reconciled on
Phoenix startup to convert orphaned `running` rows to `lost_in_restart`.

A live handle owns a 4MB byte-bounded ring buffer with monotonic per-line
offsets; reader tasks split incoming pipe bytes on newlines and append to
the ring under a mutex. The waiter task observes process exit, atomically
swaps the handle's state from `Live` to `Tombstoned` via `ArcSwap` (preserving
the last 2000 lines as `final_tail`), and pulses `tokio::sync::Notify` so any
in-flight wait calls return.

Spawn races the wait window against the exit signal in a `tokio::select!`;
peek is a snapshot read of the current state; wait blocks the same way as
spawn but on an existing handle and returns the *same* handle id on
re-timeout (no handle proliferation). Kill sends a signal to the process
group leader (set via `pre_exec` setpgid), waits up to 5s for graceful exit,
and auto-escalates TERM→KILL with the action surfaced in the response.

Command safety checks (`brush-parser` AST walk for blind git-add, force-push,
dangerous rm) and Landlock enforcement for Explore mode are unchanged from the
prior revision; both run on the spawn path only.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BASH-001:** Command Execution | 🔄 Rewrite | Spawn flow ports forward; capture goes to ring buffer instead of single string |
| **REQ-BASH-002:** Wait Semantics | ❌ New | Replaces kill-on-timeout with `wait_seconds` + `still_running` handle |
| **REQ-BASH-003:** Handle Operations | ❌ New | `peek` / `wait` / `kill` with handle ids; auto-escalation on kill |
| **REQ-BASH-004:** Ring Buffer + Read Semantics | ❌ New | Bytes-bounded ring, per-line offsets, caller-controlled read window |
| **REQ-BASH-005:** Live Handle Cap | ❌ New | Hard refusal with structured live-handles list |
| **REQ-BASH-006:** Tombstones and Process Exit | ❌ New | Demote-on-exit, retained until conversation hard-delete |
| **REQ-BASH-007:** Phoenix Restart Tombstones | ❌ New | SQLite shadow records, reconciliation on startup, `lost_in_restart` response |
| **REQ-BASH-008:** Error Reporting | 🔄 Rewrite | Stable error ids, structured envelopes; non-zero exit is not an error |
| **REQ-BASH-009:** No TTY Attached | 🔄 Carry-forward | Existing behaviour; documentation updated to point at `tmux` tool |
| **REQ-BASH-010:** Tool Schema and Mutual Exclusion | 🔄 Rewrite | New `cmd`/`peek`/`wait`/`kill` shape with `oneOf`; `mode` deprecated |
| **REQ-BASH-011:** Command Safety Checks | ✅ Carry-forward | `brush-parser` AST walk unchanged |
| **REQ-BASH-012:** Landlock Enforcement | 🔄 Renumbered | Was REQ-BASH-008; behaviour unchanged |
| **REQ-BASH-013:** Graceful Degradation Without Landlock | 🔄 Renumbered | Was REQ-BASH-009; behaviour unchanged |
| **REQ-BASH-014:** Stateless Tool with Per-Conversation Handle Registry | 🔄 Rewrite | Was REQ-BASH-010; tool stays stateless, registry reached via `ctx.bash_handles()` |
| **REQ-BASH-015:** Display Command Simplification | 🔄 Carry-forward + extension | Was REQ-BASH-011; new display labels for peek/wait/kill |

**Progress:** 0 of 15 implemented under the new spec; this revision is a
greenfield rewrite of the runtime portion. Carry-forward items (REQ-BASH-011,
-012, -013, -015) reuse the existing `bash_check.rs`, Landlock integration,
and display simplification logic; rewrite items require new code.

## Behavioural Specification

The corresponding Allium spec is `specs/bash/bash.allium`. It models:

- `Handle` entity with `running` → `exited | killed | lost_in_restart`
  transitions.
- `still_running` and `exited` as response variants on the wait window
  (transitions, not states).
- Invariants: per-conversation live-handle cap, conversation-scoped handle
  ownership, mutual exclusion of live ring vs final tail, monotonic line
  offsets, shadow record exists for every handle.
- Surface `AgentBashAccess` with structural conversation scoping and
  guarantees `HandleOwnership` and `NoSilentEviction`.

Open questions: none. All decisions resolved during the elicitation
conversation dated April 2026 (see the `Open questions` block at the bottom
of `bash.allium` for the full list).
