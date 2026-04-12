---
created: 2026-04-12
priority: p2
status: done
artifact: pending
---

# terminal-hud-state-model-polish

## Problem

The terminal HUD that shipped in task 24664 has three rough edges
that became apparent in real-world testing:

1. **OSC 133 detection doesn't fire for powerlevel10k users.** p10k's
   source gates shell integration on `$ITERM_SHELL_INTEGRATION_INSTALLED`,
   not on `POWERLEVEL9K_TERM_SHELL_INTEGRATION` (which is a phantom var
   that does nothing). Phoenix doesn't set the iTerm2 flag, so p10k
   silently refuses to emit OSC 133. Even when enabled, p10k only emits
   A and B markers — not C or D — so command lifecycle tracking is
   incomplete without a supplementary snippet.
2. **Sampler fallback is ugly with powerline prompts.** The buffer
   sampler left-truncates `╭─ ~/path    ✔` + `╰─ ❯` into bits like
   `… ✔ ╰─` with the cwd scrolled off-screen. Worse than useless.
3. **State model is confusing.** `absent idle` and `detected idle` look
   identical. The dot color scheme (green=idle, amber=running,
   red=disconnected) doesn't communicate success vs failure once a
   command completes. No visible distinction between "shell integration
   working" and "shell integration absent".
4. **Keyboard shortcut half-broken.** `Ctrl+backtick` is guarded against
   any input focus, which means you can't collapse the terminal while
   the chat textarea is focused — the most common case.
5. **No close/collapse button.** Only collapse mechanisms are drag,
   double-click, and the broken keyboard shortcut.
6. **Disconnected state is a red dot.** When the shell exits or the WS
   dies, the terminal panel looks "mostly fine" with a red dot. Nothing
   indicates the panel is dead or offers a way to revive it.

## Scope

### Backend

Set two env vars in the PTY spawn (REQ-TERM-002 env construction):

- `ITERM_SHELL_INTEGRATION_INSTALLED=Yes` — triggers p10k's A/B emission
- `TERM_PROGRAM=phoenix-ide` — future-proof signal for other prompts

### Detection contract change

Promote `unknown → detected` only when an OSC 133 `C` marker is seen.
A, B, and D alone are not sufficient. Rationale: the purpose of
detection in the HUD is command lifecycle tracking; A+B alone (as p10k
emits) gives us nothing actionable.

Spec: `ShellIntegrationDetected` rule fires on `C` only.

### Command lifecycle change

`NewPromptClearsLastCompletedCommand` rule is removed. The clearing of
`last_completed_command` now happens inside `CommandExecutionStarted`
(on `C`). Rationale: the prior model cleared on the next prompt (`A`),
which fires ~50ms after `D` — the success indicator was invisible. The
new model clears on the next command start, giving the user the entire
"reading result + thinking + typing" window to see the ✓/✗.

A and B become pure no-ops (kept in the surface for forward compat).

### HUD state model (frontend)

Remove the buffer sampler + byte-activity heuristic entirely. The HUD
renders five states:

| State | Prompt text | Dot |
|---|---|---|
| unknown (first 5s) | dimmed "Terminal" label | dimmed gray, pulsing |
| absent | dim conversation cwd | neutral gray, static |
| detected idle / last-succeeded | cwd + optional ✓ cmd (dur) | green |
| detected running | cwd + $ cmd + live dur | blue, pulsing |
| detected last-failed | cwd + ✗ cmd (exit N) | red |
| disconnected | dead label | entire panel in dead treatment, clickable to reconnect |

### UX fixes

- **Ctrl+backtick**: guard limited to xterm focus (not any input)
- **Close button**: `⌄` chevron in the expanded header, mirrors the
  `⌃` expand chevron on the collapsed strip
- **Disconnected reconnect**: clicking a disconnected panel closes the
  dead WebSocket and spawns a new one (triggers a fresh PTY via the
  backend's existing spawn path)

### Out of scope

- Auto-injecting full OSC 133 hooks via ZDOTDIR (still deferred)
- Documenting the p10k phantom variable upstream
- Real iTerm2 shell integration compatibility beyond the trigger var

## Deliverables

1. `src/terminal/spawn.rs` — 2 env var additions
2. `ui/src/components/TerminalPanel.tsx` — state machine, HUD
   render, keyboard shortcut, close button, disconnect handling
3. `ui/src/index.css` — dot color variants, disconnected panel
   treatment, close chevron
4. `specs/terminal/terminal.allium` — rule updates
5. `specs/terminal/requirements.md` — REQ-TERM-015, REQ-TERM-016
   language tweaks + REQ-TERM-019 for disconnected state UX
6. This task → `done`

## Related

- Parent task: 24664 (OSC 133 shell integration v1)
- Parent commit: 33fe129
