---
created: 2026-01-29
priority: p3
status: done
---

# Clean Up Compiler Warnings

## Summary

The build produces 42 warnings, mostly unused imports and dead code. Clean these up for a cleaner build.

## Context

Running `cargo build` shows warnings like:
- Unused imports (`types::*`, `LlmErrorKind`, `ToolCall`, `ToolInput`, etc.)
- Unused fields (`model`, `thoughts`, `resulting_content`)
- Unused functions (`open_in_memory`, `with_clipboards`, `clipboards`)

## Acceptance Criteria

- [ ] `cargo build` produces zero warnings
- [ ] `cargo build --release` produces zero warnings
- [ ] No functional changes, only cleanup

## Notes

Can use `cargo fix --bin phoenix_ide` for automatic fixes where possible.

Some "unused" items may be intentionally public APIs - consider adding `#[allow(dead_code)]` with a comment explaining why, rather than removing.
