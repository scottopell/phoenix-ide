---
created: 2026-04-20
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# Split handlers.rs into domain-specific handler modules

## Summary

handlers.rs is ~3500 lines covering HTTP routing, git operations, branch
management, conversation lifecycle, and model/credential status. The git_ops
extraction helped but more splitting is warranted.

## Done When

Separate files for git-related handlers, lifecycle handlers (abandon, merge),
and conversation CRUD. Main handlers.rs is routing + delegation. All tests pass.
