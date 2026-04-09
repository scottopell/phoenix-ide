# Auth & Share -- Executive Summary

## Overview

Single-user password authentication and read-only share mode for Phoenix
IDE. Prevents unauthorized mutations in shared workspace environments and
enables real-time conversation sharing for pair programming and demos.

## Status

| Requirement | Title | Status |
|---|---|---|
| REQ-AUTH-001 | Password-Gated Access | ❌ Not Started |
| REQ-AUTH-002 | Stateless Password Verification | ❌ Not Started |
| REQ-AUTH-003 | Login Flow | ❌ Not Started |
| REQ-AUTH-004 | Share Token Creation | ❌ Not Started |
| REQ-AUTH-005 | Read-Only Share View | ❌ Not Started |
| REQ-AUTH-006 | Share Token Exemption from Auth | ❌ Not Started |
| REQ-AUTH-007 | Multiple Simultaneous Viewers | ❌ Not Started |
| REQ-AUTH-008 | Share Token Persistence | ❌ Not Started |

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
