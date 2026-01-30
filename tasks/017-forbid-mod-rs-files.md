---
created: 2026-01-30
priority: p3
status: ready
---

# Add linter rule to forbid mod.rs files

## Summary

Add a lint or CI check to enforce using named module files (e.g., `foo.rs`) instead of `mod.rs` files (e.g., `foo/mod.rs`).

## Context

Rust supports two styles for module organization:
1. `foo.rs` + `foo/` subdirectory (modern style)
2. `foo/mod.rs` (legacy style)

The modern style is preferred because:
- File names are more descriptive in editor tabs
- Easier to navigate in file trees
- Rust 2018 edition recommendation

## Acceptance Criteria

- [ ] Add clippy lint or custom check to forbid `mod.rs` files
- [ ] Migrate any existing `mod.rs` files to named files
- [ ] Document the convention in AGENT.md or similar

## Implementation Options

1. **Clippy**: No built-in lint for this, but could use `#![forbid(clippy::mod_module_files)]` if it exists
2. **Custom script**: `find src -name 'mod.rs' | grep -q . && exit 1`
3. **CI check**: Add to GitHub Actions or pre-commit hook

## Notes

Current `mod.rs` files in the codebase:
- `src/tools/mod.rs`
- `src/tools/patch/mod.rs`
- `src/state_machine/mod.rs`
- `src/runtime/mod.rs`
- `src/api/mod.rs`
- `src/llm/mod.rs`
- `src/db/mod.rs`
