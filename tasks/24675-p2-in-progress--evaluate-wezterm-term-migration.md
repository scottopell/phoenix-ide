---
created: 2026-04-13
priority: p2
status: in-progress
artifact: pending
---

# evaluate-wezterm-term-migration

## Plan

# Evaluate wezterm-term as vt100 Replacement

## Context

The vendored `vt100` crate (`vendor/vt100/`, 6 patches applied) is unmaintained upstream. Every bug we fix extends the maintenance surface with no upstream path. This is an **evaluation-only** spike to answer the go/no-go question with concrete evidence.

Phoenix uses a tiny API surface from vt100:
- `Parser::new(rows, cols, scrollback)`
- `parser.process(&[u8])`
- `parser.set_size(rows, cols)`
- `parser.screen().size() → (rows, cols)`
- `parser.screen().contents() → String`
- `parser.screen().cursor_position() → (u16, u16)`

## What to Do

### 1. Dependency Shape (add to Cargo.toml, do not ship)
Add `wezterm-term` (via crates.io or git) to `Cargo.toml` as a non-default feature or in a dev workspace. Measure and document:
- Full transitive dependency count and notable crates pulled in
- License compatibility with existing deps (all MIT/Apache-2.0)
- Binary size delta on a release build (`cargo build --release`, compare before/after)
- Whether it's pinnable to a stable release

### 2. API Adapter Draft — `src/terminal/wezterm_parser.rs`
Write a thin adapter that wraps `wezterm-term` and exposes the same 6-method API surface. This file is a **proof of concept** — do not wire it into the production code path. Annotate any API gaps or impedance mismatches found during mapping.

### 3. Port Proptests (10,000 cases each)
In `src/terminal/proptests.rs`, add parameterised variants of:
- `prop_parser_stress_resize_then_draw` → runs against `wezterm_parser.rs`
- `prop_parser_stress_tiny_terminals` → runs against `wezterm_parser.rs`

Run both with `PROPTEST_CASES=10000 cargo test`. Report: pass/fail, any panics or assertion failures, comparison to vt100 results.

### 4. OSC 133 Access Path
Examine the wezterm-term / termwiz API to determine:
- Does it expose OSC 133 prompt/command lifecycle events (A/B/C/D markers) as structured data?
- Is this accessible from outside the crate (public API), or internal only?
- What does the event structure look like — is it usable for the "run and wait" / exit-code-inspection agent tool pattern described in the task?

### 5. Evaluation Write-Up — `specs/terminal/wezterm-evaluation.md`
Produce a structured doc with:
- Dependency report (counts, licenses, binary size delta)
- API comparison table (vt100 method → wezterm-term equivalent)
- Proptest results (10,000 cases each parser, pass/fail summary)
- OSC 133 access story (yes/partial/no, with evidence)
- Migration effort estimate (LOC, files touched, risks)
- **Clear recommendation**: migrate / don't migrate / wait for more data — with reasoning

### 6. terminal.allium Note
Note in the evaluation doc whether `terminal.allium`'s `Parser` entity definition would need updating if migration lands (non-blocking, just a note).

## Out of Scope
- The migration itself
- Any frontend (xterm.js) changes
- Evaluating alacritty_terminal (already rejected)
- Wiring the adapter into production relay loop

## Acceptance Criteria
- [ ] `specs/terminal/wezterm-evaluation.md` exists with all 6 sections complete
- [ ] `src/terminal/wezterm_parser.rs` exists as a proof-of-concept adapter (not wired to production)
- [ ] Both proptests run 10,000 cases against the wezterm adapter with results documented
- [ ] OSC 133 access path answered definitively (yes/partial/no) with code references
- [ ] Recommendation is clear and evidence-backed
- [ ] `./dev.py check` passes (no new clippy/fmt regressions; adapter can be gated behind `#[cfg(test)]` or `#[allow(dead_code)]` as appropriate)
- [ ] Task status updated to `done` in frontmatter


## Progress

