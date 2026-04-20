---
created: 2026-04-20
priority: p1
status: ready
artifact: src/db/schema.rs
---

# Filter __LEGACY_EMPTY__ sentinel in task_title accessor

## Summary

The `task_title()` accessor returns `Some("__LEGACY_EMPTY__")` if any row
still has the sentinel. Should filter to `None` in the accessor, or better
yet remove the Default impl entirely (see task 02656).

## Done When

No API response can contain `__LEGACY_EMPTY__` for any field.
