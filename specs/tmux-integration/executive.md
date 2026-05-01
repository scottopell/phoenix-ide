# Tmux Integration — Executive Summary

## What It Is

Per-conversation tmux server management plus an agent-facing `tmux` tool that
forwards subcommands to that server. The in-app terminal automatically
attaches to the conversation's tmux session whenever the tmux binary is
available; otherwise the existing direct-PTY behaviour from
`specs/terminal/` runs unchanged.

## Why It Exists

The bash tool deliberately offers no TTY, no persistence across Phoenix
process restarts, and a fixed memory budget per handle (see
`specs/bash/`). That covers the cheap-and-ephemeral case. The tmux
integration covers the persistent-and-attachable case — long-running dev
servers, interactive REPLs, anything that needs a real TTY or that should
survive a Phoenix restart. Reimplementing tmux's persistence and multi-
client protocol would be reinventing the wheel; integrating tmux as the
mechanism is mostly plumbing.

## Scope

**Included:**
- Per-conversation tmux server isolated via `-S <absolute-socket-path>`
  (kernel-level Unix-socket boundary; no parsing/whitelisting needed).
- Lazy server spawn on first operation; default session named `main`.
- Stale-socket detection (post-system-reboot recovery) — silent unlink
  and recreate, no in-pane breadcrumb.
- Conversation-hard-delete cascade (kills server, unlinks socket).
- Server survives Phoenix process restart; the OS keeps it alive
  independently.
- `tmux` agent tool — pure pass-through that prepends `-S <conv-sock>`
  before forwarding `args` verbatim to the tmux binary.
- In-app terminal auto-attaches via `tmux attach` when tmux is available;
  the existing single-attach-per-conversation constraint is preserved on
  both the tmux-attach and direct-PTY fallback paths.

**Explicitly excluded / deferred:**
- Multi-attach via tmux's native protocol — deferred (`TmuxMultiAttach`).
  Single-attach constraint stands on both paths.
- Stale-recovery user notification — deferred (`TmuxStaleRecoveryNotification`).
  Silent recreate is v1 behaviour; the `send-keys -l` breadcrumb proposed
  in the original draft was unsafe (writes to slave PTY stdin).
- Custom socket-path relocation (e.g., `$XDG_RUNTIME_DIR`); v1 hard-codes
  `~/.phoenix-ide/tmux-sockets/`.
- Per-conversation tmux resource quotas (max windows, max sessions).
- Cross-system-reboot persistence (no checkpoint/restore — out of scope).

## Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Isolation mechanism | `tmux -S <absolute-path>` per conversation | Kernel-socket boundary; no `TMUX_TMPDIR` env-var dance; agent's stray `-L`/`-S` triggers tmux usage error rather than escape |
| Tool surface shape | Pure pass-through (`tmux <args>`) | LLM already knows tmux from training; verb wrapper would drift from tmux's evolving CLI |
| Tool name | `tmux` | Matches what the tool literally is |
| Server spawn timing | Lazy on first operation | No idle servers for unused conversations |
| Default session name | `main` | Tmux's most-recent-session rule resolves argless `-t` to `main` |
| Lifetime | Conversation hard-delete only | Soft state (archive, close-tab) does not kill the server |
| Phoenix restart | Server survives | Core value-prop. The OS keeps tmux processes alive |
| System reboot | Detect + silent recreate | Stale socket file unlinked; no breadcrumb (would corrupt pane input) |
| In-app terminal | Auto-attach when tmux available; direct-PTY fallback otherwise | Free scrollback + persistence; preserves v1 behaviour where tmux is absent |
| Multi-attach | Deferred | UI design questions not v1-essential |
| Output capture | Separate stdout/stderr, 128 KB middle-truncate | Tmux subcommands emit structured CLI output; separation matters |
| Concurrency primitive | `RwLock<TmuxServer>` | Matches existing browser + bash patterns; ArcSwap was novel and not needed |
| ToolContext accessor | `async ctx.tmux() -> Result<Arc<RwLock<TmuxServer>>, TmuxError>` | Matches `ctx.browser()` and `ctx.bash_handles()` shapes |

## Build Order

1. **Registry plumbing** — `TmuxRegistry`, `TmuxServer`, socket-path
   resolution, ToolContext extension. Standalone — exercise via unit
   tests against a tmux binary on a tmpdir socket dir.
2. **Probe + lazy/stale spawn** — `probe()`, `ensure_live()`, the three
   spawn rules (NoSocket / DeadSocket / Live). Integration test:
   tear-and-recreate a socket file mid-test.
