---
created: 2026-04-12
priority: p2
status: ready
artifact: pending
---

# terminal-parser-proptest-resize-then-draw

## Problem

`terminal::proptests::prop_parser_stress_resize_then_draw` found a new failing
case during a `./dev.py check` run. The shrunk minimal counterexample is:

```
initial = Dims { cols: 1, rows: 1 }
ops = [
  Resize(Dims { cols: 230, rows: 9 }),
  Draw(...),
  Draw(...),
  Draw(...),
  Draw(...),
  Resize(Dims { cols: 210, rows: 8 }),
  Draw(...),
]
```

The new regression seed was added to
`proptest-regressions/terminal/proptests.txt`:

```
cc a7331e24b59e23f71ac9086005dd0a4139a870dbe84fc851f9f0db7a552c8d9e
```

## Reproduction

```
cargo test -p phoenix_ide --bin phoenix_ide \
    terminal::proptests::prop_parser_stress_resize_then_draw
```

Discovered while implementing the resizable-pane-divider UI refactor. The
failure is in pure Rust terminal parser code, unrelated to the UI work in
that commit.

## Investigation

Likely a parser/grid invariant violation when the grid is shrunk after a
draw with multi-byte UTF-8 sequences. Look for off-by-one or wrap handling
near the resize code path.
