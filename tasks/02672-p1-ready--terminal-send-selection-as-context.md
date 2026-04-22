---
created: 2026-04-22
priority: p1
status: ready
artifact: ui/src/components/TerminalPanel.tsx
---

# Terminal: keyboard shortcut to send selected text as LLM context

## Summary

When the user has text selected in the terminal panel, a keyboard shortcut
should pull that selection into the LLM input (e.g. the message composer's
draft) so they can immediately ask a follow-up about a terminal output
snippet without manual copy/paste and context-switch.

## Context

Today the only way to reference something from the terminal in an LLM
message is: mouse-select, Cmd-C, click/tab to the input, paste, manually
re-type or quote it. That breaks flow every time something interesting
lands in the terminal (a stack trace, a failing command, a grep hit).

xterm.js exposes `terminal.getSelection()` and `terminal.hasSelection()`,
so the read side is cheap. The composer already accepts programmatic
draft updates (see `useDraft` + InputArea in `ui/src/components/`).

Surfaces involved:
- `ui/src/components/TerminalPanel.tsx` — owns the xterm instance and the
  selection. Natural home for the key-capture.
- InputArea / message composer — consumes the selection as a draft update.

## Open design questions (resolve before implementing)

- **Shortcut**: proposed `Cmd/Ctrl+Shift+L` ("to LLM") to avoid collision
  with xterm's own bindings and OS-level text actions. Alternative:
  `Cmd/Ctrl+Enter` when terminal has focus. Pick one and document it.
- **Focus requirement**: does the shortcut work only when the terminal has
  keyboard focus, or globally while the panel is expanded? Recommend
  "terminal focused only" — predictable and avoids input-composer shadowing.
- **Formatting on insert**: wrap in a triple-backtick fence with a label
  like `From terminal:` on the line above? Append to the existing draft
  with a blank line separator, or replace? Recommend append-with-fence so
  the user's in-flight draft isn't clobbered.
- **Empty selection**: no-op silently, or do something (grab visible
  buffer, toast, etc.)? Recommend silent no-op.
- **Collapsed panel**: when the terminal panel is collapsed, the shortcut
  either does nothing or expands-and-captures. Default to nothing.

## Acceptance Criteria

- [ ] With text selected in the terminal and the terminal focused,
      pressing the chosen shortcut inserts the selection into the
      message composer's draft, fenced and clearly distinguishable from
      the user's typing.
- [ ] The user's existing draft text is preserved (new content appended,
      not replaced).
- [ ] Empty-selection case is defined and implemented (default: no-op).
- [ ] Shortcut does NOT fire outside the terminal-focused context
      (e.g. while typing in the message composer).
- [ ] Works regardless of integration status (OSC 133 detection on/off)
      and regardless of whether a command is currently running.
- [ ] Manual QA: verify in both expanded and collapsed states; confirm
      no regression in existing terminal keybindings or xterm copy behavior.

## Notes

- Worth checking whether xterm.js's own Ctrl/Cmd+Shift+C copy binding
  collides or needs to be disabled when we take Ctrl/Cmd+Shift+L.
- If we go the `Cmd+Enter`-while-focused route, need to be careful not to
  interfere with shells that bind Ctrl+M / Enter meaningfully.
- Consider whether the inserted fence should carry a source hint the
  LLM can use (e.g. "From terminal in cwd `/path`, shell `zsh`") so the
  model knows it's reading terminal output rather than user-composed prose.
