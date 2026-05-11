---
created: 2026-05-10
priority: p3
status: ready
artifact: dev.py
---

`./dev.py check` runs `cargo clippy -- -D warnings` which only checks the default targets (main bin + lib). Test code is not gated. With `cargo clippy --all-targets -- -D warnings`, ~82 lint errors surface in tests: doc_markdown (missing backticks around identifiers), too_many_lines, unnested or-patterns, redundant continue, etc.

Fix in two stages:
1. Clean up the existing test-code lint debt. Mostly mechanical (add backticks, split long test functions, flatten or-patterns).
2. Update dev.py clippy step at dev.py:1557 to include `--all-targets` so future test lint regressions are caught.

Touch: many test modules under crates/phoenix-ide/src/**/tests/, then dev.py.
