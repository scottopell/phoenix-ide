---
created: 2026-01-30
priority: p3
status: ready
---

# API route naming consistency

## Summary

The API uses inconsistent pluralization for conversation routes.

## Details

Listing endpoints use plural:
- `GET /api/conversations` - list conversations
- `GET /api/conversations/archived` - list archived
- `POST /api/conversations/new` - create new

Single-conversation endpoints use singular:
- `GET /api/conversation/:id` - get one
- `POST /api/conversation/:id/chat` - send message
- `POST /api/conversation/:id/archive` - archive
- `GET /api/conversation/:id/stream` - SSE stream

Also mixed:
- `GET /api/conversation-by-slug/:slug` - hyphenated singular

## Options

1. **Do nothing** - it works, clients adapt
2. **Standardize on plural** - `/api/conversations/:id/chat` (REST convention)
3. **Standardize on singular** - `/api/conversation/:id` and `/api/conversation` for list

## Notes

Discovered during QA testing. Low priority since functionality is correct.
If fixing, would need to update simple client and any other consumers.
