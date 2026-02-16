---
created: 2026-01-30
priority: p3
status: done
---

# API route naming consistency

## Summary

The API uses inconsistent pluralization for conversation routes.

## Resolution

Standardized all routes to use plural `/api/conversations/...` following REST conventions:

- `GET /api/conversations` - list conversations
- `GET /api/conversations/archived` - list archived
- `POST /api/conversations/new` - create new
- `GET /api/conversations/:id` - get one
- `POST /api/conversations/:id/chat` - send message
- `POST /api/conversations/:id/cancel` - cancel
- `POST /api/conversations/:id/archive` - archive
- `POST /api/conversations/:id/unarchive` - unarchive
- `POST /api/conversations/:id/delete` - delete
- `POST /api/conversations/:id/rename` - rename
- `GET /api/conversations/:id/stream` - SSE stream
- `GET /api/conversations/by-slug/:slug` - get by slug

## Changes

- `src/api/handlers.rs` - updated route definitions
- `phoenix-client.py` - updated client URLs
