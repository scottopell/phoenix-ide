---
created: 2026-04-20
priority: p1
status: ready
artifact: src/db/schema.rs
---

# Remove serde(default) shims from ConvMode fields

## Summary

The migration system (A2) handles data cleanup. The `#[serde(default)]` shims
on ConvMode::Work and ConvMode::Branch fields still produce `__LEGACY_EMPTY__`
sentinels for any corrupt row. Now that migrations guarantee no empty fields,
these defaults should be removed so deserialization failures are hard errors.

## Done When

All `#[serde(default)]` removed from ConvMode Work/Branch fields.
`Default` impl on `NonEmptyString` removed or made private.
Deserialization of empty-field rows produces a logged error, not a sentinel.
