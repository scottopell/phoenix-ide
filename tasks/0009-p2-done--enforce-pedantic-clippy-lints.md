---
created: 2026-01-29
priority: p2
status: done
---

# Enforce Pedantic Clippy Lints

## Summary

Add strict clippy pedantic lints to Cargo.toml and fix all violations. No allows - full deny compliance.

## Context

Following lading project's approach to hardcore linting. Reference: `$HOME/RUST_BEST_PRACTICES.md`

## Lints to Add

```toml
[lints.clippy]
pedantic = { level = "deny", priority = -1 }
float_cmp = "deny"
manual_memcpy = "deny"
redundant_allocation = "deny"
rc_buffer = "deny"
unnecessary_to_owned = "deny"
dbg_macro = "deny"

[lints.rust]
unused_extern_crates = "deny"
unused_allocation = "deny"
unused_assignments = "deny"
unused_comparisons = "deny"
```

## Work Done (Stashed)

Partial progress made - format string fixes, wildcard import fixes, raw string hash fixes.
Run `git stash pop` to restore.

## Remaining Errors (57)

By category:
- 8 unused self arguments → refactor to associated functions
- 6 map_unwrap_or → use map_or_else
- 5 match_same_arms → merge identical arms
- 5 doc_markdown → add backticks to identifiers
- 4 trivially_copy_pass_by_ref → pass small enums by value
- 4 returning str tied to lifetime → use `&'static str`
- 3 struct_field_names (all fields same postfix) → rename fields
- 3 format_push_string → use write!
- 2 too_many_lines → split functions or allow locally
- 2 similar_names → rename variables
- 2 needless_pass_by_value → take reference
- 2 unused_async → remove async
- 1 unnecessary_wraps → simplify return type
- 1 implicit_clone → use clone()
- 1 manual_let_else → use let...else
- 1 single_match_else → use if let
- 1 cast_possible_wrap → use cast_signed()
- 1 if_not_else → flip condition
- 1 needless_raw_string_hashes → remove hashes
- 1 redundant_closure → use method reference

## Notes

Some fixes require careful thought:
- `struct_field_names` for UsageData/Usage (tokens suffix) - API change
- `too_many_lines` for transition() and execute_effect() - may need refactoring
- `unused_self` methods - check if self is needed for trait impl consistency
