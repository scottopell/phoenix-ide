---
created: 2026-01-29
priority: p3
status: done
---

# Fix rename API field name inconsistency

## Summary

The `/api/conversation/:id/rename` endpoint expects a `slug` field in the JSON body, but documentation and intuition suggest it should be `name`.

## Context

Discovered during QA validation (REQ-API-006). The rename endpoint rejects `{"name": "new-name"}` with:
```
Failed to deserialize the JSON body into the target type: missing field `slug`
```

## Resolution

Changed field name from `slug` to `name` for better user experience.

## Changes

- `src/api/types.rs`: `RenameRequest.slug` → `RenameRequest.name`
- `src/api/handlers.rs`: Updated to use `req.name`
