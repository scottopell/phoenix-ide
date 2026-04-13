---
created: 2026-04-13
priority: p3
status: ready
artifact: pending
---

# vte-upstream-prompt-mark-handler

## Problem

`alacritty_terminal` uses `vte 0.15` for VT sequence parsing.  The `vte` crate's
`ansi::Handler` trait has no `prompt_mark()` method for OSC 133
(FinalTerm/shell-integration) sequences — all four markers (A/B/C/D) fall
through to an `unhandled()` debug log.

Adding `fn prompt_mark()` to `Handler` (defaulting to no-op) and a dispatch arm
in `osc_dispatch` for `b"133"` would let `alacritty_terminal` surface OSC 133
events through `EventListener::send_event`, unlocking the server-side command
lifecycle capture and exit-code inspection needed for the "run and wait" agent
tool pattern.

OSC 133 is now standard: supported by zsh, bash, fish, iTerm2, WezTerm, Ghostty,
Windows Terminal, and all major shells out of the box.

## Scope

1. Fork / clone `vte` (crates.io, Apache-2.0 / MIT).
2. Add to `ansi::Handler` trait:
   ```rust
   fn prompt_mark(&mut self, _kind: PromptKind, _aid: Option<&str>) {}
   ```
   where `PromptKind` covers A (prompt start), B (prompt end/input start),
   C (command start/output start), D (command finished + exit code).
3. Wire `osc_dispatch` for `params[0] == b"133"` to call `self.handler.prompt_mark(...)`.
4. Open PR to `alacritty/vte` (or `alacritty/alacritty` if vte is inline).
5. Once merged and tagged, update our `alacritty_terminal` pin.

## Why now

Discovered during the alacritty_terminal migration evaluation (task 24676).
Filing upstream promptly while the context is fresh maximises the chance of
a quick review — alacritty maintainers are active.

## Out of scope

- The phoenix-side handler implementation (separate task after PR lands).
- Changing the OSC 7 (CWD) story.
