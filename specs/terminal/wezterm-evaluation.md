# wezterm-term Migration Evaluation

**Status:** Complete — go/no-go recommendation issued  
**Task:** 24673  
**Date:** 2026-04-13  
**Evaluator:** automated spike (see `src/terminal/wezterm_parser.rs`)

---

## Summary

**Recommendation: Don't migrate — yet.**

`wezterm-term` (and its only available crates.io fork, `tattoy-wezterm-term`) has a
divide-by-zero panic in `assign_image_to_cells` triggered by any Sixel/DCS byte
sequence on a tiny terminal (cols=1 or rows=1). The same proptest cases that caught
our six vt100 bugs immediately catch this. Migrating today would swap one set of
vendor patches for another — with the added cost of 116 new transitive packages
and ~6 MB of binary growth.

The OSC 133 story is also **weaker than expected**: exit codes (OSC 133;D) are
silently dropped in the current API. Prompt/command lifecycle zones are accessible
as cell semantic annotations, not as structured events — insufficient for the
"run and wait" agent tool pattern that was the main feature justification.

The path to `migrate` is clear and traceable (see §6), but requires the upstream
bug to be fixed first, or a second vendor patch.

---

## 1. Dependency Report

### Crate evaluated

| Property | Value |
|----------|-------|
| Crate | `tattoy-wezterm-term` 0.1.0-fork.5 |
| License | MIT |
| Source | crates.io (tattoy.org fork of wezterm/wezterm `term/`) |
| Upstream | `wezterm/wezterm` (main repo, not separately published on crates.io) |
| Stable? | The `tattoy` fork publishes stable semver versions; the wezterm monorepo does not |

