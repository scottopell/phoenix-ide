---
created: 2026-01-29
priority: p3
status: ready
---

# Fix rename API field name inconsistency

## Summary

The `/api/conversation/:id/rename` endpoint expects a `slug` field in the JSON body, but documentation and intuition suggest it should be `name`.

## Context

Discovered during QA validation (REQ-API-006). The rename endpoint rejects `{"name": "new-name"}` with:
```
Failed to deserialize the JSON body into the target type: missing field `slug`
```

Either the API should accept `name` (more intuitive) or documentation should clarify the expected field.

## Acceptance Criteria

- [ ] Decide on field name: `name` (user-friendly) or `slug` (current)
- [ ] Update API or documentation accordingly
- [ ] Add test coverage for rename endpoint

## Notes

Current implementation in `src/api/handlers.rs` uses `RenameRequest` struct expecting `slug` field.
