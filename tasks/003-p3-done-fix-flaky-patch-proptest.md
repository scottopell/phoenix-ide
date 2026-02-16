---
created: 2026-01-29
priority: p3
status: done
---

# Fix Flaky prop_multiple_patches_independent Test

## Summary

The `prop_multiple_patches_independent` proptest can fail when generated strings overlap, causing `OldTextNotUnique` errors.

## Context

During testing, this failure was observed:

```
minimal failing input: path = "a.txt", part1 = "__A", part2 = "__9A", part3 = "__9", repl1 = "A00", repl3 = "0__"
```

The issue is that `part3 = "__9"` is a substring of `part2 = "__9A"`, so when trying to replace `part3` in the combined string `"__A|__9A|__9"`, there are two matches.

The test passes most of the time because the `prop_assume!` filters out cases where parts are equal, but it doesn't check for substring relationships.

## Acceptance Criteria

- [ ] Add `prop_assume!` to filter out substring relationships between parts
- [ ] Or use a different generation strategy that guarantees unique, non-overlapping strings
- [ ] Test passes consistently across 1000+ runs

## Notes

Location: `src/tools/patch/proptests.rs`, around line 219

Possible fix:
```rust
prop_assume!(!part1.contains(&part2) && !part2.contains(&part1));
prop_assume!(!part2.contains(&part3) && !part3.contains(&part2));
prop_assume!(!part1.contains(&part3) && !part3.contains(&part1));
```

Or use a strategy that generates strings with guaranteed unique prefixes.
