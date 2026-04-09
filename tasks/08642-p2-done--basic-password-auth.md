---
created: 2026-04-07
priority: p2
status: done
artifact: src/api/handlers.rs
---

# Basic single-user password authentication

## Problem

In workspace environments (DD workspaces, exe.dev), forwarded ports are
accessible to coworkers without authentication. Anyone with the URL can
send messages, approve tasks, abandon conversations, etc.

## What to build

KISS: a single password set via environment variable (e.g., `PHOENIX_PASSWORD`).
When set, all mutating API endpoints require the password. When unset,
no auth (current behavior).

### Backend

- Middleware or extractor that checks for auth on all non-GET endpoints
  (or all endpoints except the read-only share routes from task 08643)
- Auth via cookie set by a login endpoint, or a simple bearer token in
  a header -- cookie is better UX since the browser handles it
- `POST /api/auth/login` accepts `{"password": "..."}`, sets a session
  cookie if correct
- `GET /api/auth/status` returns whether auth is required and whether
  the current session is authenticated
- Session cookie: random token stored in-memory (no DB), expires with
  server restart (acceptable for single-user)

### Frontend

- If auth is required and not authenticated, show a password prompt
  instead of the app
- Simple single-field form, no username (single user)
- Store session cookie, auto-redirect to app on success

## Constraints

- No user management, no registration, no password hashing complexity
- Just bcrypt or even plain string comparison of env var
- If `PHOENIX_PASSWORD` is not set, skip all auth entirely
- Read-only share mode (task 08643) should work without auth

## Done when

- [ ] `PHOENIX_PASSWORD=foo ./dev.py up` requires password to use the app
- [ ] Without the env var, no auth required (backward compatible)
- [ ] Session persists across page refreshes (cookie)
- [ ] All mutating endpoints are protected
