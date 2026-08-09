# ADR-028: The iOS companion includes read-only project context and prose review

- **Status:** Accepted
- **Date:** 2026-08-08
- **Affects:** REQ-IOS-019, REQ-IOS-020, REQ-IOS-021

## Context

The native client begins as a conversation-focused companion optimized for
unreliable mobile connectivity. That boundary keeps a phone from becoming a
second implementation of the full web IDE, but it also withholds evidence users
need for high-value mobile decisions: the conversation's grounding, the project
files an agent is using, and prose artifacts that need review.

Phoenix runs on a server that may be a different machine from the phone. Server
paths are therefore location handles, not phone-local files. Any expanded native
surface must preserve that boundary and must not imply general remote editing,
terminal control, or a phone-local filesystem.

## Options considered

1. **Keep the companion conversation-only** — preserves the smallest client and
   avoids new file and annotation contracts, but forces users back to the web UI
   for the context behind mobile approvals and questions.
2. **Add bounded read-only project context and prose review** — lets users inspect
   server-backed grounding and files, read prose comfortably, and turn
   session-scoped notes into an editable conversation draft while retaining a
   clear non-editing boundary.
3. **Build a full mobile IDE surface** — maximizes capability parity, but brings
   terminal, editing, diff, and filesystem semantics whose cost and interaction
   model are disproportionate to the mobile use case.

## Decision

Choose bounded read-only project context and prose review. The native client may
browse content fetched from the Phoenix server, render supported prose in a
dedicated reader, and collect session-scoped notes that are formatted into the
conversation's editable message input. Notes are not a separate durable delivery
channel. It does not expose server paths as phone-local locations, invoke
server-host desktop reveal actions, or grow terminal, general file editing,
chains, or diff-viewer capability through this decision.

This option supplies the evidence and feedback loop needed for mobile review
without erasing the companion boundary that keeps the client tractable.

## Consequences

- **Positive:** Users can inspect the grounding and prose behind a conversation
  and provide feedback without moving to a desktop client.
- **Negative:** Phoenix needs stable server-content and exact WorkScope/file
  identity contracts; notes do not survive closing the reader and must be sent
  to the composer or explicitly discarded first.
- **Neutral:** The work follows the ProductConversation migration and remains
  separate from general editing, terminal access, chains, diff review, and a
  second durable comment-delivery lifecycle.

## References

- `specs/ios_client/requirements.md`
- `specs/ios_client/executive.md`
- `specs/prose-feedback/requirements.md`, especially REQ-PF-009 through REQ-PF-011.
- `specs/work-lifecycle/requirements.md`, especially REQ-WL-002 and REQ-WL-002b.
- ADR-026, for ProductConversation lifecycle and WorkScope ownership boundaries.
