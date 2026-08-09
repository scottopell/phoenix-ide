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
   server-backed grounding and files, read prose comfortably, and send anchored
   comments while retaining a clear non-editing boundary.
3. **Build a full mobile IDE surface** — maximizes capability parity, but brings
   terminal, editing, diff, and filesystem semantics whose cost and interaction
   model are disproportionate to the mobile use case.

## Decision

Choose bounded read-only project context and prose review. The native client may
browse content fetched from the Phoenix server, render supported prose in a
dedicated reader, and create durable anchored comments. It does not expose
server paths as phone-local locations and does not grow terminal, general file
editing, chains, or diff-viewer capability through this decision.

This option supplies the evidence and feedback loop needed for mobile review
without erasing the companion boundary that keeps the client tractable.

## Consequences

- **Positive:** Users can inspect the grounding and prose behind a conversation
  and provide feedback without moving to a desktop client.
- **Negative:** Phoenix needs stable server-content and comment-anchor contracts,
  and the iOS client must make stale content, failed delivery, and changed anchors
  explicit.
- **Neutral:** The work follows the ProductConversation migration and remains
  separate from general editing, terminal access, chains, and diff review.

## References

- `specs/ios_client/requirements.md`
- `specs/ios_client/executive.md`
- ADR-026, for ProductConversation lifecycle and WorkScope ownership boundaries.
