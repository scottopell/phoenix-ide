---
created: 2026-04-10
priority: p2
status: done
artifact: vendor/vt100/src/grid.rs
---

# vt100 0.15.2 panics on certain byte sequences — fixed via vendor patch

Found by proptest (`prop_parser_accepts_arbitrary_bytes`) during allium:propagate
for terminal.allium (`ParserFedEveryByte` invariant).

## Root cause

Three related u16 underflows in `vt100 0.15.2` (also present in 0.16.2):

1. `grid.rs col_wrap()`: `prev_pos.row -= scrolled` overflows when `scrolled > prev_pos.row`
2. `grid.rs col_wrap()`: `self.size.cols - width` overflows when `width > size.cols`
3. `screen.rs`: `pos.col > size.cols - width` overflows when `width > size.cols`

All three are triggered by wide characters (unicode width=2) on narrow terminals (≤1 col).

## Minimal reproducer

```rust
let mut parser = vt100::Parser::new(1, 1, 0); // 1×1 terminal
parser.process(&[0xe3, 0x80, 0x80]);          // U+3000 IDEOGRAPHIC SPACE (width 2)
```

## Fixes applied in vendor/vt100

- `src/grid.rs` line 668: `self.size.cols - width` → guarded with `width <= self.size.cols &&`
- `src/grid.rs` line 672: `prev_pos.row -= scrolled` → `prev_pos.row.saturating_sub(scrolled)`
- `src/screen.rs` line 822: width clamped to `size.cols.max(1)` at entry point,
  preventing overflow in all downstream arithmetic
- `src/screen.rs` line 837: `pos.col > size.cols - width` guarded with `size.cols > 0 && width <= size.cols &&`

The same bugs exist in vt100 0.16.2 (verified). Remove the vendor patch and the
`[patch.crates-io]` entry in Cargo.toml when an upstream release contains the fix.
