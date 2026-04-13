---
created: 2026-04-12
priority: p2
status: done
artifact: specs/terminal/wezterm-evaluation.md
---

# evaluate-wezterm-term-migration

## Problem

The vendored `vt100` crate has been accumulating bugs. We've patched
multiple issues over time (narrow-terminal underflow, tiny-terminal wide
char panics, and now the `restore_cursor` stale-cursor panic from task
24668). The upstream `vt100` project has been effectively unmaintained
for ~2 years — no release cut, maintainer silent on issues. Every patch
we apply is tech debt we carry with no upstream path.

Beyond the stability concern, richer agent terminal tools (OSC 133
command lifecycle capture server-side, "run and wait", exit code
inspection, etc.) need a terminal parser that exposes more semantic
hooks than vt100 does. wezterm's `wezterm-term` / `termwiz` provides
OSC 133 parsing and shell integration semantics as first-class features,
which would save us writing our own handler layer.

## Scope

**This is an evaluation task, not a migration task.** Ship a spike that
answers the go/no-go question with concrete evidence.

1. **Dependency shape**: add `wezterm-term` as a git or crates dep in a
   throwaway branch. Document:
   - Whether it pulls in unreasonable transitive deps
   - Whether it's stable enough to pin
   - License compatibility with the rest of Phoenix
   - Binary size impact on the release build

2. **API mapping**: write a small adapter module `src/terminal/wezterm_parser.rs`
   that exposes the same subset of operations Phoenix currently uses from
   vt100:
   - `new(rows, cols, scrollback)`
   - `process(&[u8])`
   - `set_size(rows, cols)`
   - `screen().size() -> (rows, cols)`
   - `screen().contents() -> String` (for the `read_terminal` agent tool)
   - `screen().cursor_position() -> (u16, u16)`

   Don't ship the adapter yet. Just prove it's possible and note the
   rough shape.

3. **Replay the proptest corpus**: port the existing
   `prop_parser_stress_resize_then_draw` and
   `prop_parser_stress_tiny_terminals` proptests to run against the
   wezterm adapter as well. Run 10000 cases each. Any panics or
   assertion failures are go/no-go data.

4. **OSC 133 access path**: verify that wezterm-term exposes the
   prompt/command lifecycle events (whatever its equivalent to OSC 133
   parsing is) in a usable way — not just internal. This is the main
   feature-unlock justification for the migration.

5. **Write up findings**: a short design doc
   `specs/terminal/wezterm-evaluation.md` with:
   - Dependency report
   - API comparison (vt100 vs wezterm-term)
   - Proptest results across both
   - OSC 133 access story
   - Migration effort estimate (LOC, files touched, risks)
   - **Recommendation**: migrate, don't migrate, or wait for more data

## Out of scope

- The migration itself. That's a follow-up task contingent on the
  evaluation's recommendation.
- Swapping xterm.js or any other frontend component — this task is
  purely about the backend parser.
- Evaluating alacritty_terminal as an alternative. Considered earlier
  and rejected: alacritty_terminal is stable but doesn't offer unique
  value over vt100 on the feature axes we care about (semantic events).
  wezterm-term is the only backend we're considering.

## Why not just keep patching vt100

Each patch extends our maintenance surface without moving the project
forward. The recent bugs have been real (would have panicked in
production given the right byte sequence), not pedantic. And we have
no escape hatch when the next bug is harder to patch — the upstream
isn't accepting PRs.

## Related

- Task 24668 (the vt100 bug that triggered this evaluation)
- `specs/terminal/terminal.allium` — currently references the vt100
  Parser entity; the evaluation should note whether the spec needs
  changes if migration lands
