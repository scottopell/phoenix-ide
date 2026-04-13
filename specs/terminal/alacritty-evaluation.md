# alacritty_terminal Migration Evaluation

**Status:** Complete — go/no-go recommendation issued  
**Task:** 24676  
**Date:** 2026-04-13  
**Related:** `specs/terminal/wezterm-evaluation.md` (task 24673)

---

## Summary

**Recommendation: Don't migrate — yet. Same reason as wezterm-term, but a more fixable bug.**

`alacritty_terminal v0.26.0` (tag v0.17.0, and `master` v0.26.1-dev) has an
index-out-of-bounds panic in `grid.rs:439` when a width-2 Unicode character
(e.g. U+3000 IDEOGRAPHIC SPACE) is fed to a terminal with exactly 1 column.
The same proptest cases catch this immediately. Root cause: `write_at_cursor`
for the `WIDE_CHAR_SPACER` advances the cursor to `col + 1` unconditionally,
which is OOB when `columns = 1`. Same class as vt100 patches 2–4.

However, the story here is meaningfully better than wezterm-term:

- **10 new packages** (vs wezterm's 116). Zero image codecs. Zero rav1e.
- **+200 KB binary growth** (vs wezterm's +5 MB). Essentially free.
- **The bug is simpler** — a one-line guard at the spacer write. Not a
  deep arithmetic overflow in image decoding.
- **Active upstream** — alacritty is heavily maintained; the fix could land quickly.
- **OSC 133** is absent in both, so neither wins on that axis.

alacritty_terminal is the stronger long-term candidate. When the upstream fix
lands, the migration path is straightforward (adapter already written).

---

## 1. Dependency Report

### Crate evaluated

| Property | Value |
|----------|-------|
| Crate | `alacritty_terminal` 0.26.0 |
| Tag | `v0.17.0` (alacritty/alacritty monorepo) |
| License | **Apache-2.0** |
| Source | git dep from `github.com/alacritty/alacritty` |
| Pinnable? | Yes, via git tag |
| Maintenance | Actively developed; v0.17.0 released 2026-03-05, commits days ago |

**Note on Apache-2.0**: All existing phoenix deps are MIT or Apache-2.0. Apache-2.0
is compatible. No license conflict.

**Note on crates.io**: `alacritty_terminal` is not independently published to
crates.io. The git dep pinned to a tag is the recommended approach.

### Transitive dependency count

| Metric | Baseline | +alacritty_terminal |
|--------|----------|--------------------|
| Total packages in `Cargo.lock` | 400 | **410** |
| New packages added | — | **10** |

Compare: wezterm-term added **116** packages.

### New packages added

| Package | Purpose | Size impact |
|---------|---------|-------------|
| `alacritty_terminal` | Terminal emulator core | Moderate |
| `vte` 0.15.0 | VT sequence parser (new version, coexists with 0.11 used by vt100) | Moderate |
| `cursor-icon` | Mouse cursor types | Tiny |
| `hermit-abi` | Hermit OS ABI stubs | Tiny |
| `home` | Home directory lookup | Tiny |
| `polling` | Cross-platform epoll/kqueue | Small |
| `rustix-openpty` | `openpty(2)` binding | Tiny |
| `signal-hook` | Unix signal handling | Small |
| `miow` | Windows I/O completion (dead code on Linux) | Tiny |
| `piper` | Async byte pipe (Windows only) | Tiny |

**No `image`, no `rav1e`, no `rayon`, no image codecs.**

### License compatibility

All 10 new packages are Apache-2.0 or MIT licensed. No GPL or proprietary licenses.

### Binary size impact

| Binary | Size (stripped) | Notes |
|--------|-----------------|-------|
| Baseline hello-world | 426 KB | Isolated baseline |
| `alac-size-test` (imports + uses `Term`, stripped) | **616 KB** | +190 KB from alacritty deps |
| `wezterm-test` (same test, stripped) | 5.4 MB | For comparison |
| `phoenix_ide` baseline | 24 MB | Current production binary |
| `phoenix_ide` + alacritty (estimated) | **~24.2 MB** | +~200 KB, <1% growth |

The binary size delta is **negligible** compared to wezterm-term's +5 MB.

---

## 2. API Comparison

| vt100 method | alacritty_terminal equivalent | Notes |
|---|---|---|
| `Parser::new(rows, cols, 0)` | `AlacrittyParser::new(rows, cols)` | Requires impl `Dimensions` (trivial struct); `Term::new(Config::default(), &size, VoidListener)` + `ansi::Processor::new()` |
| `parser.process(&bytes)` | `ansi_processor.advance(&mut term, bytes)` | **Split pattern**: parser and state are separate structs |
| `parser.set_size(rows, cols)` | `term.resize(TermSize { cols, rows })` | `TermSize` is `#[cfg(test)]`-only; must define own `Dimensions` impl |
| `parser.screen().size()` | `(term.screen_lines() as u16, term.columns() as u16)` | `Dimensions` trait; clean |
| `parser.screen().contents()` | `term.bounds_to_string(Point::new(Line(0), Column(0)), Point::new(Line(rows-1), Column(cols-1)))` | `bounds_to_string` is a public method |
| `parser.screen().cursor_position()` | `(term.grid().cursor.point.line.0 as u16, term.grid().cursor.point.column.0 as u16)` | Public fields on `Cursor` |

### Structural difference: split parser/state

vt100 and wezterm-term bundle the parser and terminal state into one struct.
alacritty_terminal separates them:

```rust
// vt100 (current)
let mut parser = vt100::Parser::new(rows, cols, 0);
parser.process(bytes);

// alacritty_terminal
let mut term  = Term::new(Config::default(), &size, VoidListener);
let mut ansi  = ansi::Processor::new();
ansi.advance(&mut term, bytes);
```

The `AlacrittyParser` adapter (see `src/terminal/alacritty_parser.rs`) hides
this split behind the same one-struct API. Migration would touch ~10 lines in
`session.rs` (store `AlacrittyParser` instead of `Arc<Mutex<vt100::Parser>>`).

### Other impedance mismatches

**`TermSize` is `#[cfg(test)]`-only** in both the tagged release and master.
Must define a local `Dimensions` implementation (three trait methods, ~8 lines).
This is minor.

**`ansi::Processor` type inference**: `Processor` is generic over a `Timeout`
impl; calling `Processor::new()` requires a type annotation (`Processor::
<ansi::StdSyncHandler>::new()`) or explicit type ascription in some contexts.
The adapter handles this transparently.

**`bounds_to_string` coordinate system**: `Line(0)` is the top of the viewport;
negative lines are scrollback. With `scrolling_history: 0` in `Config`, there
is no scrollback — same as vt100's `scrollback = 0`.

**Cursor position**: `term.grid().cursor.point` is a public field. `line.0` is
`i32`, `column.0` is `usize`. Values are always in bounds for the visible
screen. **No deferred-wrap position at `col == cols`** (unlike wezterm-term).
alacritty uses `input_needs_wrap: bool` flag instead. Cursor row/col always
satisfy `row < rows` and `col < cols`.

**Proof of concept:** See `src/terminal/alacritty_parser.rs`. All 8 unit tests
pass, including the exact wezterm-term blocker bytes.

---

## 3. Proptest Results

### Test setup

Same two stress tests as the wezterm evaluation, ported to `AlacrittyParser`.
Default 512 cases; run with `PROPTEST_CASES=10000` for evaluation.

Run with: `cargo test alac_prop -- --ignored`

### Results

| Test | Cases (512) | Result | Cases (10000) | Result |
|------|-------------|--------|---------------|--------|
| `alac_prop_parser_stress_tiny_terminals` | 512 | **FAIL** (0 passed) | 10000 | **FAIL** |
| `alac_prop_parser_stress_resize_then_draw` | 512 | **FAIL** (0 passed) | 10000 | **FAIL** |

### Failure root cause

**Both tests fail on the same input:** `cols=1, rows=1, bytes=[0xE3, 0x80, 0x80]`

Decoded: U+3000 IDEOGRAPHIC SPACE — a valid, printable, width-2 Unicode character
commonly used in CJK text. Any program that emits Japanese/Chinese text could
trigger this.

**Panic location:** `alacritty_terminal v0.26.0/src/grid/mod.rs:439`

```rust
// grid.rs:437-441
pub fn cursor_cell(&mut self) -> &mut T {
    let point = self.cursor.point;
    &mut self[point.line][point.column]  // ← OOB when column == cols
}
```

**Stack:**
```
Handler::input (width-2 char)
  → write_at_cursor (WIDE_CHAR at col=0, sets cursor.col = 0+1 = 1)
  → write_at_cursor (WIDE_CHAR_SPACER at col=1)   ← cursor_cell panics
  → grid[Line(0)][Column(1)]  ← row has 1 element, index 1 is OOB
```

**Root cause:** In `term/mod.rs`, the `input()` handler for width-2 chars checks
`col + 1 >= columns` to decide whether to wrap the wide char to the next line.
When `columns = 1`: the check fires and the char wraps. After wrapping, `col = 0`
again, and the wide char is written at `col = 0`. The code then **unconditionally**
advances `cursor.col` to `col + 1 = 1` and writes the `WIDE_CHAR_SPACER` at
`col = 1` — which is out-of-bounds for a 1-column row.

**Upstream status:** Same code is present in `master` (`v0.26.1-dev`) as of
2026-04-13. Not yet fixed.

**Compared to wezterm-term bug:** Simpler. One guard is missing, in a well-isolated
functionm not in an image decoding pipeline. A one-line fix:
```rust
// Proposed fix in term/mod.rs, after write_at_cursor for WIDE_CHAR:
if self.grid.cursor.point.column + 1 < columns {
    self.grid.cursor.point.column += 1;
    self.grid.cursor.template.flags.insert(Flags::WIDE_CHAR_SPACER);
    self.write_at_cursor(' ');
    self.grid.cursor.template.flags.remove(Flags::WIDE_CHAR_SPACER);
}
```
This is an upstreamable PR-sized fix, unlike the wezterm sixel arithmetic.

### Proptest regression seed

Captured in `proptest-regressions/terminal/alac_proptests.txt`.

---

## 4. OSC 133 Access Path

**Status: Not implemented. `❌` for both A/B/C zone annotations and D exit codes.**

Alacritty_terminal (via its `vte` crate) dispatches unrecognized OSC sequences
to a `debug!` log line and discards them:

```rust
// vte-0.15.0/src/ansi.rs, osc_dispatch:
match params[0] {
    b"52" => { /* clipboard */ }
    b"10" | b"11" | b"12" => { /* colors */ }
    // ... other known OSCs ...
    _ => unhandled(params),  // OSC 133 lands here
}
```

There are no `SemanticType` cell annotations, no `AlertHandler`-equivalent
callbacks, and no stored exit code. OSC 133 is completely invisible to the API.

**Compared to wezterm-term:** wezterm-term applies A/B/C as semantic zone
annotations on cells (accessible via `line.semantic_zone_ranges()`), and drops
only D (exit codes). alacritty_terminal drops all four markers.

**Future feasibility:** The `EventListener::send_event` callback could be used.
A custom `EventListener` impl receiving OSC 133 events would require either:
1. A PR to `vte` to parse OSC 133 and call a `Handler::prompt_mark()` method, or
2. A custom `vte::Perform` shim that intercepts `osc_dispatch`, checks `params[0] == b"133"`,
   and emits an event before forwarding.

Path 2 is ~30 lines of code, implementable without upstream changes.
Path 1 is a well-scoped contribution to `vte` / alacritty.

Either way, this is future work — it does not block the stability migration goal.

---

## 5. Migration Effort Estimate

### Assuming upstream fix to the wide-char OOB panic

| Category | Files | Estimated LOC |
|----------|-------|---------------|
| Remove vendor patch and `[patch.crates-io]` block | `Cargo.toml`, `vendor/vt100/` (delete) | −600 LOC |
| Promote `alacritty_parser.rs` adapter to production | `src/terminal/alacritty_parser.rs` | ~120 LOC (already written) |
| Wire into `session.rs` (swap parser type) | `src/terminal/session.rs` | ~10 LOC |
| Update relay loop | `src/terminal/relay.rs` | ~15 LOC |
| Update `read_terminal` tool | `src/tools/read_terminal.rs` | ~5 LOC |
| Update `spawn.rs` | `src/terminal/spawn.rs` | ~5 LOC |
| Remove `vt100` dep, add `alacritty_terminal` git dep | `Cargo.toml` | ~3 LOC |
| Port proptests | `src/terminal/proptests.rs` | ~30 LOC change |
| **Total** | **7 files** | **~195 LOC change, −600 LOC net** |

Nearly identical effort to the wezterm migration estimate, because the API
surface is the same.

### Risks

1. **The wide-char panic must be fixed first.** One-line fix, clearly upstreamable.
   ETA unclear without filing an issue.

2. **Split parser/state** adds a second field to `TerminalHandle`. Low complexity;
   `AlacrittyParser` wraps both already.

3. **`TermSize` is `#[cfg(test)]`-only** upstream. Our `Dimensions` shim is 8
   lines and stable. If upstream later promotes `TermSize` to public, the shim
   can be removed.

4. **Git dep, not crates.io.** Must pin by tag. If alacritty stops tagging releases
   (unlikely given the active project), we'd need a commit hash. Risk: low.

5. **`terminal.allium` spec update** — same items as the wezterm evaluation:
   `Parser` entity constructor, `ParserFedEveryByte` rule method name.
   Non-blocking spec hygiene on migration sprint.

### Rollback plan

Add back `vendor/vt100` and revert the session.rs/relay.rs changes. The adapter
file can stay dormant in `#[cfg(test)]`.

---

## 6. Recommendation

### Don't migrate now — file upstream issue, revisit when fixed

**Rationale:**

The wide-char panic is a hard blocker today, but it is a **much simpler fix**
than the wezterm Sixel divide-by-zero. It's a missing bounds guard in a
well-understood code path, not deep image codec arithmetic. Filing an issue
with the minimal reproducer (`[0xE3, 0x80, 0x80]` on a 1×1 terminal) gives
alacritty's active maintainers a clear, small PR target.

alacritty_terminal is **the preferred candidate** for the eventual migration:
- 10 new packages vs 116 for wezterm-term
- +200 KB vs +5 MB binary growth
- Active upstream with rapid response time
- No image codec footprint
- Cursor semantics are cleaner than wezterm-term (no deferred-wrap at `col == cols`)

### Action items

1. **File an upstream issue** at `alacritty/alacritty` with the reproducer:
   `[0xE3, 0x80, 0x80]` on a `columns=1` terminal — `grid.rs:439` OOB in
   `cursor_cell`. Reference the stack trace from this evaluation.

2. **Watch the issue**. When a fix is tagged (expected: one-line guard), migrate
   by pinning to that tag. The adapter is already written.

3. **OSC 133 exit codes**: implement a byte-stream parser at the relay level
   independently of whichever parser we use. This unblocks the "run and wait"
   agent tool without waiting for either migration.

---

## 7. Comparative Summary Across Both Evaluations

| Dimension | vt100 (current) | wezterm-term | alacritty_terminal |
|-----------|-----------------|--------------|--------------------|
| Upstream maintenance | Dead (~2yr) | Active | Very active |
| New packages | 0 | +116 | **+10** |
| Binary size delta | 0 | +5 MB | **+200 KB** |
| Tiny-terminal bug | 6 patches applied | Divide-by-zero in sixel (unfixed) | Wide-char OOB (unfixed, simpler) |
| OSC 133 zone annotations | No | Yes (A/B/C) | No |
| OSC 133 exit codes | No | No (silently dropped) | No |
| Cursor model | Strict `col < cols` | Deferred-wrap (`col <= cols`) | Strict `col < cols` |
| License | MIT | MIT | Apache-2.0 |
| Migration effort | n/a | ~200 LOC | **~195 LOC** |
| **Recommendation** | Keep for now | No-go | **Preferred; wait for fix** |

### Decision tree

```
Does the upstream alacritty wide-char fix land?
  YES → Migrate to alacritty_terminal. Adapter is ready.
  NO (>3mo) → Re-evaluate: apply vendor patch to alacritty_terminal
              (simpler than vt100 patches) or wait further.

Does wezterm-term fix its Sixel panic AND expose OSC 133 exit codes?
  YES to both → Re-evaluate wezterm-term for the richer feature set.
  YES to panic only → Still not preferred; image dep footprint remains.
```

---

## terminal.allium Note

If alacritty migration lands, the same three `terminal.allium` items need
updating as noted in `wezterm-evaluation.md`:
- `Parser` entity constructor signature
- `ParserFedEveryByte` rule method name (`advance` not `process`)
- `ParserDimensionSync` invariant access path

Additionally: the `cursor_position` contract in the spec should note that
alacritty_terminal strictly satisfies `row < rows` and `col < cols` at all
times (no deferred-wrap edge case to document).

---

## Appendix: Reproduction

```bash
# Run the evaluation proptests (they are #[ignore]'d in CI)
PROPTEST_CASES=10000 cargo test alac_prop -- --ignored

# Expected output: both tests FAIL with
# "index out of bounds: the len is 1 but the index is 1"
# in alacritty_terminal/src/grid/mod.rs:439

# Run unit tests (all 8 pass, including the wezterm blocker bytes)
cargo test terminal::alacritty_parser

# Verify the wezterm blocker byte sequence is handled safely:
# bytes [0x90, 0x71, 0x3F, 0x80] on a 1x1 terminal -- alacritty ignores DCS/sixel
cargo test alac_process_does_not_panic_on_wezterm_trigger_bytes
```

**Regression seeds**: `proptest-regressions/terminal/alac_proptests.txt`

**Adapter source**: `src/terminal/alacritty_parser.rs`
