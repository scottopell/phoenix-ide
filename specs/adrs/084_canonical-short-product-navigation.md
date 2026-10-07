# ADR-084: Canonical short ProductConversation navigation

Status: Accepted

Date: 2026-10-07

Affects: ProductConversation browser navigation and compatibility reference resolution.

## Context

Stable product IDs and root transcript IDs can share underlying bytes. Identical unqualified URLs cannot simultaneously promise stable continuation following and exact historical selection. Existing source links carry explicit transcript and message evidence.

## Options considered

1. Keep separate long stable URLs and short transcript URLs.
2. Infer identity from UUID spelling.
3. Make the short product URL stable and retain explicit evidence selectors for historical navigation.

## Decision

The user approved option 3 for the current ProductConversation milestone. Bare authoritative product references use `/c/<product-id>` and follow continuation. Explicit transcript/message evidence remains pinned. Membership and domain are validated; invalid explicit selectors fail rather than fall back to latest. Existing long routes remain compatibility aliases. API endpoints and the desktop URI grammar remain unchanged.

## Consequences

Canonical route producers, browser ownership, and compatibility resolution must agree. Source-link producers retain explicit transcript identity even without a tool locator. No historical database rewriting is required. Global Coordinator navigation stays in its own domain. Cross-version compatibility beyond these explicit aliases is not introduced.
