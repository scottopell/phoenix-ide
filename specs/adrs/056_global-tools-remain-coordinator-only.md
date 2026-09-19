# ADR-056: Phoenix-wide tools remain Coordinator-only

- **Status:** Accepted
- **Date:** 2026-09-19
- **Affects:** REQ-GR-007, REQ-RET-009
- **Supersedes:** ADR-027 and ADR-051 wherever they grant ordinary parents Phoenix-wide search, read, database-query, reference-resolution, or cross-conversation messaging capabilities

## Context

Ordinary parents need evidence from their own predecessor transcripts, but that need does not require Phoenix-wide authority. Giving ordinary write-capable parents global search and read tools expands their authority beyond the executing transcript lineage and makes scoped predecessor recall redundant.

The Global Coordinator already owns Phoenix-wide orientation and cross-conversation capabilities. Predecessor recall has a distinct host-bound scope derived from the executing transcript and ProductConversation lineage.

## Options considered

1. Give ordinary write-capable parents both global and predecessor tools.
2. Give ordinary parents only scoped predecessor tools and retain global tools on the Coordinator.
3. Remove global tools from every agent surface.

## Decision

The Global Coordinator exclusively receives Phoenix-wide history search, global conversation reads, database queries, reference resolution, and cross-conversation messaging.

Ordinary parents receive only host-bound predecessor discovery, search, and read tools. Those tools derive authority from the executing transcript and reject targets outside its predecessor lineage. Planning conversations and sub-agents receive neither global tools nor ordinary-parent predecessor tools unless a separate normative capability explicitly grants them.

## Consequences

Ordinary parents can recover relevant lineage evidence without gaining ambient access to unrelated Phoenix history. The Coordinator remains the sole agent surface for Phoenix-wide orientation. Existing historical decisions remain authoritative except for their superseded grants of Phoenix-wide search, read, database-query, reference-resolution, or cross-conversation messaging capabilities to ordinary parents.
