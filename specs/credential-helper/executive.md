# Credential Helper -- Executive Summary

## Overview

Interactive credential helper support for `LLM_API_KEY_HELPER`. When the
configured helper requires user action (OIDC device flow, SSO), Phoenix
streams the helper's stdout to the browser in real-time so the user can
complete auth without leaving the UI.

The existing `CommandCredential` silently captures all stdout as the token.
This spec extends it to: extract only the last non-empty line as the
credential, stream intermediate lines to the UI, and expose credential
status in the models API so the UI can surface actionable auth state.

## Status

| Requirement | Title | Status |
|---|---|---|
| REQ-CREDHELPER-001 | Last-Line Credential Extraction | ❌ Not Started |
| REQ-CREDHELPER-002 | Credential Status in Models Response | ❌ Not Started |
| REQ-CREDHELPER-003 | Auth Execution Endpoint | ❌ Not Started |
| REQ-CREDHELPER-004 | Single Concurrent Execution | ❌ Not Started |
| REQ-CREDHELPER-005 | Failure Surfacing | ❌ Not Started |
| REQ-CREDHELPER-006 | TTL-Based Caching | ❌ Not Started |
| REQ-CREDHELPER-007 | Cache Invalidation on 401 | ❌ Not Started |
| REQ-CREDHELPER-008 | UI Auth Panel | ❌ Not Started |

## Scope

**In:** Interactive helpers emitting OIDC device codes; streaming stdout to
browser; credential status in models API; global auth indicator in UI;
SSE-based run endpoint; single-concurrent-execution guarantee; last-line
token extraction.

**Out:** OAuth refresh tokens; multiple concurrent helpers (one provider);
helper per-provider overrides; storing credentials to disk; automatic
re-auth on TTL expiry (user-triggered only).

## Allium Spec

Behavioral specification: `specs/credential-helper/credential-helper.allium`

Defines the `CredentialHelper` entity lifecycle (idle → running → valid /
failed), the `HelperOutputBroadcast` concept, and invariants (single
instance, last-line extraction, no credential on non-zero exit).
