# ADR-041: Minimal History finalization

- **Status:** Accepted
- **Date:** 2026-08-26
- **Affects:** REQ-WL-002b, REQ-CONV-001
- **Supersedes:** The FTS, permanent-delete, crash-recoverable deletion, restore, retention, and bulk-lifecycle consequences of ADR-038 for this milestone

## Context

Close retirement establishes the destructive boundary for a ProductConversation. The next product increment needs only a durable successful completion and a read-only History view. Search indexing, deletion, restoration, retention, and bulk lifecycle controls add independent durable authorities and recovery contracts without advancing that immediate outcome.

## Options considered

1. Include search, deletion, restoration, retention, and bulk lifecycle in the initial History finalizer.
2. Persist an atomic successful Close outcome and History transition only, then add later lifecycle capabilities behind their own authority.

## Decision

After successful Close retirement, one exact-attempt transaction records one successful Close outcome and transitions the ordinary ProductConversation to History. Replaying the same finalizer is idempotent. History is read-only and derives from the aggregate lifecycle authority.

The finalizer persists only its completion obligation. This milestone does not implement FTS projection, permanent deletion, restoration, retention, bulk lifecycle operations, or lifecycle-aware cleanup.

## Consequences

- Users can see successfully closed aggregates in History without changing the Close retirement contract.
- Close has one final completion obligation rather than a deletion or search pipeline.
- Later lifecycle capabilities require their own accepted decision and durable authority.

## References

- ADR-038
- ADR-040
- `specs/work-lifecycle/requirements.md`