3. **`tmux` agent tool** — dispatch, subprocess invocation, output
   capture, timeout/cancel handling, response shaping. Integration test
   covering `new-window`, `capture-pane`, `send-keys`, `kill-window` end-
   to-end.
4. **Hard-delete cascade** — wire into bedrock's hard-delete handler
   alongside the bash handle cascade. *Depends on bedrock adding a
   `ConversationHardDeleted` event / cascade orchestrator hook.*
5. **Terminal attach branching** — `build_pty_exec_argv` switches between
   `tmux attach` and `$SHELL -i`. Single-attach constraint preserved.

## Bedrock Dependency

The hard-delete cascade requires bedrock to emit a
`ConversationHardDeleted` event (or expose a cascade-orchestrator hook)
that this spec — and `specs/bash/`, `specs/projects/` — can subscribe
to. At the time of this revision, bedrock has neither directly. The
cascade integration is gated on adding that hook.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-TMUX-001:** Per-Conversation Tmux Server (Socket Isolation) | ❌ Not Started | `-S <absolute-path>` selector |
| **REQ-TMUX-002:** Lazy Server Spawn | ❌ Not Started | Triggered by tmux tool call OR terminal open |
| **REQ-TMUX-003:** `tmux` Agent Tool — Pure Pass-Through | ❌ Not Started | New tool, pass-through dispatch |
| **REQ-TMUX-004:** In-App Terminal Auto-Attaches When Tmux Available | ❌ Not Started | Modifies `src/terminal/spawn.rs` argv selection; single-attach constraint preserved |
| **REQ-TMUX-005:** Server Survives Phoenix Process Restart | ❌ Not Started | Probe-based; no in-memory state to persist |
| **REQ-TMUX-006:** Stale Socket Detection (System Reboot Recovery) | ❌ Not Started | Silent unlink + respawn (no breadcrumb) |
| **REQ-TMUX-007:** Server Termination on Conversation Hard-Delete | ❌ Not Started | Cascade handler |
| **REQ-TMUX-008:** Conversation Soft-State Does Not Affect Server | ❌ Not Started | Explicit no-op |
| **REQ-TMUX-009:** Tool Description Communicates Two-Tier Persistence Model | ❌ Not Started | Static text in tool description |
| **REQ-TMUX-010:** Tool Cancellation and Output Limits | ❌ Not Started | Timeout, cancellation, middle-truncate |
| **REQ-TMUX-011:** Tool Surface Hardening — Phoenix-Injected Flag Authority | ❌ Not Started | Argument-list ordering |
| **REQ-TMUX-012:** Output Capture Format | ❌ Not Started | Separate stdout/stderr response shape |
| **REQ-TMUX-013:** Stateless Tool with Per-Conversation Server Registry | ❌ Not Started | Mirrors bash + browser registry patterns; matches `ctx.browser()` accessor shape |

**Progress:** 0 of 13 implemented; greenfield spec.

## Behavioural Specification

`specs/tmux-integration/tmux-integration.allium` models:

- `TmuxServer` entity with `not_running` → `live` → `gone` transitions.
- Three spawn rules covering each `ProbeOutcome` (no_socket / dead_socket /
  live).
- `tmux` tool dispatch and the three response variants (ok / timed_out /
  cancelled).
- Terminal attach with tmux available vs. direct-PTY fallback as sibling
  rules.
- Hard-delete cascade and soft-state no-op.
- Invariants: deterministic socket path, one server per conversation,
  socket path always set on live servers.
- Surfaces `AgentTmuxAccess` (with `SocketIsolation` and `NoStdinAccess`
  guarantees) and `UserTerminalTmuxBridging` (with `SeamlessFallback` and
  `SingleAttachPreserved` guarantees).
- Deferred entries documenting `TmuxMultiAttach`,
  `TmuxStaleRecoveryNotification`, `TmuxResourceQuotas`, and
  `TmuxSocketRelocation`.

Open questions: none. All decisions resolved during the elicitation
conversation dated April 2026 and confirmed by the panel review (see the
`Open questions` block at the bottom of `tmux-integration.allium`).

## Cross-Spec Cross-References

- `specs/bash/`: REQ-BASH-009 description text points at this tool for TTY
  / persistence / interactive needs. The two specs share the
  ToolContext-accessor pattern (`ctx.tmux()` mirrors `ctx.bash_handles()`).
- `specs/terminal/`: REQ-TERM-003's single-terminal-per-conversation
  constraint applies on both the tmux-attach and direct-PTY paths; this
  spec does not require any edit to the terminal spec.
- `specs/bedrock/`: hard-delete cascade gains a tmux step alongside bash
  and project cascades, gated on bedrock adding a
  `ConversationHardDeleted` event. No new bedrock states.
