# ADR-010: Durable workflows use an engine-owned normalized core and typed profiles

- **Status:** Accepted
- **Date:** 2026-07-11
- **Affects:** REQ-DWF-001, REQ-DWF-002, REQ-DWF-003, REQ-DWF-013, REQ-DWF-015, REQ-DWF-016, REQ-DWF-WAKE-001 through REQ-DWF-WAKE-005, REQ-DWF-CREATE-001 through REQ-DWF-CREATE-005

## Context

Conversation creation and wake delivery each need durable scheduling, recovery,
fencing, cancellation, and audit history. Keeping their schedulers independent
would duplicate safety machinery, while a domain-neutral blob store would erase
type boundaries and let generic infrastructure acquire product meaning. The
existing product reducer already owns conversation semantics and must remain the
one authority for user-visible state.

A first adopter is needed to keep the shared design honest. Wake exercises
observation, later runtime acceptance, coalescing, continuation, and terminal
substrate evidence. Creation then exercises a larger dependency graph, resource
reservation, destructive reconciliation, and compensation.

## Options considered

1. **Keep bespoke wake and creation schedulers.** This minimizes near-term change,
   but duplicates claims, deadlines, retries, evidence, and acceptance machinery
   and leaves no shared correctness boundary.
2. **Use one generic opaque workflow document.** This centralizes scheduling, but
   hides authority fields in payloads, weakens schema constraints, and encourages
   the core to interpret domain semantics.
3. **Use an engine-owned normalized core with registered typed profiles.** The
   core owns execution truth; profiles own non-overlapping domain intent, codecs,
   adapters, reducer mapping, locks, compensation, and projections. Adopt wake
   first, then creation through shadow parity and versioned cutover.

## Decision

Adopt option 3. The engine owns normalized workflow snapshots, transition history,
effects and dependency rows, claims and leases, attempts, observations, receipts,
barriers, deadlines, and scheduling. Queryable authority never depends on JSON
extraction. Typed profiles own domain state and families without duplicating core
facts. The existing product reducer remains the sole semantic authority.

Wake is the first authoritative engine profile after shadow parity. Conversation
creation follows under a separate protocol selection and drain. This adoption
order is a delivery decision, not a wake-specific limitation on the engine.

## Consequences

- **Positive:** Safety machinery is implemented once and exercised by two
  materially different profiles.
- **Positive:** Relational constraints protect authority while typed profile
  boundaries keep product semantics out of generic infrastructure.
- **Positive:** Wake delivers user value before the larger creation migration.
- **Negative:** Profile registration and codec compatibility become explicit
  contracts, and both legacy executors must coexist during drain.
- **Negative:** Some existing wake and creation tables become temporary parallel
  diagnostic or legacy representations until their authority drains.
- **Neutral:** Future workflow profiles require separate product justification;
  the engine is not an arbitrary plugin platform.

## References

- Related ADRs: ADR-007, ADR-008, ADR-009, ADR-011, ADR-012
- Feature spec: `specs/durable-workflows/requirements.md`
- Profile specs: `specs/wake-contracts/requirements.md`, `specs/conversation-creation/requirements.md`
