---
created: 2026-04-13
priority: p2
status: done
artifact: src/terminal/
---

# migrate-to-alacritty-terminal

## Problem

`vendor/vt100` carries 6 hand-applied patches against an unmaintained upstream.
The evaluation (tasks 24673, 24676) found `alacritty_terminal` is the right
replacement: 10 new packages, +200 KB binary, active upstream, strict cursor
semantics.

The only blocking bug (wide-char OOB on 1-column terminals) is fully prevented
by enforcing `cols >= 2` in resize validation — a one-line change that is
independently justified (a 1-column terminal is unusable and no real terminal
emulator produces one).

## What to do

### 1. Enforce `cols >= 2` minimum (ResizeFrameRejected extension)

In `src/terminal/relay.rs` change:
```rust
if cols == 0 || rows == 0 {
```
to:
```rust
if cols < 2 || rows == 0 {
```

Update the `ResizeFrameRejected` rule in `terminal.allium` and the proptests to
reflect `cols >= 2` as the new minimum.

### 2. Promote `alacritty_parser.rs` to production

- Rename / move `src/terminal/alacritty_parser.rs` to be a production module
  (remove `#[cfg(test)]` gate).
- Rename `AlacrittyParser` to `Parser` inside the module (or keep the name —
  adapter wraps both `Term` and `ansi::Processor`).

### 3. Wire into session and relay

- `src/terminal/session.rs`: replace `Arc<Mutex<vt100::Parser>>` with
  `Arc<Mutex<alacritty_parser::AlacrittyParser>>`.
- `src/terminal/relay.rs`: update `set_size`, `process`, and parser lock usage.
- `src/terminal/spawn.rs`: replace `vt100::Parser::new(rows, cols, 0)`.
- `src/tools/read_terminal.rs`: update `screen().contents()` call to
  `parser.contents()`.

### 4. Move dep from dev to production

In `Cargo.toml`:
- Move `alacritty_terminal` from `[dev-dependencies]` to `[dependencies]`.
- Remove `vt100 = "0.15"` from `[dependencies]`.
- Remove the `[patch.crates-io]` block.
- Remove `tattoy-wezterm-term` from `[dev-dependencies]` (wezterm eval is done).
- Delete `vendor/vt100/`.

### 5. Update proptests

- Remove `vt100::Parser` from all proptest helpers; use `AlacrittyParser`.
- Update `arb_valid_dims()` minimum cols from `1` to `2`.
- Remove the `#[ignore]` from `alac_prop_parser_stress_*` tests (they should
  now pass with the cols>=2 constraint).
- Update `zero_cols_resize_frame_is_rejected` to also assert cols=1 is rejected.
- Remove `wezterm_parser.rs` module (or keep as dormant `#[cfg(test)]` file
  for historical reference — either is fine).

### 6. Update `terminal.allium`

- `Parser` entity: new constructor signature.
- `ParserFedEveryByte` rule: `advance()` not `process()`.
- `ParserDimensionSync` invariant: new access path.
- `ResizeFrameRejected` rule: precondition is `cols >= 2` (not `cols > 0`).
- Add note: cursor position always satisfies `col < cols` (no deferred-wrap).

### 7. Update `alacritty-evaluation.md`

Note the migration landed and the min-cols-2 mitigation applied.

## Acceptance criteria

- [ ] `vendor/vt100/` deleted; `[patch.crates-io]` block removed
- [ ] `vt100` dep removed from `Cargo.toml`
- [ ] `alacritty_terminal` in `[dependencies]`
- [ ] `cols < 2` guard in relay.rs resize path
- [ ] `AlacrittyParser` drives the production relay loop and `read_terminal` tool
- [ ] `alac_prop_parser_stress_*` proptests pass (no longer `#[ignore]`)
- [ ] `terminal.allium` updated
- [ ] `./dev.py check` passes
