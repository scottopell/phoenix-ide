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
- Per-conversation tmux server isolated via `-L <socket-path>` (kernel-
  level Unix-socket boundary; no parsing/whitelisting needed).
- Lazy server spawn on first operation; default session named `main`.
- Stale-socket detection (post-system-reboot recovery) with breadcrumb
  rendering on the next attach.
- Conversation-hard-delete cascade (kills server, unlinks socket).
- Server survives Phoenix process restart; the OS keeps it alive
  independently.
- `tmux` agent tool — pure pass-through that prepends `-L <conv-sock>`
  before forwarding `args` verbatim to the tmux binary.
- In-app terminal auto-attaches via `tmux attach` when tmux is available;
  multiple attaches per conversation are supported.

**Explicitly excluded / deferred:**
- Custom socket-path relocation (e.g., `$XDG_RUNTIME_DIR`); v1 hard-codes
  `~/.phoenix-ide/tmux-sockets/`.
- UI surfaces beyond the in-pane breadcrumb for stale recovery.
- Per-conversation tmux resource quotas (max windows, max sessions).
- Cross-system-reboot persistence (no checkpoint/restore — out of scope).

## Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Isolation mechanism | `tmux -L <conv-sock>` per conversation | Kernel-socket boundary; no parsing/whitelisting required. Agents cannot reach other servers. |
| Tool surface shape | Pure pass-through (`tmux <args>`) | LLM already knows tmux from training; verb wrapper would drift from tmux's evolving CLI. |
| Tool name | `tmux` | Matches what the tool literally is. |
| Server spawn timing | Lazy on first operation | No idle servers for unused conversations. |
| Default session name | `main` | Tmux's most-recent-session rule resolves argless `-t` to `main`. |
| Lifetime | Conversation hard-delete only | Soft state (archive, close-tab) does not kill the server. |
| Phoenix restart | Server survives | Core value-prop. The OS keeps tmux processes alive. |
| System reboot | Detect + recreate fresh | Stale socket file unlinked; breadcrumb in next attach. |
| In-app terminal | Auto-attach when tmux available; direct-PTY fallback otherwise | Free scrollback + persistence; preserves v1 behaviour where tmux is absent. |
| Multi-attach | Permitted on tmux path | Tmux's native multi-client protocol; share state across browser tabs. |
| Output capture | Separate stdout/stderr, 128 KB middle-truncate | Tmux subcommands emit structured CLI output; separation matters. |

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
   alongside the bash handle cascade.
5. **Terminal attach branching** — `build_pty_exec_argv` switches between
   `tmux attach` and `$SHELL -i`. Multi-attach validation.
6. **Stale-recovery breadcrumb** — `send-keys -l` injection before the
   next attach receives output.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-TMUX-001:** Per-Conversation Tmux Server (Socket Isolation) | ❌ Not Started | New tool surface |
| **REQ-TMUX-002:** Lazy Server Spawn | ❌ Not Started | Triggered by tmux tool call OR terminal open |
| **REQ-TMUX-003:** `tmux` Agent Tool — Pure Pass-Through | ❌ Not Started | New tool, pass-through dispatch |
| **REQ-TMUX-004:** In-App Terminal Auto-Attaches When Tmux Available | ❌ Not Started | Modifies `src/terminal/spawn.rs` argv selection |
| **REQ-TMUX-005:** Multiple Terminal Clients Allowed Per Conversation | ❌ Not Started | Weakens existing terminal-spec single-client constraint on tmux path |
| **REQ-TMUX-006:** Server Survives Phoenix Process Restart | ❌ Not Started | Probe-based; no in-memory state to persist |
| **REQ-TMUX-007:** Stale Socket Detection (System Reboot Recovery) | ❌ Not Started | Probe + unlink + respawn + breadcrumb |
| **REQ-TMUX-008:** Server Termination on Conversation Hard-Delete | ❌ Not Started | Cascade handler |
| **REQ-TMUX-009:** Conversation Soft-State Does Not Affect Server | ❌ Not Started | Explicit no-op rule |
| **REQ-TMUX-010:** Tool Description Communicates Two-Tier Persistence Model | ❌ Not Started | Static text in tool description |
| **REQ-TMUX-011:** Tool Cancellation and Output Limits | ❌ Not Started | Timeout, cancellation, middle-truncate |
| **REQ-TMUX-012:** Tool Surface Hardening — Phoenix-Injected Flag Authority | ❌ Not Started | Argument-list ordering |
| **REQ-TMUX-013:** Output Capture Format | ❌ Not Started | Separate stdout/stderr response shape |
| **REQ-TMUX-014:** Stateless Tool with Per-Conversation Server Registry | ❌ Not Started | Mirrors bash + browser registry patterns |

**Progress:** 0 of 14 implemented; greenfield spec.

## Behavioural Specification

`specs/tmux-integration/tmux-integration.allium` models:

- `TmuxServer` entity with `not_running` → `live` → `gone` transitions.
- Three spawn rules covering each `ProbeOutcome` (no_socket / dead_socket /
  live).
- `tmux` tool dispatch and the three response variants (ok / timed_out /
  cancelled).
- Terminal attach with tmux available vs. direct-PTY fallback as sibling
  rules.
- Stale-recovery breadcrumb consumption on attach.
- Hard-delete cascade and soft-state no-op.
- Invariants: deterministic socket path, one server per conversation,
  socket path always set on live servers.
- Surfaces `AgentTmuxAccess` (with `SocketIsolation` and `NoStdinAccess`
  guarantees) and `UserTerminalTmuxBridging` (with `SeamlessFallback` and
  `MultiAttachOnTmuxPath` guarantees).

Open questions: none. All decisions resolved during the elicitation
conversation dated April 2026 (see the `Open questions` block at the
bottom of `tmux-integration.allium`).

## Cross-Spec Cross-References

- `specs/bash/`: REQ-BASH-009 description text points at this tool for TTY
  / persistence / interactive needs. The two specs share the
  ToolContext-accessor pattern (`ctx.tmux()` mirrors `ctx.bash_handles()`).
- `specs/terminal/`: REQ-TERM-NEW (additive update in that spec) defers to
  this spec for attach-vs-direct-PTY decisioning. The single-client
  constraint from `specs/terminal/` is preserved on the direct-PTY
  fallback only.
- `specs/bedrock/`: hard-delete cascade gains a tmux step alongside the
  existing bash and project cascades. No new bedrock states.
