---
created: 2026-04-07
priority: p2
status: done
artifact: src/api/handlers.rs
---

# Read-only share mode (no auth required)

## Problem

Users want to share their Phoenix session with coworkers for pair
programming or demos. Currently it's all-or-nothing: either the port
is open (full access) or behind auth (no access without password).

## What to build

A read-only view accessible without authentication. Coworkers can
watch the conversation stream live but cannot send messages, approve
tasks, or trigger any mutations.

### Backend

- Share URL pattern: `/share/{slug}` or `/s/{slug}` serves a read-only
  view of the conversation
- SSE stream endpoint works without auth (GET, read-only by nature)
- All GET endpoints for conversation data (messages, state) work without
  auth when accessed via the share path
- POST/PUT/DELETE endpoints always require auth (from task 08642)

### Frontend

- Read-only view: same message list and StateBar, but no InputArea
- Banner at top: "Read-only view" or "Shared by [user]"
- Live updates via SSE (same stream, just no ability to send)
- No settings, no file explorer, no work actions -- just the
  conversation stream

## Depends on

Task 08642 (basic password auth) -- share mode is the "unauthenticated
GET" carve-out from the auth middleware.

## Done when

- [ ] `/share/{slug}` shows live conversation without auth
- [ ] No input area or mutation controls in share view
- [ ] SSE stream works for unauthenticated viewers
- [ ] Share view updates in real-time as the owner works
