---
created: 2026-04-12
priority: p2
status: ready
artifact: pending
---

# tmux-session-wrap-pilot

## Problem

Phoenix terminals are currently tied 1:1 to a WebSocket connection:
when the WS closes (navigation, reload, server restart, shell exit),
the PTY dies. This is fine for simple use cases but has three pain
points we've already hit or will hit:

1. **Cross-conversation navigation loses state.** Task 24666 (seeded
   conversations) uses Option A (keep xterm mounted, WS persistent)
   to preserve terminal state across collapse/expand within a
   conversation. But navigating to a different conversation still
   tears down the PTY. Users working across multiple conversations
   lose shell state repeatedly.

2. **Server restart kills everything.** Every ./dev.py restart (and
   presumably production deploys) kills all running terminals.

3. **The deferred spec called for this already.**
   `specs/terminal/terminal.allium` has an explicit deferred section:

   ```
   deferred TmuxIntegration.named_sessions
       Persistent named sessions; reconnect to existing session on WS
       open. Future path for users who want scrollback or session
       persistence.
   ```

   We've been saying "future" for a while. Time to pilot it.

## Proposed approach

**Pilot behind an opt-in flag**, don't default it until we've lived
with it for a week.

1. **Config**: `PHOENIX_USE_TMUX=1` env var at server startup, or
   a field on the project record. Off by default.

2. **Spawn semantics**: when the flag is set, instead of
   `spawn_pty() → $SHELL -i`, Phoenix runs:

   ```
   tmux new-session -d -s phoenix-<conv-id> $SHELL -i
   tmux attach-session -t phoenix-<conv-id>   # this is the PTY the browser sees
   ```

   Phoenix holds the attach-session PTY; the underlying shell runs
   inside a detached tmux session that survives the attach PTY dying.

3. **Session lifecycle**:
   - On conversation end (REQ-TERM-012,
     TerminalAbandonedWithConversation): kill the tmux session via
     `tmux kill-session -t phoenix-<conv-id>`.
   - On WS disconnect mid-conversation: leave the tmux session
     running. Next connect calls `tmux attach-session` again and the
     user sees the preserved state.
   - On Phoenix server start: enumerate any existing
     `phoenix-*` tmux sessions that don't match active conversations
     and kill them. Prevent accumulated orphans.

4. **Reconnect path**: the existing click-to-reconnect flow from task
   24665 already spawns a fresh WS on click. In tmux mode, that fresh
   WS calls `tmux attach-session` instead of `spawn_pty`, and the
   existing shell is there waiting.

5. **read_terminal tool** (REQ-TERM-011) still needs a screen parser
   to return cursor-aware current-screen text. tmux provides the raw
   byte stream but not the grid semantics. So this task is
   **orthogonal** to the parser question (task 24673): we still need
   vt100 or its replacement to drive the agent's read_terminal tool.

## Scope

- Rust backend: new `spawn_tmux()` alongside `spawn_pty()`, guarded by
  the config flag
- Session naming + cleanup logic
- Handle the `tmux` binary not being installed (graceful fallback to
  direct PTY with a log warning)
- Test manually: spawn, navigate away, navigate back, verify state
- Spec update: add REQ-TERM-0XX for the tmux integration surface and
  promote `TmuxIntegration.named_sessions` from deferred to active

## Out of scope (for this pilot)

- Promoting tmux to default. Ship it opt-in first; decide after soak.
- Using tmux features beyond session persistence (no tmux copy-mode
  integration, no status line, no pane layouts).
- Integration with a user's own tmux sessions. Phoenix uses a
  dedicated namespace (`phoenix-*`) and does not touch other sessions.
- Handling `tmux kill-server` from outside while a conversation is
  active. Log and treat as a disconnect.

## Related

- `specs/terminal/terminal.allium` — `deferred TmuxIntegration.named_sessions`
- Task 24665 (terminal HUD state model, click-to-reconnect)
- Task 24666 (seeded conversations; tmux would make sub-conversation
  terminals persistent across navigation, closing the gap the seed
  primitive noted as a limitation)
- Task 24673 (wezterm-term evaluation; orthogonal, both could ship
  independently)
