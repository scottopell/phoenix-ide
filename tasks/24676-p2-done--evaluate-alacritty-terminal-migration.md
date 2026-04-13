---
created: 2026-04-13
priority: p2
status: done
artifact: specs/terminal/alacritty-evaluation.md
---

# evaluate-alacritty-terminal-migration

## Problem

The vendored `vt100` crate is unmaintained. The wezterm-term evaluation (task
24673) found that wezterm-term has its own divide-by-zero panic in sixel
handling, and that exit codes (OSC 133;D) are silently dropped — making it a
no-go for now.

`alacritty_terminal` is the remaining candidate: actively maintained (commits
days ago), lean dep tree (no image/rav1e/rayon), Apache-2.0 licensed. The
original rejection in task 24673 ("no unique value on semantic events") was
pre-wezterm-evaluation. Now the question is whether it serves the **stability**
goal — escaping the vendoring cycle — even without OSC 133.

## Scope

Same structure as the wezterm evaluation.

1. **Dependency shape** — add to `[dev-dependencies]`, measure new packages and
   binary size delta.
2. **API adapter** — `src/terminal/alacritty_parser.rs` wrapping the 6 methods.
3. **Proptests** — port both stress tests, run 10,000 cases each.
4. **OSC 133** — brief section confirming status and future feasibility.
5. **Write-up** — `specs/terminal/alacritty-evaluation.md` with go/no-go.
6. **Cross-reference** — update wezterm-evaluation.md to link the alacritty doc.

## Out of scope

- The migration itself.
- Any frontend changes.
