# Auth & Share -- Executive Summary

## Overview

Single-user password authentication and read-only share mode for Phoenix
IDE. Prevents unauthorized mutations in shared workspace environments and
enables real-time conversation sharing for pair programming and demos.

## Status

| Requirement | Title | Status | Notes |
|---|---|---|---|
| REQ-AUTH-001 | Password-Gated Access | ✅ Complete | `crates/phoenix-ide/src/main.rs:205-210` reads `PHOENIX_PASSWORD`; `api.rs:39` carries it on `ServerState`; middleware in `api/auth.rs:101+` enforces |
| REQ-AUTH-002 | Stateless Password Verification | ✅ Complete | `api/auth.rs:19` constant-time compare via `subtle`-style `constant_time_eq`; used at `:47,:58,:171` |
| REQ-AUTH-003 | Login Flow | ✅ Complete | Login endpoint in `api/auth.rs`; cookie set on success; login page styled at `ui/src/index.css:7545` |
| REQ-AUTH-004 | Share Token Creation | ✅ Complete | `api/handlers.rs:3346,3351,3365`; reuses existing token if present; 302 to `/s/{token}` |
| REQ-AUTH-005 | Read-Only Share View | ✅ Complete | `api/handlers.rs:3374`; share page styled at `ui/src/index.css:7643` |
| REQ-AUTH-006 | Share Token Exemption from Auth | ✅ Complete | `api/handlers.rs:3401`; share routes validate token instead of password; `ui/src/api.ts:811-812` `getSharedConversation` |
| REQ-AUTH-007 | Multiple Simultaneous Viewers | ✅ Complete | `api/handlers.rs:3446`; SSE-validated on token, no per-viewer mutation |
| REQ-AUTH-008 | Share Token Persistence | ✅ Complete | `share_tokens` table in `db/schema.rs:172-182`; CRUD at `db.rs:212-280` |

**Progress:** 8 of 8 complete. Both Phase 1 (auth) and Phase 2 (share) shipped.

## MVP Scope

**Phase 1 (auth):** REQ-AUTH-001 through REQ-AUTH-003. Password protection
for all endpoints. Login page. Cookie persistence.

**Phase 2 (share):** REQ-AUTH-004 through REQ-AUTH-008. Share token
creation via URL, read-only view, SSE streaming, DB persistence.

Phase 1 is independently useful -- it protects the instance even without
sharing. Phase 2 depends on Phase 1 (share tokens exempt from auth that
must exist first).

## Allium Spec

Behavioral specification: `specs/auth/auth.allium`

Defines actors (`Owner`, `Viewer`), surfaces (`OwnerConversation`,
`SharedConversation`), share token entity, creation/revocation rules,
and invariants (unique tokens, constant-time comparison, no tokens
without auth).
