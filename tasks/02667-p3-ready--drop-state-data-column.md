---
created: 2026-04-20
priority: p3
status: ready
artifact: src/db/migrations.rs
---

# Drop dead state_data column from conversations table

## Summary

The `state_data` column is never read in db.rs, never populated except by a
migration that NULLs it. Should be dropped via a new migration.

## Done When

Migration 003 drops the column. No code references state_data.
