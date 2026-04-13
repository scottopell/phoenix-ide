---
created: 2026-04-13
priority: p1
status: ready
artifact: specs/terminal/requirements.md, specs/terminal/terminal.allium, src/terminal/session.rs, src/terminal/ws.rs
---

# Revisit shell integration detection lock-in policy

## Summary

The current policy locks `shell_integration_status` to `absent` permanently after a 5-second
detection window expires, with no recovery path except opening a new terminal. This produces
silent, permanent failures for a substantial fraction of real user environments.

## Context

The 5-second window starts at PTY spawn, not at first user interaction. Three concrete failure
modes were identified in panel review:

**Slow shell startup**: oh-my-zsh + plugins + nvm/rbenv/pyenv chains take 3-8 seconds on
real laptops. The detection window can expire before `.zshrc` finishes loading and registers
the `preexec`/`precmd` hooks that emit OSC 133;C. The user's integration is present and
correct but the window misses it.

**p10k instant-prompt** (the modal macOS zsh setup): `POWERLEVEL9K_INSTANT_PROMPT=verbose`
pre-renders the prompt before `.zshrc` finishes — hooks added later may not fire in the
detection window. Powerlevel10k is the most-installed zsh theme; this is not an edge case.

**tmux detach/reattach** (the sharpest variant): When a user reattaches a tmux session, the
child shell PID has not changed, so no re-detection fires. But if a command was in-flight at
detach, its OSC 133;D marker may never arrive — leaving `current_capture` permanently open,
blocking finalization for the remainder of the session. The ring buffer gets a stuck record
that never resolves. No error is surfaced.

The current "lock at 5s" policy was chosen for simplicity (monotonic state, no HUD flipping)
but the tradeoff underweights the real-world failure rate.

## Acceptance Criteria

- [ ] Decide: re-arm detection on each new command attempt vs. soften the window vs. detect
      per-session-reconnect vs. another mechanism. Document the decision in requirements.md
      and terminal.allium.
- [ ] Handle the stuck-capture case on reconnect: when a new WebSocket connection opens for
      a conversation that already has `current_capture = Some(...)`, clear the in-flight
      capture and log at debug level. A command that started before detach cannot be finalized.
- [ ] Define and implement recovery UX: what does the user see when they have an existing
      terminal in `absent` state that could now be re-detected? Options: (a) reconnect button
      that spawns a fresh PTY, (b) extend detection window to first actual command, (c) silent
      per-command re-arm.
- [ ] Add test coverage for the tmux detach/reattach scenario (reconnect with open
      `current_capture`).
- [ ] Update terminal.allium: revise `ShellIntegrationStatusMonotonic` invariant if the
      policy changes, or add a new rule for reconnect-clearing.

## Notes

The tmux stuck-capture fix is independently actionable regardless of which detection policy
is chosen — a reconnect should always clear in-flight state. Consider splitting that into a
fast-follow if the broader policy decision takes longer to resolve.

p10k workaround: `POWERLEVEL9K_INSTANT_PROMPT=off` or adding the OSC 133 snippet before the
p10k init block in `.zshrc` works around the issue. The shell integration setup-assist
(REQ-TERM-020) should detect p10k and include this guidance.