**Why `tattoy-wezterm-term` and not `wezterm/wezterm` git?**  
The canonical `wezterm-term` lives inside the `wezterm/wezterm` workspace and is
not published separately on crates.io. Using it as a git dep would pull the entire
~40-crate monorepo. `tattoy-wezterm-term` is an independent crate.io publish with
the same code, used by the [Tattoy](https://github.com/tattoy-org/tattoy) terminal
renderer — a real production user.

### Transitive dependency count

| Metric | Before | After |
|--------|--------|-------|
| Total packages in `Cargo.lock` | 400 | 516 |
| New packages added | — | **116** |

### Notable new packages

| Package | Why it comes in | Impact |
|---------|-----------------|--------|
| `image` 0.25 | Hard dep of `wezterm-term` (non-optional) | Pulls in all codec stack |
| `rav1e` 0.8 | AV1 encoder via `image` → `ravif` | ~7 MB rlib |
| `rayon` 1.11 | Parallelism for AV1 encoding | Thread pool in binary |
| `gif`, `png`, `tiff`, `webp` | Image codecs via `image` | +several MB rlib |
| `fancy-regex` | `termwiz` regex engine | ~1 MB rlib |
| `phf` + friends | Perfect hash tables for terminal tables | Moderate |
| `terminfo` | Terminal database lookup | Small |
| `vtparse` 0.7 | VTE sequence parser (replaces `vte` 0.11) | ~comparable |
| `filedescriptor` | Unix fd abstraction | Small |

**The `image` crate is the key concern.** It is a mandatory (non-optional)
dependency of `wezterm-term`, and it unconditionally pulls in `rav1e` (AV1
encoder). Phoenix never encodes images, so all image codec code is dead — but
it still survives DCE: `rav1e` symbols appear in the stripped binary even in a
hello-world program that only calls `Terminal::new`.

### License compatibility

All 116 new packages are MIT or Apache-2.0 licensed. No GPL or proprietary
licenses introduced. Compatible with the existing stack.

### Binary size impact

| Binary | Size (stripped) | Notes |
|--------|-----------------|-------|
| `phoenix_ide` baseline | **24 MB** | Current production binary |
| `wezterm-test` (hello world, no deps) | 0.4 MB | Isolated baseline |
| `wezterm-test` (imports `Terminal`, minimal use) | **5.4 MB** | +5 MB from new deps |
| `phoenix_ide` + wezterm (estimated) | **~29 MB** | +5 MB, ~21% larger |

The 5 MB delta is almost entirely `rav1e` + image codecs. The terminal emulator
logic itself (`tattoy-wezterm-term`, `tattoy-termwiz`, etc.) is modest in size.

### Pinnable?

Yes. `tattoy-wezterm-term` follows semver on crates.io. Lock to `0.1.0-fork.5`
or a later stable version. Upstream `wezterm/wezterm` releases are infrequent but
tagged; the `tattoy` fork tracks them with patch versions.

---

## 2. API Comparison

| vt100 method | wezterm-term equivalent | Notes |
|---|---|---|
| `Parser::new(rows, cols, 0)` | `WezParser::new(rows, cols)` | Requires `Arc<dyn TerminalConfiguration>` + `Box<dyn Write + Send>` (pass `sink()`) |
| `parser.process(&bytes)` | `terminal.advance_bytes(bytes)` | Direct equivalent. No scrollback limit needed. |
| `parser.set_size(rows, cols)` | `terminal.resize(TerminalSize { rows, cols, .. })` | `TerminalSize` has 5 fields; `pixel_width/height/dpi` can be 0/96 |
| `parser.screen().size()` | `(screen.physical_rows as u16, screen.physical_cols as u16)` | Fields are `usize`, not `u16`; cast required |
| `parser.screen().contents()` | `lines_in_phys_range` + `l.as_str()` join | `visible_lines()` is `#[cfg(test)]`-only in both upstream and tattoy fork. Must reconstruct manually. |
| `parser.screen().cursor_position()` | `terminal.cursor_pos()` → `CursorPosition { x, y, .. }` | Returns `CursorPosition` struct; `(y as u16, x as u16)`. **Deferred-wrap semantic difference** (see below). |

### Impedance mismatches

**`TerminalConfiguration` trait** must be implemented — there's no zero-config
constructor. The minimum viable implementation requires `scrollback_size()` and
`color_palette()`. Phoenix's use case needs only these two.

**`visible_lines()` is `#[cfg(test)]`-only** — documented in both codebase versions.
This is not a gotcha: the API is designed around `phys_range + lines_in_phys_range`
or `for_each_phys_line`, which work fine for our use case. Three lines of extra code.

**Cursor deferred-wrap semantic**: vt100 clamps the cursor strictly to `< cols`
after a save/resize/restore sequence. wezterm-term allows `cursor.x == physical_cols`
(the "pending-wrap" or "right-margin cursor" position). This is standard terminal
behaviour (also correct in xterm). The difference only matters for the
`cursor_position()` return value; screen contents are unaffected. The
`read_terminal` tool would need a note about this. See the test
`wez_resize_cursor_deferred_wrap_semantic` for a documented example.

**Proof of concept:** See `src/terminal/wezterm_parser.rs` for the complete adapter.
All 6 unit tests pass.

---

## 3. Proptest Results

### Test setup

Two stress tests ported from `src/terminal/proptests.rs`, running against the
`WezParser` adapter. Both use proptest with `cases: 512` (default), and can be
run with `PROPTEST_CASES=10000` for higher confidence.

Run with: `cargo test wez_prop -- --ignored`

### Results

| Test | Cases (512 default) | Result | Cases (10000) | Result |
|------|---------------------|--------|---------------|--------|
| `wez_prop_parser_stress_tiny_terminals` | 512 | **FAIL** (0 passed) | 10000 | **FAIL** (0 passed) |
| `wez_prop_parser_stress_resize_then_draw` | 512 | **FAIL** (~128 passed) | 10000 | **FAIL** |

### Failure root cause

**Both tests fail on the same input:** `cols=1, rows=1, bytes=[0x90, 0x71, 0x3F, 0x80]`

Decoded:
- `0x90` = DCS (Device Control String begin)
- `0x71` = `q` → sixel graphics mode  
- `0x3F 0x80` = sixel data payload

This is a **syntactically valid Sixel/DCS sequence** that any terminal might
receive (e.g. from a program that renders images, or from adversarial PTY input).

**Panic location:** `tattoy-wezterm-term 0.1.0-fork.5/src/terminalstate/image.rs:103`

```rust
// image.rs:101-103 (simplified)
let x_delta_divisor = (cols * cell_pixel_width) as u32
    * params.image_width
    / draw_width;  // ← divide by zero when draw_width = 0
```

On a 1×1 terminal, `cell_pixel_width` defaults to 1 and `draw_width` can be 0
for a zero-width Sixel image, triggering the divide.

**Upstream status:** The same code exists verbatim in
`wezterm/wezterm main/term/src/terminalstate/image.rs:103`. The bug is present in
both upstream `wezterm/wezterm` and the `tattoy-wezterm-term` fork. No upstream
fix found at time of evaluation (2026-04-13).

**Equivalence class to vt100 bugs:** This is the same category as our six
`vendor/vt100` patches — image/wide-char arithmetic that doesn't guard against
tiny-terminal edge cases. We'd need to either:
1. Vendor `tattoy-wezterm-term` and apply a `saturating_div` patch, or  
2. Wait for an upstream fix and pin to the patched release.

### Proptest regression seed

Capture saved in `proptest-regressions/terminal/wez_proptests.txt`.

---

## 4. OSC 133 Access Path

### Question: can the backend access structured OSC 133 events?

#### What wezterm-term provides

wezterm-term parses OSC 133 (FinalTerm semantic prompts) in
`terminalstate/performer.rs`. The parsed variants are:

| OSC 133 marker | Handler action | Public access |
|---|---|---|
| `A` / `P` — Prompt start | `pen.set_semantic_type(Prompt)` | Via `screen.lines → line.semantic_zone_ranges()` |
| `B` / `I` — Input start | `pen.set_semantic_type(Input)` | Via `screen.lines → line.semantic_zone_ranges()` |
| `C` — Command start (output begins) | `pen.set_semantic_type(Output)` | Via `screen.lines → line.semantic_zone_ranges()` |
| `D status` — **Command exit code** | `{}` (silently dropped) | **NOT ACCESSIBLE** |

#### Detailed analysis

**Zone annotations (A/B/C)**: OSC 133 A/B/C markers tag each cell written
after them with a `SemanticType` (`Prompt`, `Input`, `Output`). These tags
survive in the screen model and can be read as `ZoneRange` slices via
`line.semantic_zone_ranges()` on each `Line`. This is enough to identify
which parts of the screen are prompt vs command output — useful for the
"extract last command output" pattern.

**Exit codes (D)**: `FinalTermSemanticPrompt::CommandStatus { status, aid }` is
handled with a bare `{}` in `performer.rs:901`. The status code is never stored
or forwarded. There is no `Alert` variant for it, no event callback, and no
public method on `Terminal` or `TerminalState` that exposes the last exit code.

**Assessment**: OSC 133 in wezterm-term is **partial**.
- Prompt/input/output zone demarcation: ✅ accessible
- Exit code on command completion: ❌ silently dropped

**For the "run and wait" agent pattern** (the primary feature motivation from
the task), exit codes are required. This pattern — spawn command, wait for OSC
133;D, return exit code — cannot be implemented using wezterm-term's public API
without either:
1. A code contribution upstream to expose `CommandStatus` via `AlertHandler`, or
2. Implementing our own OSC 133;D parser at the byte stream level (same as current approach).

---

## 5. Migration Effort Estimate

### Assuming upstream fix to the Sixel panic

| Category | Files | Estimated LOC |
|----------|-------|---------------|
| Remove vendor patch and `[patch.crates-io]` block | `Cargo.toml`, `vendor/vt100/` (delete) | −600 LOC |
| Promote `wezterm_parser.rs` adapter to production | `src/terminal/wezterm_parser.rs` | ~130 LOC (already written) |
| Wire adapter into `session.rs` | `src/terminal/session.rs` | ~10 LOC |
| Update relay.rs to use `WezParser` instead of `vt100::Parser` | `src/terminal/relay.rs` | ~15 LOC |
| Update `read_terminal` tool | `src/tools/read_terminal.rs` | ~5 LOC |
| Update `spawn.rs` | `src/terminal/spawn.rs` | ~5 LOC |
| Port `proptests.rs` (remove `vt100::` references) | `src/terminal/proptests.rs` | ~30 LOC change |
| Remove vendor/ directory | `vendor/vt100/` | Delete |
| **Total** | **7 files** | **~200 LOC change, −600 LOC net** |

### Risks

1. **The Sixel panic must be fixed first** — either upstream or via our own patch.
   Without this, we've simply traded one set of vendor patches for another.

2. **`image` is a mandatory dep** — no feature flag to exclude it. Until wezterm-term
   offers a `no-image` feature, the 5 MB binary size increase and 116 new packages
   are non-negotiable.

3. **Exit codes not accessible** — the "run and wait" agent pattern requires
   either upstream changes to expose `CommandStatus` via `AlertHandler`, or a
   separate OSC 133;D byte-stream parser. This was the feature justification for
   migration; if we still need our own parser anyway, the advantage narrows.

4. **Deferred-wrap cursor semantics** — `cursor_position()` may return
   `col == physical_cols`. Any downstream code that assumes `col < cols` will
   need review.

5. **`terminal.allium` spec update** — the `Parser` entity definition in
   `terminal.allium` currently describes the vt100 API surface. A migration
   would require updating the entity definition, constructor signature, and
   the `ParserFedEveryByte` rule (which references `process()`).

### Rollback plan

Since this branch only adds a dev-dep and a `#[cfg(test)]` adapter, rolling back
is trivial: remove `tattoy-wezterm-term` from `[dev-dependencies]`, delete
`src/terminal/wezterm_parser.rs`, and remove the `wez_proptest` module from
`src/terminal/proptests.rs`.

---

## 6. Recommendation

### Don't migrate now — revisit after upstream Sixel fix

**Rationale:**

The Sixel divide-by-zero panic is a hard blocker. It fires on bytes `[0x90, 0x71,
0x3F, 0x80]` — a realistic terminal byte sequence from any graphics-capable
program. Deploying wezterm-term without this fix would expose production to the
same class of panic we've been patching out of vt100.

If we vendor wezterm-term to apply the patch ourselves, we've reproduced the
current vt100 situation at higher cost: 116 new packages, ~5 MB bigger binary,
and still no upstream fix path. The original motivation for the evaluation was
to *escape* the vendoring cycle — this doesn't accomplish that.

The OSC 133 exit code omission further weakens the case. The "run and wait"
feature (the main upside of migration) still requires either an upstream change
or our own parser — same as today.

### Conditions to revisit

1. **The Sixel panic is fixed upstream** in `wezterm/wezterm` (or the tattoy
   fork) and released as a pinnable version.

2. **`CommandStatus` is exposed** via `AlertHandler` or a new public API,
   enabling real exit-code access.

3. **Or**: `image` becomes an optional feature, removing the 5 MB binary cost.

If conditions 1 + 2 are met, migration becomes attractive: the API parity is
good (adapter is already written), the OSC 133 semantic zone model is a genuine
upgrade over vt100 for output parsing, and we'd shed 600 LOC of vendor patches
at the cost of 200 LOC of adapter code.

### What to do instead

- Keep `vendor/vt100` as-is. It's stable for our usage pattern.
- For OSC 133 exit codes and "run and wait": implement a byte-stream parser at
  the relay level (independent of vt100). This doesn't require a parser migration.
- File an upstream issue with the wezterm/wezterm repo linking the failing byte
  sequence: `[0x90, 0x71, 0x3F, 0x80]` on a 1×1 terminal → `image.rs:103`
  divide-by-zero.

---

## 7. terminal.allium Note

If migration lands, the following `terminal.allium` items need updating:

- **`Parser` entity** (currently describes `vt100::Parser`): constructor
  signature changes from `new(rows, cols, scrollback)` to
  `new(size: TerminalSize, config, term_program, term_version, writer)`. The
  `scrollback` parameter disappears (configurable via `TerminalConfiguration`).

- **`ParserFedEveryByte` rule**: `parser.process(&bytes)` becomes
  `terminal.advance_bytes(bytes)`. Semantics identical; name only.

- **`ParserDimensionSync` invariant**: `parser.screen().size()` becomes
  `(screen.physical_rows as u16, screen.physical_cols as u16)`. Same contract,
  different access path.

- **New invariant candidate**: `CursorWithinGridOrDeferredWrap` — cursor
  position satisfies `row < rows && col <= cols` (not `< cols` as in vt100).
  Current spec assumes `< cols`.

None of these are blocking — they're spec hygiene tasks for the migration sprint.

---

## Related Evaluation

See `specs/terminal/alacritty-evaluation.md` (task 24676) for the alacritty_terminal
evaluation conducted after this one.  Summary: alacritty_terminal has a similar
tiny-terminal panic (wide-char OOB, not sixel) but is the **preferred long-term
candidate** due to 10 new packages (+200 KB) vs wezterm's 116 new packages (+5 MB).

---

## Appendix: Reproduction

```bash
# Run the evaluation proptests explicitly (they are #[ignore]'d in CI)
PROPTEST_CASES=10000 cargo test wez_prop -- --ignored

# Expected output: both tests FAIL with
# "attempt to divide by zero" in tattoy-wezterm-term/src/terminalstate/image.rs:103

# Run unit tests (all pass)
cargo test terminal::wezterm_parser
```

**Regression seeds**: `proptest-regressions/terminal/wez_proptests.txt`

**Adapter source**: `src/terminal/wezterm_parser.rs`
